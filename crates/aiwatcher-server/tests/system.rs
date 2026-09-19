// See the note in aiwatcher-bus/tests.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! No secret this deployment was configured with reaches `GET /api/v1/system`.
//!
//! The inventory's whole difficulty is one line: **that a thing is configured
//! is not a secret, and its value often is.** `aiwatcher-api` can assert the
//! static half of that — that no setting prints the variable of a credential —
//! but not the half that matters, because it builds its state by hand and
//! therefore never holds a real secret to leak.
//!
//! This does. It configures a deployment the way an operator would, with a
//! **recognisable value in every sensitive variable**, wires it through the
//! same [`aiwatcher_server::build`] the binary uses, asks the route, and fails
//! if any of those values comes back. A future field that serialised an
//! authenticator, an object store or a runner would fail here and nowhere
//! else.
//!
//! Two things keep it honest. The needles are distinct strings, so a failure
//! names which one leaked. And there is a **positive control**: the response
//! has to still name the variables and carry the issuer, so a route that broke,
//! answered `{}` or lost its capabilities cannot pass by saying nothing.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use aiwatcher_auth::{AttemptCredentials, AuthConfig, AuthMode, IngestToken};
use aiwatcher_server::Config;
use aiwatcher_server::config::{
    BackendKind, PodRuntime, ProcessRole, PromptStoreKind, WorkflowRunnerKind, WorkflowStoreKind,
};

/// A value nothing else in a response could plausibly be, per sensitive
/// variable.
///
/// The variable is here so a failure says *which* setting leaked rather than
/// only that something did — and so that adding a sensitive variable to the
/// configuration and forgetting this list is a visible omission rather than a
/// silent one.
const NEEDLES: &[(&str, &str)] = &[
    ("AIWATCHER_AUTH_CLIENT_SECRET", "needle-oidc-client-secret"),
    ("AIWATCHER_AUTH_SESSION_SECRET", "needle-session-secret"),
    (
        "AIWATCHER_AUTH_INGEST_TOKENS",
        "needleingesttokensecret0123456789",
    ),
    (
        "AIWATCHER_POD_CREDENTIAL_SECRET",
        "needle-pod-credential-secret",
    ),
    (
        "AIWATCHER_PROMPT_S3_ENDPOINT",
        "needle-object-store.invalid",
    ),
    ("AIWATCHER_PROMPT_S3_BUCKET", "needle-bucket"),
    ("AIWATCHER_PROMPT_S3_ACCESS_KEY", "needle-s3-access-key"),
    ("AIWATCHER_PROMPT_S3_SECRET_KEY", "needle-s3-secret-key"),
    (
        "AIWATCHER_PROMPT_S3_SESSION_TOKEN",
        "needle-s3-session-token",
    ),
    (
        "AIWATCHER_CONVERSATION_KEYS",
        "bmVlZGxlLWNvbnZlcnNhdGlvbi1hcmNoaXZlLWtleSE",
    ),
    ("AIWATCHER_KAGGLE_USERNAME", "needle-kaggle-user"),
    ("AIWATCHER_KAGGLE_KEY", "needle-kaggle-key"),
    ("AIWATCHER_HUGGINGFACE_TOKEN", "needle-huggingface-token"),
    ("AIWATCHER_JUDGE_URL", "https://needle-judge.invalid/v1"),
    (
        "AIWATCHER_AUTH_PROVISION_URL",
        "https://needle-enrolment.invalid",
    ),
    ("AIWATCHER_AUTH_PROVISION_TOKEN", "needle-provision-token"),
    ("AIWATCHER_AUTH_PROVISION_FLOW", "needle-enrolment-flow"),
    ("AIWATCHER_JUDGE_TOKEN", "needle-judge-token"),
    ("AIWATCHER_SCORER_URL", "https://needle-scorers.invalid"),
    ("AIWATCHER_SCORER_TOKEN", "needle-scorer-token"),
    (
        "AIWATCHER_ML_PIPELINE_URL",
        "https://needle-notebooks.invalid",
    ),
    ("AIWATCHER_QUERY_URL", "https://needle-query.invalid"),
    (
        "AIWATCHER_WORKFLOW_RUNNER_URL",
        "https://needle-orchestrator.invalid/run",
    ),
    (
        "AIWATCHER_WORKFLOW_RUNNER_TOKEN",
        "needle-workflow-runner-token",
    ),
    (
        "AIWATCHER_ALERT_WEBHOOK_URL",
        "https://needle-alert-receiver.invalid/hook",
    ),
    (
        "AIWATCHER_ALERT_WEBHOOK_TOKEN",
        "needle-alert-webhook-token",
    ),
    (
        "AIWATCHER_ALERT_WEBHOOK_SECRET",
        "needle-alert-webhook-secret",
    ),
    (
        "AIWATCHER_WORKFLOW_POSTGRES_URL",
        "postgres://needle:needle-pg-password@needle-db.invalid/needle",
    ),
    (
        "AIWATCHER_IAM_POSTGRES_URL",
        "postgres://needle:needle-iam-password@needle-iam.invalid/needle",
    ),
    (
        "AIWATCHER_LASER_CONNECTION_STRING",
        "needle:needle-broker-password@needle-broker.invalid:8090",
    ),
    ("AIWATCHER_POD_API_URL", "https://needle-api.invalid"),
    ("AIWATCHER_POD_NAMESPACE", "needle-namespace"),
    (
        "AIWATCHER_OTLP_ENDPOINT",
        "https://needle-collector.invalid",
    ),
    // Not a variable of its own: it is inside the templates file, which is the
    // one piece of this configuration written the way a secret is referenced.
    ("AIWATCHER_POD_TEMPLATES", "needle-image-pull-secret"),
];

