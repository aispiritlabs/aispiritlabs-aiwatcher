#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Opening the identity provider's enrolment to one person (IAM-03 D5).
//!
//! Against a loopback provider that answers the two calls authentik's API
//! answers, so what is under test is the adapter's own rules rather than
//! authentik: that it asks for an *invitation* and nothing else, that the URL
//! it hands back is built here from the configured base, and that a provider
//! saying no is a refusal with the provider's words in it rather than a panic.

use std::sync::Arc;
use std::time::Duration;

use aiwatcher_auth::{AccountProvisioning, AuthError, ProvisioningConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// What the fake provider was asked, so a test can read it back.
#[derive(Debug, Default)]
struct Seen {
    paths: Vec<String>,
    bodies: Vec<String>,
    authorization: Option<String>,
}

/// A provider that answers both calls, or refuses the second.
async fn provider(refuse: bool, flows: &str) -> (String, Arc<tokio::sync::Mutex<Seen>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let url = format!("http://{}", listener.local_addr().expect("an address"));
    let seen = Arc::new(tokio::sync::Mutex::new(Seen::default()));
    let recording = Arc::clone(&seen);
    let flows = flows.to_owned();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut buffer = vec![0_u8; 8192];
            let read = stream.read(&mut buffer).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
            let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
            let body = request.split("\r\n\r\n").nth(1).unwrap_or("").to_owned();
            {
                let mut seen = recording.lock().await;
                seen.paths.push(path.clone());
                if !body.is_empty() {
                    seen.bodies.push(body);
                }
                seen.authorization = request
                    .lines()
                    .find(|line| line.to_ascii_lowercase().starts_with("authorization:"))
                    .map(|line| line["authorization:".len()..].trim().to_owned());
            }
            let (status, answer) = if path.starts_with("/api/v3/flows/instances/") {
                ("200 OK", flows.clone())
            } else if refuse {
                ("403 Forbidden", r#"{"detail":"no permission"}"#.to_owned())
            } else {
                ("201 Created", r#"{"pk":"the-invitation"}"#.to_owned())
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                answer.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
    });
    (url, seen)
}

fn config(url: String) -> ProvisioningConfig {
    ProvisioningConfig {
        url,
        token: "a service account token".to_owned(),
        flow: "aiwatcher-enrolment".to_owned(),
        ttl: Duration::from_secs(1800),
        http_timeout: Duration::from_secs(5),
    }
}

const ONE_FLOW: &str = r#"{"results":[{"pk":"the-flow"}]}"#;

#[tokio::test]
async fn an_enrolment_is_one_invitation_and_a_url_this_process_builds() {
    let (url, seen) = provider(false, ONE_FLOW).await;
    let provisioning = AccountProvisioning::new(config(url.clone())).expect("configured");

    let enrollment = provisioning
        .open_enrollment(Some("somebody@example.test"))
        .await
        .expect("an enrolment");

    // The URL is this deployment's configured base and the id that came back —
    // never a URL the provider chose, which is the rule a rerun target keeps
    // for the same reason.
    assert_eq!(
        enrollment.url,
        format!("{url}/if/flow/aiwatcher-enrolment/?itoken=the-invitation")
    );
    assert!(enrollment.expires_at > time::OffsetDateTime::now_utc().unix_timestamp());

    let seen = seen.lock().await;
    assert_eq!(
        seen.paths,
        vec![
            "/api/v3/flows/instances/?slug=aiwatcher-enrolment",
            "/api/v3/stages/invitation/invitations/",
        ],
        "it resolves the flow and creates an invitation, and asks for nothing else"
    );
    assert_eq!(
        seen.authorization.as_deref(),
        Some("Bearer a service account token")
    );
    let created: serde_json::Value =
        serde_json::from_str(seen.bodies.last().expect("a body")).expect("json");
    assert_eq!(created["single_use"], true, "one person, once");
    assert_eq!(created["flow"], "the-flow");
    assert_eq!(
        created["fixed_data"]["email"], "somebody@example.test",
        "the label prefills the form and decides nothing"
    );
    assert!(
        created.get("groups").is_none() && created.get("user").is_none(),
        "it creates no user and joins nobody to a group: {created}"
    );
}

#[tokio::test]
async fn a_provider_that_refuses_is_a_refusal_carrying_what_it_said() {
    let (url, _) = provider(true, ONE_FLOW).await;
    let provisioning = AccountProvisioning::new(config(url)).expect("configured");
    let refused = provisioning
        .open_enrollment(None)
        .await
        .expect_err("refused");
    assert!(
        matches!(&refused, AuthError::Provisioning(said) if said.contains("403")
            && said.contains("no permission")),
        "{refused}"
    );
    assert!(
        !refused.is_caller_fault() && !refused.is_retryable(),
        "not the person's doing, and not worth trying again on its own"
    );
}

#[tokio::test]
async fn a_flow_the_provider_does_not_have_names_the_variable_to_fix() {
    let (url, _) = provider(false, r#"{"results":[]}"#).await;
    let provisioning = AccountProvisioning::new(config(url)).expect("configured");
    let refused = provisioning
        .open_enrollment(None)
        .await
        .expect_err("refused");
    assert!(
        matches!(&refused, AuthError::Provisioning(said)
            if said.contains("AIWATCHER_AUTH_PROVISION_FLOW")),
        "{refused}"
    );
}

#[test]
fn half_a_configuration_is_refused_before_anything_starts() {
    for (field, broken) in [
        (
            "token",
            ProvisioningConfig {
                token: "  ".to_owned(),
                ..config("http://host".into())
            },
        ),
        (
            "flow",
            ProvisioningConfig {
                flow: String::new(),
                ..config("http://host".into())
            },
        ),
        ("url", config("not a url".into())),
    ] {
        assert!(
            AccountProvisioning::new(broken).is_err(),
            "an empty {field} has to be a start-up failure, not a call that fails later"
        );
    }
}
