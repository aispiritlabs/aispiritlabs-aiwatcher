//! Every operation in the contract, asked by somebody holding no instance role.
//!
//! IAM-03's D4 in one sweep. The rule it holds is not "these routes refuse" but
//! **this caller reads nothing of the deployment's** — so the list is taken from
//! `contracts/openapi.json` rather than written here, for the reason
//! `SCOPED_ROUTES` in the panel is: a hand-written list is a list that goes
//! stale in the direction nobody notices, which is the route somebody added
//! last week.
//!
//! Two properties, and the second is what stops the next handler forgetting.
//!
//! * Nothing outside [`REACHES`] answers with data. That is the plan's
//!   property, and it is what a client on a shared deployment is owed.
//! * Every instance operation refuses with **`instance_role_required`** and not
//!   with something else that also happens to carry no rows. A 400 from a
//!   query parameter this sweep guessed wrong would satisfy the first property
//!   while telling us nothing, so the second reads as a rule about handlers:
//!   **on an instance route the caller is extracted before the request is
//!   parsed.** A handler that takes no caller at all fails here, which is how
//!   the four in `imports`, the four in `conversations` and the pairs in
//!   `artifacts`, `context` and `hubs` were found.
use super::*;

/// What a signed-in caller holding no instance role still reaches, and why.
///
/// Small on purpose, and each line is a route that answers about the *caller*
/// rather than about anything this deployment holds.
const REACHES: &[(&str, &str)] = &[
    (
        "GET /api/v1/auth/me",
        "who the caller is — refusing it reports a valid session as signed out",
    ),
    (
        "GET /api/v1/iam/organizations",
        "which organizations are theirs: the list the scope selector is built from",
    ),
];

/// Answered before authentication at all (`auth::is_public`): the probes a
/// kubelet reaches with no credential, and the sign-in routes, which cannot
/// require a session in order to establish one. Out of the sweep because this
/// is about an authenticated caller, and these answer whoever asks.
const PUBLIC: &[&str] = &[
    "/livez",
    "/healthz",
    "/readyz",
    "/api/v1/auth/config",
    "/api/v1/auth/login",
    "/api/v1/auth/callback",
    "/api/v1/auth/logout",
];

/// One request per operation, with every path parameter filled in.
fn contract() -> Vec<(String, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .parent()
        .expect("the workspace root")
        .join("contracts/openapi.json");
    let document: Value =
        serde_json::from_str(&std::fs::read_to_string(&root).expect("the contract")).expect("json");
    let mut operations = Vec::new();
    for (template, item) in document["paths"].as_object().expect("paths") {
        if PUBLIC.contains(&template.as_str()) {
            continue;
        }
        for method in ["GET", "POST", "PUT", "DELETE"] {
            if item.get(method.to_lowercase()).is_some() {
                operations.push((method.to_owned(), filled(template)));
            }
        }
    }
    operations.sort();
    operations
}

/// A path parameter's stand-in. The scope's two are real uuids because the
/// scoped family parses them before anything else; the rest name nothing,
/// which is all this sweep needs them to do.
fn filled(template: &str) -> String {
    let mut path = String::new();
    for segment in template.split('/').skip(1) {
        path.push('/');
        match segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            Some("organization") => path.push_str(ORGANIZATION),
            Some("project") => path.push_str(PROJECT),
            Some(_) => path.push_str("nothing-of-mine"),
            None => path.push_str(segment),
        }
    }
    path
}

const ORGANIZATION: &str = "0198c0de-0000-7000-8000-00000000000f";
const PROJECT: &str = "0198c0de-0000-7000-8000-0000000000ff";
/// A project's own family: refused here because this principal holds no grant,
/// which is the 404 IAM-02's M1 measured and not D4's business.
const SCOPED: &str = "/api/v1/orgs/";
/// The control plane, which authorizes **itself**, per principal: an
/// organization this caller is in answers, one they are not is 404, and neither
/// answer comes from an instance role. Its refusals are therefore its own.
const CONTROL_PLANE: &str = "/api/v1/iam/";

#[tokio::test]
async fn nothing_this_deployment_holds_answers_a_caller_with_no_instance_role() {
    let f = IamFixture::with_registries().await;
    let client = f.client_cookie("a client of this deployment");

    let mut answered = Vec::new();
    let mut misrefused = Vec::new();
    for (method, path) in contract() {
        let operation = format!("{method} {path}");
        if REACHES.iter().any(|(reached, _)| *reached == operation) {
            continue;
        }
        let asked = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            f.request(&method, &path, Some(&client), json!({}), true),
        )
        .await;
        let Ok((status, body)) = asked else {
            answered.push(format!("{operation} — held the connection open"));
            continue;
        };
        if status.is_success() || status.is_redirection() {
            answered.push(format!("{status} {operation}"));
            continue;
        }
        // The scoped family refuses for the other reason on purpose: this
        // principal holds no grant, and `IamStore::access` answers 404 for
        // that, which is the boundary IAM-02's M1 already measured.
        let code = body.get("code").and_then(Value::as_str).unwrap_or("none");
        if !path.starts_with(SCOPED)
            && !path.starts_with(CONTROL_PLANE)
            && code != "instance_role_required"
        {
            misrefused.push(format!("{status} {code} — {operation}"));
        }
    }

    assert!(
        answered.is_empty(),
        "{} operation(s) answered a caller holding no instance role:\n  {}",
        answered.len(),
        answered.join("\n  ")
    );
    assert!(
        misrefused.is_empty(),
        "{} instance operation(s) refused for a reason other than the boundary — a handler \
         that parses the request before it takes the caller refuses the right way round only \
         by luck:\n  {}",
        misrefused.len(),
        misrefused.join("\n  ")
    );
}

#[tokio::test]
async fn the_routes_that_do_answer_are_the_two_that_answer_about_the_caller() {
    let f = IamFixture::with_registries().await;
    let client = f.client_cookie("a client of this deployment");

    for (operation, why) in REACHES {
        let (method, path) = operation.split_once(' ').expect("method and path");
        let (status, _) = f
            .request(method, path, Some(&client), json!({}), true)
            .await;
        assert_eq!(status, StatusCode::OK, "{operation} — {why}");
    }
}

#[tokio::test]
async fn a_grant_is_what_makes_a_client_able_to_read_anything_at_all() {
    // The other half of D4, and the half that makes it a product rather than a
    // lockout: the same caller, the same session, one grant — and their own
    // project's runs answer while the instance's list still refuses.
    let f = IamFixture::with_registries().await;
    let owner = f.cookie("owner", Role::Admin);
    let client = f.client_cookie("a client of this deployment");
    let (_, identity) = f
        .request("GET", "/api/v1/auth/me", Some(&client), json!({}), false)
        .await;
    let subject = identity["subject"].as_str().expect("a subject").to_owned();

    let organization = f.create(&owner).await;
    let project = f.project(&owner, &organization).await;
    f.grant(&owner, &organization, &project, &subject, "viewer")
        .await;

    let scoped = format!("/api/v1/orgs/{organization}/projects/{project}/runs");
    let (status, page) = f
        .request("GET", &scoped, Some(&client), json!({}), false)
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");

    let (status, body) = f
        .request("GET", "/api/v1/runs", Some(&client), json!({}), false)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "instance_role_required");
}