fn needle(variable: &str) -> &'static str {
    NEEDLES
        .iter()
        .find(|(name, _)| *name == variable)
        .map(|(_, value)| *value)
        .unwrap_or_else(|| panic!("{variable} has no needle"))
}

/// The issuer, which is deliberately *not* a needle.
///
/// It is the one address this route says in full — "which authentik is this
/// pointing at" is otherwise unanswerable without a shell on the pod, and it
/// is already public on `/api/v1/auth/config` before anybody has signed in. It
/// is the positive control: a test that only looked for absences would pass
/// against a route that had stopped answering.
const ISSUER: &str = "https://sso.example.test/application/o/aiwatcher/";

struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("aiwatcher-system-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("a scratch directory");
        Self(path)
    }

    fn write(&self, name: &str, body: &str) -> String {
        let path = self.0.join(name);
        std::fs::write(&path, body).expect("writes");
        path.to_string_lossy().into_owned()
    }

    fn path(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A deployment with as much wired as can be wired with nothing running.
///
/// The backends that need a process — PostgreSQL for the workflow store and
/// for IAM, a broker for the log — are configured but not selected, so their
/// connection strings are held by `Config` and never reach `AppState`. That is
/// the point rather than a gap: the route is handed `AppState` and nothing
/// else, and the assertion is that the response holds none of these strings by
/// whatever path.
///
/// Everything that *can* be reached is: an S3 object store keeps the access
/// key and the secret it signs with, the authenticator keeps the client
/// secret, the session secret, the ingest tokens and the pod credential key,
/// the hubs keep Kaggle's and Hugging Face's credentials, the runner keeps its
/// endpoint and bearer token, and the templates keep the operator's own pod
/// fragment.
fn configured(scratch: &Scratch) -> Config {
    let templates = scratch.write(
        "pod-templates.json",
        &format!(
            r#"{{"houses": {{
                 "images": ["registry.example.test/houses"],
                 "resources": {{"max": {{"cpu": "2", "memory": "4Gi"}}}},
                 "command": ["aiwatcher-worker", "run-attempt"],
                 "pod": {{"imagePullSecrets": [{{"name": "{}"}}]}}
               }}}}"#,
            needle("AIWATCHER_POD_TEMPLATES")
        ),
    );
    let prices = scratch.write(
        "prices.json",
        r#"{"currency": "USD", "prices": [{"model": "gpt-4.1-mini", "input_per_million": 0.4,
           "output_per_million": 1.6, "source": "https://openai.com/api/pricing",
           "as_of": "2026-01-02"}]}"#,
    );

    Config {
        // Nothing this test starts should outlive it, and nothing it writes
        // should land next to a real instance's write-ahead log.
        data_dir: scratch.path(),
        bus: BackendKind::Memory,
        // The shipped seed imports curation recipes through the object store,
        // which here is an S3 endpoint that does not exist.
        seed_file: None,
        laser_connection_string: Some(needle("AIWATCHER_LASER_CONNECTION_STRING").to_owned()),
        otlp_endpoint: Some(needle("AIWATCHER_OTLP_ENDPOINT").to_owned()),
        ingest_enabled: true,

        // An object store that holds a real credential. `create_bucket` is
        // off, so connecting reaches nothing: the signer is built and never
        // asked to sign.
        prompt_store: PromptStoreKind::S3,
        prompt_s3_endpoint: Some(format!(
            "https://{}",
            needle("AIWATCHER_PROMPT_S3_ENDPOINT")
        )),
        prompt_s3_bucket: needle("AIWATCHER_PROMPT_S3_BUCKET").to_owned(),
        prompt_s3_access_key: Some(needle("AIWATCHER_PROMPT_S3_ACCESS_KEY").to_owned()),
        prompt_s3_secret_key: Some(needle("AIWATCHER_PROMPT_S3_SECRET_KEY").to_owned()),
        prompt_s3_session_token: Some(needle("AIWATCHER_PROMPT_S3_SESSION_TOKEN").to_owned()),
        prompt_s3_create_bucket: false,

        conversation_archive: true,
        conversation_keys: Some(format!(
            "needle-key-id:{}",
            needle("AIWATCHER_CONVERSATION_KEYS")
        )),

        // Held by `Config` and never selected, so the connection strings stay
        // where they were read. Both would need a database running and a cargo
        // feature this test does not ask for.
        workflow_store: WorkflowStoreKind::File,
        workflow_postgres_url: Some(needle("AIWATCHER_WORKFLOW_POSTGRES_URL").to_owned()),
        iam_postgres_url: None,

        workflow_runner: WorkflowRunnerKind::Http,
        workflow_runner_url: Some(needle("AIWATCHER_WORKFLOW_RUNNER_URL").to_owned()),
        workflow_runner_token: Some(needle("AIWATCHER_WORKFLOW_RUNNER_TOKEN").to_owned()),

        alert_webhook_url: Some(needle("AIWATCHER_ALERT_WEBHOOK_URL").to_owned()),
        alert_webhook_token: Some(needle("AIWATCHER_ALERT_WEBHOOK_TOKEN").to_owned()),
        alert_webhook_secret: Some(needle("AIWATCHER_ALERT_WEBHOOK_SECRET").to_owned()),

        huggingface_enabled: true,
        huggingface_token: Some(needle("AIWATCHER_HUGGINGFACE_TOKEN").to_owned()),
        kaggle_username: Some(needle("AIWATCHER_KAGGLE_USERNAME").to_owned()),
        kaggle_key: Some(needle("AIWATCHER_KAGGLE_KEY").to_owned()),

        judge_url: Some(needle("AIWATCHER_JUDGE_URL").to_owned()),
        judge_provider: Some("openai".to_owned()),
        judge_token: Some(needle("AIWATCHER_JUDGE_TOKEN").to_owned()),
        scorer_url: Some(needle("AIWATCHER_SCORER_URL").to_owned()),
        scorer_token: Some(needle("AIWATCHER_SCORER_TOKEN").to_owned()),
        ml_pipeline_url: Some(needle("AIWATCHER_ML_PIPELINE_URL").to_owned()),
        query_url: Some(needle("AIWATCHER_QUERY_URL").to_owned()),
        query_engine: aiwatcher_datasets::QueryEngine::DuckDb,

        // `serve` only checks a step's request against the templates, which is
        // what lets this run with no cluster client compiled in.
        role: ProcessRole::Serve,
        pod_templates: Some(templates),
        pod_runtime: PodRuntime::Kubernetes,
        pod_namespace: Some(needle("AIWATCHER_POD_NAMESPACE").to_owned()),
        pod_api_url: Some(needle("AIWATCHER_POD_API_URL").to_owned()),

        model_prices: Some(prices),
        witnesses: vec!["houses".to_owned()],

        // Proxy mode: a real identity with a real role, and the one mode that
        // establishes one without a provider to discover. The secrets beside
        // it are held all the same.
        auth: AuthConfig {
            mode: AuthMode::Proxy,
            issuer: ISSUER.to_owned(),
            client_id: "aiwatcher".to_owned(),
            client_secret: Some(needle("AIWATCHER_AUTH_CLIENT_SECRET").to_owned()),
            session_secret: Some(needle("AIWATCHER_AUTH_SESSION_SECRET").to_owned()),
            ingest_tokens: vec![
                format!("houses={}", needle("AIWATCHER_AUTH_INGEST_TOKENS"))
                    .parse::<IngestToken>()
                    .expect("long enough to be accepted"),
            ],
            attempts: Some(
                AttemptCredentials::new(Some(needle("AIWATCHER_POD_CREDENTIAL_SECRET")))
                    .expect("a secret"),
            ),
            ..AuthConfig::default()
        },
        provisioning: Some(aiwatcher_auth::ProvisioningConfig {
            url: needle("AIWATCHER_AUTH_PROVISION_URL").to_owned(),
            token: needle("AIWATCHER_AUTH_PROVISION_TOKEN").to_owned(),
            flow: needle("AIWATCHER_AUTH_PROVISION_FLOW").to_owned(),
            ttl: std::time::Duration::from_secs(1800),
            http_timeout: std::time::Duration::from_secs(5),
        }),
        ..Config::default()
    }
}

