//! Router boundary tests: verified signed OIDC sessions, no injected Caller.
use super::*;
use aiwatcher_auth::{Credential, Identity, Role, signing::Signer};
use aiwatcher_iam::{IamStore, Principal, memory::MemoryIamStore};
const SECRET: &str = "iam-http-integration-session-secret";
const ROOT: &str = "/api/v1/iam/organizations";

struct IamFixture {
    fixture: Fixture,
    issuer: String,
    provider_task: tokio::task::JoinHandle<()>,
    store: Arc<MemoryIamStore>,
    clock: Arc<IamClock>,
}
#[derive(Debug)]
struct IamClock(std::sync::atomic::AtomicI64);
impl aiwatcher_iam::Clock for IamClock {
    fn now(&self) -> i64 {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}
impl Drop for IamFixture {
    fn drop(&mut self) {
        self.provider_task.abort();
    }
}
impl IamFixture {
    async fn new() -> Self {
        Self::build(false).await
    }

    /// The same instance with an object store behind it, so the authored
    /// registries answer their handlers rather than 501 before reaching one.
    /// What the contract sweep needs: a route that refused for want of a store
    /// says nothing about whether it would have refused for want of a role.
    async fn with_registries() -> Self {
        Self::build(true).await
    }

    async fn build(registries: bool) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let metadata = json!({ "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/authorize"),
            "token_endpoint": format!("{issuer}/token"), "jwks_uri": format!("{issuer}/jwks") });
        let app = axum::Router::new()
            .route("/.well-known/openid-configuration", axum::routing::get(move || async move { axum::Json(metadata) }))
            .route("/jwks", axum::routing::get(|| async { axum::Json(json!({"keys": [{
                "kty": "RSA", "kid": "test", "use": "sig", "alg": "RS256",
                "n": include_str!("../../../aiwatcher-auth/tests/fixtures/signing-key.modulus").trim(), "e": "AQAB"
            }]})) }));
        let provider_task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let auth = Authenticator::connect(AuthConfig {
            mode: AuthMode::Oidc,
            issuer: issuer.clone(),
            client_id: "iam-test".into(),
            redirect_url: "http://localhost/api/v1/auth/callback".into(),
            session_secret: Some(SECRET.into()),
            discovery_attempts: 1,
            ..AuthConfig::default()
        })
        .await
        .unwrap()
        .unwrap();
        let clock = Arc::new(IamClock(std::sync::atomic::AtomicI64::new(1000)));
        let store = Arc::new(MemoryIamStore::with_clock(clock.clone()));
        let mut fixture = Fixture::build(false, registries, None, Some(Arc::new(auth)), None);
        fixture.state.iam = Some(store.clone());
        Self {
            fixture,
            issuer,
            provider_task,
            store,
            clock,
        }
    }
    fn cookie(&self, subject: &str, role: Role) -> String {
        let mut identity = Identity::anonymous();
        identity.issuer = Some(self.issuer.clone());
        identity.subject = subject.into();
        identity.credential = Credential::Session;
        identity.roles = vec![role];
        identity.expires_at = Some(time::OffsetDateTime::now_utc().unix_timestamp() + 3600);
        self.seal(&identity)
    }
    /// A client of this deployment: signed in, holding no instance role.
    ///
    /// What `AIWATCHER_AUTH_DEFAULT_ROLE=project` produces, spelled as the
    /// identity rather than as the configuration — `RoleMapping::resolve` has
    /// its own tests for the mapping, and what these ask about is the router.
    fn client_cookie(&self, subject: &str) -> String {
        let mut identity = Identity::anonymous();
        identity.issuer = Some(self.issuer.clone());
        identity.subject = subject.into();
        identity.credential = Credential::Session;
        identity.roles = Vec::new();
        identity.expires_at = Some(time::OffsetDateTime::now_utc().unix_timestamp() + 3600);
        self.seal(&identity)
    }

    fn seal(&self, identity: &Identity) -> String {
        let token = Signer::new(SECRET.as_bytes())
            .seal(identity, time::Duration::hours(1))
            .unwrap();
        format!("{}={token}", AuthConfig::default().cookie_name)
    }
    async fn request(
        &self,
        method: &str,
        path: &str,
        cookie: Option<&str>,
        body: Value,
        marker: bool,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(cookie) = cookie {
            request = request.header(header::COOKIE, cookie);
        }
        if marker {
            request = request.header("x-aiwatcher-iam", "1");
        }
        self.fixture
            .request(request.body(Body::from(body.to_string())).unwrap())
            .await
    }
    /// Give this instance somewhere to put a frozen copy of a trail.
    ///
    /// Left out by default, because that is the deployment that has an IAM
    /// store and no object store — and the one whose export routes have to
    /// answer 501 rather than queue a job that could never write a shard.
    fn with_exports(mut self) -> Self {
        self.fixture.state.iam_audit_exports = Some(Arc::new(aiwatcher_iam::AuditExports::new(
            Arc::new(MemoryObjectStore::new()),
            "iam-audit",
        )));
        self
    }

    /// Run whatever export is queued, as the worker would.
    async fn drain(&self, organization: &str) {
        let exports = self.fixture.state.iam_audit_exports.clone().unwrap();
        for (org, job) in exports
            .claimable(time::OffsetDateTime::now_utc())
            .await
            .unwrap()
        {
            assert_eq!(org.0.to_string(), organization);
            exports
                .run(self.store.as_ref(), org, &job, "a test")
                .await
                .unwrap();
        }
    }

    async fn create(&self, cookie: &str) -> String {
        let (status, org) = self
            .request(
                "POST",
                ROOT,
                Some(cookie),
                json!({"name": "HTTP organization"}),
                true,
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{org}");
        org["id"].as_str().unwrap().to_owned()
    }

    async fn project(&self, cookie: &str, organization: &str) -> String {
        let (status, created) = self
            .request(
                "POST",
                &format!("{ROOT}/{organization}/commands"),
                Some(cookie),
                json!({"type": "create_project", "name": "A client's project"}),
                true,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{created}");
        created["ProjectCreated"]["scope"]["project"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    async fn grant(
        &self,
        cookie: &str,
        organization: &str,
        project: &str,
        subject: &str,
        role: &str,
    ) {
        let commands = format!("{ROOT}/{organization}/commands");
        self.request(
            "POST",
            &commands,
            Some(cookie),
            json!({"type": "set_member",
                "principal": {"provider": self.issuer, "subject": subject},
                "role": "member"}),
            true,
        )
        .await;
        let (status, granted) = self
            .request(
                "POST",
                &commands,
                Some(cookie),
                json!({"type": "grant", "project": project,
                    "grantee": {"kind": "user",
                        "value": {"provider": self.issuer, "subject": subject}},
                    "role": role, "window": {"valid_from": 0}}),
                true,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{granted}");
    }
}

#[tokio::test]
async fn iam_bootstrap_and_cross_organization_boundaries() {
    let f = IamFixture::new().await;
    let owner = f.cookie("owner", Role::Admin);
    let outsider = f.cookie("outsider", Role::Admin);
    let reader = f.cookie("reader", Role::Viewer);
    assert_eq!(
        f.request("GET", ROOT, None, Value::Null, false).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.request("POST", ROOT, Some(&reader), json!({"name":"denied"}), true)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.request("POST", ROOT, Some(&owner), json!({"name":"denied"}), false)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let org = f.create(&owner).await;
    let other = f.create(&outsider).await;
    let commands = format!("{ROOT}/{org}/commands");
    let audit = format!("{ROOT}/{org}/audit");
    let (status, created) = f
        .request(
            "POST",
            &commands,
            Some(&owner),
            json!({"type":"create_project","name":"Project"}),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let project = created["ProjectCreated"]["scope"]["project"]
        .as_str()
        .unwrap();
    for path in [
        format!("{ROOT}/{org}/projects"),
        format!("{ROOT}/{org}/projects/{project}/access"),
        audit.clone(),
    ] {
        assert_eq!(
            f.request("GET", &path, Some(&outsider), Value::Null, false)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        f.request(
            "GET",
            &format!("{ROOT}/{other}/projects/{project}/access"),
            Some(&outsider),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.request(
            "POST",
            &commands,
            Some(&outsider),
            json!({"type":"create_team","name":"denied"}),
            true
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let (_, discovered) = f
        .request("GET", ROOT, Some(&owner), Value::Null, false)
        .await;
    assert_eq!(discovered.as_array().unwrap().len(), 1);
    assert_eq!(discovered[0]["id"], org);
    let (_, entries) = f
        .request("GET", &audit, Some(&owner), Value::Null, false)
        .await;
    assert_eq!(entries.as_array().unwrap().len(), 2);
    assert_eq!(entries[1]["actor"]["subject"], "owner");
    assert_eq!(entries[1]["action"]["change"], created);
}

#[tokio::test]
async fn iam_membership_and_revocation_are_fresh_for_the_same_session() {
    let f = IamFixture::new().await;
    let owner = f.cookie("owner", Role::Admin);
    let reader = f.cookie("reader", Role::Viewer);
    let org = f.create(&owner).await;
    let commands = format!("{ROOT}/{org}/commands");
    let principal = json!({"provider":f.issuer,"subject":"reader"});
    assert_eq!(
        f.request(
            "POST",
            &commands,
            Some(&owner),
            json!({"type":"set_member","principal":principal,"role":"member"}),
            true
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, created) = f
        .request(
            "POST",
            &commands,
            Some(&owner),
            json!({"type":"create_project","name":"Project"}),
            true,
        )
        .await;
    let project = created["ProjectCreated"]["scope"]["project"]
        .as_str()
        .unwrap();
    let path = format!("{ROOT}/{org}/projects/{project}/access");
    assert_eq!(
        f.request("GET", &path, Some(&reader), Value::Null, false)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let (_, grant) = f.request("POST", &commands, Some(&owner), json!({"type":"grant","project":project,
        "grantee":{"kind":"user","value":principal},"role":"editor","window":{"valid_from":0,"edit_until":null,"read_until":null}}), true).await;
    let (status, access) = f
        .request("GET", &path, Some(&reader), Value::Null, false)
        .await;
    assert_eq!(status, StatusCode::OK, "{access}");
    assert_eq!(access["role"], "editor");
    assert_eq!(
        f.request(
            "GET",
            &format!("{ROOT}/{org}/audit"),
            Some(&reader),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.request(
            "POST",
            &commands,
            Some(&reader),
            json!({"type":"create_team","name":"denied"}),
            true
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.request(
            "POST",
            &commands,
            Some(&owner),
            json!({"type":"revoke_grant","project":project,"grant":grant["GrantCreated"]["id"]}),
            true
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.request("GET", &path, Some(&reader), Value::Null, false)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    // Neither an added actor field nor a bootstrap owner field can spoof the caller.
    assert_eq!(
        f.request(
            "POST",
            ROOT,
            Some(&owner),
            json!({"name":"spoof","owner":principal}),
            true
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        f.request(
            "POST",
            &commands,
            Some(&owner),
            json!({"type":"create_team","name":"spoof","actor":principal}),
            true
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn iam_rejects_legacy_expired_foreign_and_non_oidc_sessions() {
    let mut f = IamFixture::new().await;
    let owner = Principal::new(&f.issuer, "owner").unwrap();
    f.store.create_organization(&owner, "IAM").await.unwrap();
    let mut identity = Identity::anonymous();
    identity.subject = "owner".into();
    identity.credential = Credential::Session;
    identity.roles = vec![Role::Admin];
    identity.expires_at = Some(time::OffsetDateTime::now_utc().unix_timestamp() + 3600);
    let legacy = f.seal(&identity);
    assert_eq!(
        f.request("GET", ROOT, Some(&legacy), Value::Null, false)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    identity.issuer = Some("https://different-provider.test".into());
    assert_eq!(
        f.request("GET", ROOT, Some(&f.seal(&identity)), Value::Null, false)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    identity.issuer = Some(f.issuer.clone());
    identity.expires_at = Some(1);
    assert_eq!(
        f.request("GET", ROOT, Some(&f.seal(&identity)), Value::Null, false)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    f.fixture.state.auth = None;
    assert_eq!(
        f.request("GET", ROOT, None, Value::Null, false).await.0,
        StatusCode::UNAUTHORIZED
    );
    f.fixture.state.iam = None;
    assert_eq!(
        f.request("GET", ROOT, None, Value::Null, false).await.0,
        StatusCode::NOT_IMPLEMENTED
    );
    let mut proxy = Fixture::behind_a_proxy(false).await;
    proxy.state.iam = Some(f.store.clone());
    assert_eq!(
        proxy.get_as(ROOT, "owner", "aiwatcher-admins").await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn iam_expiry_and_audit_pagination_are_evaluated_on_each_request() {
    let f = IamFixture::new().await;
    let cookie = f.cookie("owner", Role::Admin);
    let org = f.create(&cookie).await;
    let commands = format!("{ROOT}/{org}/commands");
    let (_, created) = f
        .request(
            "POST",
            &commands,
            Some(&cookie),
            json!({"type":"create_project","name":"Expires"}),
            true,
        )
        .await;
    let project = created["ProjectCreated"]["scope"]["project"]
        .as_str()
        .unwrap();
    let path = format!("{ROOT}/{org}/projects/{project}/access");
    let (_, initial) = f
        .request("GET", &path, Some(&cookie), Value::Null, false)
        .await;
    let permanent = &initial["grants"][0]["grant"]["id"];
    let (_, timed) = f
        .request(
            "POST",
            &commands,
            Some(&cookie),
            json!({"type":"grant","project":project,
        "grantee":{"kind":"user","value":{"provider":f.issuer,"subject":"owner"}},
        "role":"editor","window":{"valid_from":1000,"edit_until":1010,"read_until":1020}}),
            true,
        )
        .await;
    assert!(timed.get("GrantCreated").is_some(), "{timed}");
    assert_eq!(
        f.request(
            "POST",
            &commands,
            Some(&cookie),
            json!({"type":"revoke_grant","project":project,"grant":permanent}),
            true
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.request("GET", &path, Some(&cookie), Value::Null, false)
            .await
            .1["role"],
        "editor"
    );
    f.clock.0.store(1010, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        f.request("GET", &path, Some(&cookie), Value::Null, false)
            .await
            .1["role"],
        "viewer"
    );
    f.clock.0.store(1020, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        f.request("GET", &path, Some(&cookie), Value::Null, false)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    // Organization ownership gives audit administration but no implicit data read grant.
    let audit = format!("{ROOT}/{org}/audit?after=1&limit=2");
    let (_, page) = f
        .request("GET", &audit, Some(&cookie), Value::Null, false)
        .await;
    assert_eq!(page.as_array().unwrap().len(), 2);
    assert_eq!(page[0]["sequence"], 2);
    assert_eq!(page[1]["sequence"], 3);
    assert_eq!(
        f.request(
            "POST",
            &commands,
            Some(&cookie),
            json!({"type":"remove_member","principal":{"provider":f.issuer,"subject":"owner"}}),
            true
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let response = aiwatcher_api::router(f.fixture.state.clone())
        .oneshot(
            Request::builder()
                .uri(&path)
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
}

/// What a link offers, to somebody who has not signed in and may have no way to.
///
/// D5's two public routes. The offer is readable by whoever holds the token and
/// by nobody else, looking costs it nothing, and the grant is still made by the
/// redemption — under a session, against the pair it verified.
#[tokio::test]
async fn an_offer_reads_to_a_stranger_and_is_spent_only_by_a_redemption() {
    let f = IamFixture::new().await;
    let owner = f.cookie("owner", Role::Admin);
    let organization = f.create(&owner).await;
    let project = f.project(&owner, &organization).await;
    let (status, issued) = f
        .request(
            "POST",
            &format!("{ROOT}/{organization}/projects/{project}/invitations"),
            Some(&owner),
            json!({"role": "editor", "window": {"valid_from": 0},
                "expires_at": time::OffsetDateTime::now_utc().unix_timestamp() + 3600,
                "label": "somebody@example.test"}),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{issued}");
    let token = issued["token"].as_str().expect("a token").to_owned();
    assert_eq!(
        issued["invitation"]["standing"], "guest",
        "an offer that does not ask for a colleague makes a guest (D3)"
    );

    // No cookie at all: this is the whole point of the route.
    let preview = "/api/v1/iam/invitations/preview";
    let (status, offered) = f
        .request("POST", preview, None, json!({"token": token}), false)
        .await;
    assert_eq!(status, StatusCode::OK, "{offered}");
    assert_eq!(offered["organization"]["id"], organization);
    assert_eq!(offered["project"]["scope"]["project"], project);
    assert_eq!(offered["role"], "editor");
    assert_eq!(offered["standing"], "guest");

    // A token nobody issued is absent rather than a hint, as it is on redeem.
    assert_eq!(
        f.request(
            "POST",
            preview,
            None,
            json!({"token": "0".repeat(64)}),
            false
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    // Enrolment needs somewhere to enrol into, named.
    let (status, refused) = f
        .request(
            "POST",
            "/api/v1/iam/invitations/enrollment",
            None,
            json!({"token": token}),
            false,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert!(
        refused["message"]
            .as_str()
            .expect("a message")
            .contains("AIWATCHER_AUTH_PROVISION_URL"),
        "{refused}"
    );

    // Looking cost the offer nothing, and redeeming still takes a session.
    let stranger = f.client_cookie("a stranger");
    let (status, redeemed) = f
        .request(
            "POST",
            "/api/v1/iam/invitations/redeem",
            Some(&stranger),
            json!({"token": token}),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{redeemed}");
    assert_eq!(redeemed["role"], "editor");
    // …and now there is nothing left to preview.
    assert_eq!(
        f.request("POST", preview, None, json!({"token": token}), false)
            .await
            .0,
        StatusCode::CONFLICT
    );
}

#[path = "project_datasets.rs"]
mod project_datasets;

async fn project(f: &IamFixture, cookie: &str, org: &str) -> String {
    let (status, body) = f
        .request(
            "POST",
            &format!("{ROOT}/{org}/commands"),
            Some(cookie),
            json!({"type":"create_project","name":"Test"}),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["ProjectCreated"]["scope"]["project"]
        .as_str()
        .unwrap()
        .into()
}
fn base(org: &str, project: &str) -> String {
    format!("/api/v1/orgs/{org}/projects/{project}")
}
async fn command(f: &IamFixture, owner: &str, org: &str, command: Value) -> Value {
    let (status, body) = f
        .request(
            "POST",
            &format!("{ROOT}/{org}/commands"),
            Some(owner),
            command,
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}
async fn grant(
    f: &IamFixture,
    owner: &str,
    org: &str,
    project: &str,
    role: &str,
    window: Value,
) -> Value {
    let principal = json!({"provider":f.issuer,"subject":"member"});
    command(
        f,
        owner,
        org,
        json!({"type":"set_member","principal":principal,"role":"member"}),
    )
    .await;
    command(f, owner, org, json!({"type":"grant","project":project,"grantee":{"kind":"user","value":principal},"role":role,"window":window})).await["GrantCreated"]["id"].clone()
}

/// An audit export is administrator-only, needs the mutation header, and is
/// 501 on an instance with nowhere to put one.
#[tokio::test]
async fn iam_audit_exports_are_administered_frozen_and_read_back() {
    let disabled = IamFixture::new().await;
    let cookie = disabled.cookie("owner", Role::Admin);
    let org = disabled.create(&cookie).await;
    let (status, body) = disabled
        .request(
            "POST",
            &format!("{ROOT}/{org}/audit/exports"),
            Some(&cookie),
            json!({}),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{body}");
    assert_eq!(body["code"], "iam_audit_exports_disabled");
    drop(disabled);

    let f = IamFixture::new().await.with_exports();
    let owner = f.cookie("owner", Role::Admin);
    let org = f.create(&owner).await;
    let commands = format!("{ROOT}/{org}/commands");
    let outsider = f.cookie("outsider", Role::Admin);
    for name in ["Alpha", "Beta"] {
        f.request(
            "POST",
            &commands,
            Some(&owner),
            json!({"type":"create_project","name":name}),
            true,
        )
        .await;
    }

    let exports = format!("{ROOT}/{org}/audit/exports");
    // A write with no `X-AIWatcher-IAM: 1` is refused before anything is
    // queued, like every other mutation here.
    assert_eq!(
        f.request("POST", &exports, Some(&owner), json!({}), false)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    // And an instance administrator of another organization is told it does
    // not exist.
    assert_eq!(
        f.request("POST", &exports, Some(&outsider), json!({}), true)
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    let (status, job) = f
        .request(
            "POST",
            &exports,
            Some(&owner),
            json!({"reason": "a question in writing"}),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{job}");
    assert_eq!(job["state"], "queued");
    assert_eq!(job["through_sequence"], 3, "three entries, pinned");
    assert_eq!(job["cursor"], 0);
    let job_id = job["job_id"].as_str().unwrap().to_owned();

    // A retried POST joins the job it already started rather than freezing the
    // same range twice.
    let (_, again) = f
        .request(
            "POST",
            &exports,
            Some(&owner),
            json!({"reason": "a question in writing"}),
            true,
        )
        .await;
    assert_eq!(again["job_id"], job_id);

    // Nothing to read until it has finished.
    let rows = format!("{exports}/{job_id}/rows");
    assert_eq!(
        f.request("GET", &rows, Some(&owner), Value::Null, false)
            .await
            .0,
        StatusCode::CONFLICT
    );

    f.drain(&org).await;

    let (status, done) = f
        .request(
            "GET",
            &format!("{exports}/{job_id}"),
            Some(&owner),
            Value::Null,
            false,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["state"], "completed");
    assert_eq!(done["counts"]["entries"], 3);
    assert_eq!(done["counts"]["missing"], 0);
    assert!(done["version"].as_str().is_some(), "a content address");

    let (status, page) = f
        .request("GET", &rows, Some(&owner), Value::Null, false)
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["total"], 3);
    assert_eq!(page["entries"][0]["sequence"], 1);
    assert_eq!(page["entries"][2]["action"]["type"], "command_applied");
    assert!(page["next_offset"].is_null());

    // A finished export is a record; cancelling it is a 409 rather than a
    // silent success.
    assert_eq!(
        f.request(
            "DELETE",
            &format!("{exports}/{job_id}"),
            Some(&owner),
            Value::Null,
            true
        )
        .await
        .0,
        StatusCode::CONFLICT
    );

    // Where the trail begins, and — with nothing swept — that it is whole.
    let (status, bounds) = f
        .request(
            "GET",
            &format!("{ROOT}/{org}/audit/bounds"),
            Some(&owner),
            Value::Null,
            false,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{bounds}");
    assert_eq!(bounds["first_sequence"], 1);
    assert_eq!(bounds["last_sequence"], 3);
    assert_eq!(bounds["entries"], 3);
    assert!(bounds["watermark"].is_null());

    // Ordinary membership administers nothing here.
    let member = f.cookie("member", Role::Editor);
    f.request(
        "POST",
        &commands,
        Some(&owner),
        json!({"type":"set_member","principal":{"provider":f.issuer,"subject":"member"},"role":"member"}),
        true,
    )
    .await;
    for (method, path) in [
        ("GET", format!("{ROOT}/{org}/audit/bounds")),
        ("GET", exports.clone()),
        ("GET", format!("{exports}/{job_id}")),
        ("GET", rows.clone()),
    ] {
        assert_eq!(
            f.request(method, &path, Some(&member), Value::Null, false)
                .await
                .0,
            StatusCode::FORBIDDEN,
            "{method} {path}"
        );
    }
}

#[path = "project_prompts.rs"]
mod project_prompts;

#[path = "project_training.rs"]
mod project_training;

#[path = "project_annotations.rs"]
mod project_annotations;

#[path = "project_evaluation.rs"]
mod project_evaluation;

#[path = "project_definitions.rs"]
mod project_definitions;

#[path = "project_reviews.rs"]
mod project_reviews;

#[path = "project_cohorts.rs"]
mod project_cohorts;

#[path = "project_recordings.rs"]
mod project_recordings;

#[path = "project_bundles.rs"]
mod project_bundles;

#[path = "project_evidence.rs"]
mod project_evidence;

#[path = "project_calibrations.rs"]
mod project_calibrations;

#[path = "project_declarations.rs"]
mod project_declarations;

#[path = "project_judged.rs"]
mod project_judged;

#[path = "project_labs.rs"]
mod project_labs;

#[path = "project_runs.rs"]
mod project_runs;

#[path = "instance_reach.rs"]
mod instance_reach;

#[path = "project_run_artifacts.rs"]
mod project_run_artifacts;