async fn inventory(config: Config) -> (StatusCode, String) {
    let runtime = aiwatcher_server::build(config)
        .await
        .expect("a deployment that needs nothing running");
    let response = aiwatcher_api::router(runtime.state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/system")
                .header("x-authentik-username", "ops")
                .header("x-authentik-groups", "aiwatcher-admins")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("responds");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("collects");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn the_inventory_names_every_sensitive_variable_and_prints_none_of_their_values() {
    let scratch = Scratch::new("secrets");
    let (status, body) = inventory(configured(&scratch)).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    for (variable, value) in NEEDLES {
        assert!(
            !body.contains(value),
            "GET /api/v1/system printed the value of {variable}"
        );
    }

    // The positive control, in three parts, because each would let a leak
    // through unnoticed on its own: the route answered with an inventory, that
    // inventory names the variables whose values were withheld, and it says in
    // full the one address it is meant to say.
    assert!(
        body.contains("\"capabilities\""),
        "the body is not an inventory: {body}"
    );
    for variable in [
        "AIWATCHER_PROMPT_STORE",
        "AIWATCHER_CONVERSATION_ARCHIVE",
        "AIWATCHER_CONVERSATION_KEYS",
        "AIWATCHER_WORKFLOW_RUNNER_URL",
        "AIWATCHER_ML_PIPELINE_URL",
        "AIWATCHER_JUDGE_URL",
        "AIWATCHER_SCORER_URL",
        "AIWATCHER_KAGGLE_KEY",
        "AIWATCHER_POD_TEMPLATES",
    ] {
        assert!(
            body.contains(variable),
            "{variable} is named by nothing, so a reader cannot tell what to set"
        );
    }
    assert!(
        body.contains(ISSUER),
        "the issuer is the one address this route says in full, and it did not: {body}"
    );

    // And the things the inventory is *for*, on a deployment that has them:
    // the states are real, the engine is the one chosen, and a template is
    // named without its body.
    assert!(body.contains("\"duckdb\""), "the query engine: {body}");
    assert!(
        body.contains("\"houses\""),
        "the pod template's name: {body}"
    );
    assert!(
        body.contains("\"kubernetes\""),
        "what a pod is here: {body}"
    );
    assert!(
        !body.contains("imagePullSecrets"),
        "a template's body reached the inventory: {body}"
    );
}

#[tokio::test]
async fn a_deployment_with_nothing_wired_says_so_rather_than_answering_nothing() {
    // The other side of the same route, and the one a fresh `aiwatcher up`
    // is in: no object store, no archive, nothing reached out to. It must be
    // an inventory of refusals rather than an empty document, because an empty
    // one reads as "this instance has nothing to say".
    let scratch = Scratch::new("bare");
    let (status, body) = inventory(Config {
        data_dir: scratch.path(),
        bus: BackendKind::Memory,
        seed_file: None,
        prompt_store: PromptStoreKind::None,
        workflow_store: WorkflowStoreKind::Memory,
        auth: AuthConfig {
            mode: AuthMode::Proxy,
            ..AuthConfig::default()
        },
        ..Config::default()
    })
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let document: serde_json::Value = serde_json::from_str(&body).expect("an inventory");
    let capabilities = document["capabilities"].as_array().expect("capabilities");
    let not_configured = capabilities
        .iter()
        .filter(|capability| capability["state"] == "not_configured")
        .count();
    assert!(
        not_configured > 10,
        "a deployment with nothing wired reported {not_configured} refusals: {body}"
    );
    assert!(
        capabilities
            .iter()
            .any(|capability| capability["id"] == "query-engine"
                && capability["state"] == "configured"),
        "an engine is chosen even where nothing else is: {body}"
    );
}
