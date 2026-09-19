//! What this instance is.
//!
//! One read route, and it answers a question every other area of this API
//! answers only by refusing: **which of this system's capabilities does this
//! deployment actually have, and which variable decides each one.** Until now
//! that answer was scattered across twenty 501s — `RegistryDisabled` names
//! `AIWATCHER_PROMPT_STORE`, `HubsDisabled` names three more — each of them
//! correct, each of them reachable only by asking for something and being told
//! no. This is the same set of facts, read out once, before anybody asks.
//!
//! **It is written over [`AppState`] and over nothing else.** Every state here
//! is a field this process already holds, so a capability this crate cannot
//! see does not appear: an inventory that listed what a deployment *might*
//! have wired would be the plausible fake `AreaPlaceholder` exists to refuse,
//! one level down.
//!
//! ## What may be said, and what may not
//!
//! **That a thing is configured is not a secret; its value often is.** The
//! split this module keeps:
//!
//! - **Never a value that is a credential.** No token, no key, no session or
//!   cookie secret, no client secret, no pod credential secret. Nothing here
//!   reads one, and the regression in `tests/system.rs` over in the server
//!   crate puts a recognisable string into every one of them and fails if it
//!   comes back.
//! - **Never an address, either.** A database URL, an object store's endpoint,
//!   a judge's or a notebook runtime's host — none of these is a credential and
//!   all of them are reconnaissance for somebody who is already inside. They
//!   are reported as presence and a variable name, never as content, which is
//!   the same line `build_iam_store` draws when it refuses to log the URL it
//!   just connected with.
//! - **Values that name a choice are said in full**, because they are what the
//!   inventory is for: the query engine, the pod runtime, the template names,
//!   the auth mode and its issuer, a price table's currency, what the execution
//!   store can do.
//!
//! ## Why `admin`
//!
//! This is an inventory of the deployment, not an aid to somebody reading a
//! run. Every line of it — which stores are wired, which third parties this
//! instance reaches out to, which provider it trusts, which images a pod may
//! run — is of use to an operator and of use to an attacker, and of no use at
//! all to the person looking at why a run was slow. So it sits with the two
//! other operator-shaped routes: the rerun, and reading a conversation's words.
//!
//! ## What this is not
//!
//! **Not a write path.** Changing a setting is an environment variable and a
//! restart, and that is the point rather than a limitation: a configuration a
//! route could change would be one whose value nothing durable explains.
//!
//! **Not a health probe.** Whether a configured service is answering right now
//! is a different question with a different failure mode and a different
//! refresh rate; `/readyz` is this process's own, and a hub that will not
//! answer says so on the route that asked it. An inventory that went red when
//! somebody else's service restarted would be read as this one being broken.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use utoipa::OpenApi;

use aiwatcher_auth::Role;

use crate::auth::Caller;
use crate::error::ApiResult;
use crate::state::AppState;

/// This module's operations, as the contract they satisfy.
#[derive(OpenApi)]
#[openapi(paths(system,))]
struct Api;

/// The operations this module serves. Composed by [`crate::openapi`].
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    Api::openapi()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/system", get(system))
        // An inventory of a deployment has no business in a shared cache, and
        // it is cheap enough that there is nothing to gain by letting it.
        .layer(tower_http::set_header::SetResponseHeaderLayer::overriding(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-store"),
        ))
}

// ── The shape ────────────────────────────────────────────────────────────────

/// Whether this deployment has a capability at all.
///
/// Two states and no third, because the question this route answers is a
/// deployment's rather than a service's: "wired" and "not wired" are decided
/// here, and "up" and "down" are decided somewhere this process is not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = SystemCapabilityState)]
pub enum CapabilityState {
    Configured,
    NotConfigured,
}

impl CapabilityState {
    const fn of(configured: bool) -> Self {
        if configured {
            Self::Configured
        } else {
            Self::NotConfigured
        }
    }
}

/// Which half of the deployment a capability belongs to.
///
/// The grouping is the reader's, not the code's: an operator asking "what does
/// this instance reach out to" wants one answer and not five, and the same for
/// "where does it put things" and "what can it run".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = SystemCapabilityGroup)]
pub enum CapabilityGroup {
    /// Who the caller is, and who decides what they may reach.
    Identity,
    /// The log, what folds it, and what it is read against.
    Observability,
    /// Where things that outlive a run are kept.
    Storage,
    /// What work runs here, and under what.
    Runtime,
    /// What leaves the building: a service this deployment reaches but does
    /// not run.
    Integration,
}

/// One value of a capability that is safe to say out loud.
///
/// Never a credential and never an address — see this module's header for
/// where that line falls and why it falls there.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
#[schema(as = SystemSetting)]
pub struct Setting {
    /// What this value is, as somebody reads it: `engine`, `pod runtime`,
    /// `template`.
    pub name: String,
    /// The value, rendered. A number is a string here because this is an
    /// inventory somebody reads rather than a number anybody computes with,
    /// and one shape per row is worth more to the panel than one type per
    /// setting.
    pub value: String,
    /// The variable that set it, where one did. Absent for what this process
    /// worked out rather than read — how many corpora a catalogue held, what
    /// an execution store's adapter can do.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variable: Option<String>,
}

impl Setting {
    fn read(name: &str, value: impl Into<String>, variable: &str) -> Self {
        Self {
            name: name.to_owned(),
            value: value.into(),
            variable: Some(variable.to_owned()),
        }
    }

    fn derived(name: &str, value: impl Into<String>) -> Self {
        Self {
            name: name.to_owned(),
            value: value.into(),
            variable: None,
        }
    }
}

/// One thing this deployment either has or has not.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
#[schema(as = SystemCapability)]
pub struct Capability {
    /// Stable across releases. What the panel keys a row on, and what a person
    /// quotes in a ticket.
    pub id: String,
    pub label: String,
    pub group: CapabilityGroup,
    pub state: CapabilityState,
    /// The variables that decide it, named the way a refusal names them, and
    /// in the order the refusal names them in.
    pub variables: Vec<String>,
    /// What it is set to, where saying so gives nothing away. Empty where the
    /// value is a credential or an address, and empty where there is nothing
    /// to set.
    pub settings: Vec<Setting>,
    /// What this deployment does without it — one sentence, and the reason the
    /// absent case is worth reading rather than worth fixing.
    pub note: String,
}

struct Entry {
    id: &'static str,
    label: &'static str,
    group: CapabilityGroup,
    state: CapabilityState,
    variables: &'static [&'static str],
    settings: Vec<Setting>,
    note: &'static str,
}

impl From<Entry> for Capability {
    fn from(entry: Entry) -> Self {
        Self {
            id: entry.id.to_owned(),
            label: entry.label.to_owned(),
            group: entry.group,
            state: entry.state,
            variables: entry
                .variables
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
            settings: entry.settings,
            note: entry.note.to_owned(),
        }
    }
}

/// What this instance is: a version, and a capability at a time.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
#[schema(as = SystemInventory)]
pub struct Inventory {
    /// The build serving this. The same version the OpenAPI document carries,
    /// so an operator reading a contract and an operator reading this are
    /// talking about one thing.
    pub version: String,
    /// Every capability, grouped and ordered as this module lists them. A
    /// deployment's whole answer arrives at once: paging an inventory of two
    /// dozen rows would be a cursor over a list that cannot grow with data.
    pub capabilities: Vec<Capability>,
}

// ── The route ────────────────────────────────────────────────────────────────

/// What this deployment has configured, and which variable decides each.
///
/// `admin`, because this is an inventory of the deployment rather than help
/// for whoever is reading a run.
#[utoipa::path(
    get,
    path = "/api/v1/system",
    responses(
        (status = 200, body = Inventory),
        (status = 401, description = "Not signed in"),
        (status = 403, body = crate::error::ErrorBody, description = "Reading this needs the admin role"),
    ),
    tag = "system",
)]
async fn system(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<Inventory>> {
    caller.require(Role::Admin)?;
    Ok(Json(inventory(&state)))
}

/// The inventory, read off the state and nothing else.
///
/// A function rather than a block inside the handler so a test can hold it to
/// a state it built, which is the only way to check that a capability's state
/// really follows its field rather than following a constant.
fn inventory(state: &AppState) -> Inventory {
    let mut capabilities = Vec::new();
    capabilities.extend(identity(state));
    capabilities.extend(observability(state));
    capabilities.extend(storage(state));
    capabilities.extend(runtime(state));
    capabilities.extend(integrations(state));
    Inventory {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        capabilities: capabilities.into_iter().map(Capability::from).collect(),
    }
}

fn identity(state: &AppState) -> Vec<Entry> {
    // The mode, the provider's name and the issuer, from the one place that
    // already answers them publicly — `GET /api/v1/auth/config`, which a
    // browser reads before anybody has signed in. Saying them again here costs
    // nothing and reading them from a second place would be a second answer.
    let auth = state
        .auth
        .as_ref()
        .map_or_else(aiwatcher_auth::Authenticator::disabled_config, |auth| {
            auth.public_config()
        });
    let mut settings = vec![Setting::read(
        "mode",
        auth.mode.as_str(),
        "AIWATCHER_AUTH_MODE",
    )];
    if !auth.provider.is_empty() {
        settings.push(Setting::read(
            "provider",
            auth.provider.clone(),
            "AIWATCHER_AUTH_PROVIDER_NAME",
        ));
    }
    // The issuer is an address, and it is the one address here that is said in
    // full: "which authentik is this pointing at" is unanswerable without a
    // shell on the pod otherwise, it is already public on `/auth/config`
    // before anybody signs in, and a browser is redirected to it in the open.
    if let Some(issuer) = &auth.issuer {
        settings.push(Setting::read(
            "issuer",
            issuer.clone(),
            "AIWATCHER_AUTH_ISSUER",
        ));
    }

    vec![
        Entry {
            id: "authentication",
            label: "Signing in",
            group: CapabilityGroup::Identity,
            state: CapabilityState::of(state.auth.is_some()),
            variables: &["AIWATCHER_AUTH_MODE"],
            settings,
            note: "Not configured is AIWATCHER_AUTH_MODE=none: every caller is anonymous and \
                   every role check passes, which is a local mode rather than a way to run this \
                   for more than one team.",
        },
        Entry {
            id: "iam-control-plane",
            label: "Organizations, teams and grants",
            group: CapabilityGroup::Identity,
            state: CapabilityState::of(state.iam.is_some()),
            // Never the connection string. Its presence is the fact; its
            // content is a host, a database and often a password.
            variables: &["AIWATCHER_IAM_POSTGRES_URL", "AIWATCHER_AUTH_MODE"],
            settings: Vec::new(),
            note: "Its own database, never the workflow store's, and it needs \
                   AIWATCHER_AUTH_MODE=oidc. Without it there are no projects and every route \
                   under /api/v1/iam answers 501.",
        },
        Entry {
            id: "account-enrolment",
            label: "Opening an account from an invitation",
            group: CapabilityGroup::Identity,
            state: CapabilityState::of(state.provisioning.is_some()),
            // The variable, never the address: an identity provider's admin
            // API is reconnaissance for somebody already inside, and the token
            // beside it is a credential.
            variables: &[
                "AIWATCHER_AUTH_PROVISION_URL",
                "AIWATCHER_AUTH_PROVISION_TOKEN",
                "AIWATCHER_AUTH_PROVISION_FLOW",
            ],
            settings: Vec::new(),
            note: "Lets somebody holding an invitation and no account make one at the identity \
                   provider. Without it an invitation still works for whoever already has an \
                   account, and the route answers 501.",
        },
    ]
}

fn observability(state: &AppState) -> Vec<Entry> {
    let prices = state.model_prices.as_ref().map_or_else(Vec::new, |table| {
        vec![
            Setting::derived("currency", table.currency.clone()),
            Setting::derived("entries", table.prices.len().to_string()),
        ]
    });

    vec![
        Entry {
            id: "event-ingest",
            label: "Events over HTTP",
            group: CapabilityGroup::Observability,
            state: CapabilityState::of(state.sink.is_some()),
            variables: &["AIWATCHER_INGEST_ENABLED"],
            settings: Vec::new(),
            note: "The way in for a producer that cannot reach the log directly. A deployment \
                   whose producers all publish to the broker leaves it off rather than exposing \
                   a second write path.",
        },
        Entry {
            id: "observation-periods",
            label: "Observed periods",
            group: CapabilityGroup::Observability,
            state: CapabilityState::of(state.observations.is_some()),
            variables: &[
                "AIWATCHER_PROMPT_STORE",
                "AIWATCHER_OBSERVATION_PERIOD_SECONDS",
            ],
            settings: Vec::new(),
            note: "The fold that answers a window over what a variant was observed doing, past \
                   the log's retention. Without an object store to write periods to, a window is \
                   the read model's and reaches back only as far as it holds.",
        },
        Entry {
            id: "asked-index",
            label: "Index of questions asked",
            group: CapabilityGroup::Observability,
            state: CapabilityState::of(state.asked.is_some()),
            variables: &["AIWATCHER_PROMPT_STORE", "AIWATCHER_ASKED_INDEX_DAYS"],
            settings: Vec::new(),
            note: "What witnesses saw asked, kept past the read model and past a restart. \
                   Without it a traces step reads the read model instead.",
        },
        Entry {
            id: "model-prices",
            label: "Model price table",
            group: CapabilityGroup::Observability,
            state: CapabilityState::of(state.model_prices.is_some()),
            variables: &["AIWATCHER_MODEL_PRICES"],
            settings: prices,
            note: "A call no entry covers is counted unpriced, never priced at nought, and \
                   nothing here fetches a price.",
        },
        Entry {
            id: "witnesses",
            label: "Named witnesses",
            group: CapabilityGroup::Observability,
            state: CapabilityState::of(!state.witnesses.names().is_empty()),
            // The count, never the names: a witness is named by the label of
            // an ingest credential, and the set of credentials this deployment
            // issued is not a list to hand out.
            variables: &["AIWATCHER_WITNESSES", "AIWATCHER_WITNESS_DIGESTS"],
            settings: vec![Setting::derived(
                "named",
                state.witnesses.names().len().to_string(),
            )],
            note: "Where none is named, any credential other than the answer's own witnesses it. \
                   Where some are, only those do.",
        },
    ]
}

fn storage(state: &AppState) -> Vec<Entry> {
    // One variable decides six of these, which is the fact worth reading: a
    // deployment either has somewhere to put what outlives a run or it has
    // none. They are still six rows, because six different areas answer 501
    // and somebody looking for one of them should find it by its own name.
    const OBJECT_STORE: &[&str] = &["AIWATCHER_PROMPT_STORE"];

    let sources = vec![
        Setting::derived("corpora", state.sources.sources.len().to_string()),
        Setting::derived("directories", state.sources.directories.len().to_string()),
    ];

    vec![
        Entry {
            id: "prompt-registry",
            label: "Prompt registry",
            group: CapabilityGroup::Storage,
            state: CapabilityState::of(state.prompts.is_some()),
            variables: OBJECT_STORE,
            settings: Vec::new(),
            note: "Authored rather than observed, so a version outlives the runs that used it. \
                   Its endpoint, bucket and keys are configuration this route does not report.",
        },
        Entry {
            id: "dataset-registry",
            label: "Dataset registry",
            group: CapabilityGroup::Storage,
            state: CapabilityState::of(state.datasets.is_some()),
            variables: OBJECT_STORE,
            settings: Vec::new(),
            note: "Saved curation recipes, pipelines and the dataset versions they publish. The \
                   same store as the prompt registry, under its own prefix.",
        },
        Entry {
            id: "annotation-registry",
            label: "Annotation registry",
            group: CapabilityGroup::Storage,
            state: CapabilityState::of(state.annotations.is_some()),
            variables: OBJECT_STORE,
            settings: Vec::new(),
            note: "Drawings, their reviews and the training exports built from them. Ships no \
                   vocabulary: the project's label schema carries the domain.",
        },
        Entry {
            id: "training-registry",
            label: "Training runs and models",
            group: CapabilityGroup::Storage,
            state: CapabilityState::of(state.training.is_some()),
            variables: OBJECT_STORE,
            settings: Vec::new(),
            note: "The one registry whose contents never came from the event log at all.",
        },
        Entry {
            id: "evaluation-registry",
            label: "Evaluation evidence",
            group: CapabilityGroup::Storage,
            state: CapabilityState::of(state.evaluations.is_some()),
            variables: OBJECT_STORE,
            settings: Vec::new(),
            note: "Pinned declarations, approvals and published results — durable, and \
                   independent of the log's fold of evaluation reports.",
        },
        Entry {
            id: "evaluation-bundles",
            label: "Staging a bundle's bytes",
            group: CapabilityGroup::Storage,
            state: CapabilityState::of(state.evaluation_bundles.is_some()),
            variables: &["AIWATCHER_PROMPT_STORE", "AIWATCHER_EVALUATION_SOURCE_DIR"],
            settings: Vec::new(),
            note: "Where an operator puts the files a pair is admitted by, when they arrive over \
                   the API rather than on a host's disk.",
        },
        Entry {
            id: "conversation-archive",
            label: "Conversation archive",
            group: CapabilityGroup::Storage,
            state: CapabilityState::of(state.conversations.is_some()),
            variables: &[
                "AIWATCHER_CONVERSATION_ARCHIVE",
                "AIWATCHER_CONVERSATION_KEYS",
            ],
            // The key *ids* are on `/api/v1/conversation-policy`, which is
            // where an operator checking a rotation looks; repeating them on
            // an inventory would put a second answer beside the first.
            settings: Vec::new(),
            note: "The one authored store that is off by default, and the only state here whose \
                   absence is a decision rather than an omission: content is sealed, retained on \
                   its own clock, and erasable by subject.",
        },
        Entry {
            id: "dataset-sources",
            label: "Curated corpus licences",
            group: CapabilityGroup::Storage,
            state: CapabilityState::of(
                !state.sources.sources.is_empty() || !state.sources.directories.is_empty(),
            ),
            variables: &["AIWATCHER_DATASET_SOURCES"],
            settings: sources,
            note: "Empty is a working state rather than a disabled one: nothing matches, every \
                   hub result stays licence-unclear, and an import records unknown rights.",
        },
        Entry {
            id: "attempt-artifacts",
            label: "What an attempt produced",
            group: CapabilityGroup::Storage,
            state: CapabilityState::of(state.artifacts.is_some() && state.catalog.is_some()),
            variables: OBJECT_STORE,
            settings: Vec::new(),
            note: "The bytes a worker reads and writes, and the index a cache hit is looked up \
                   in. Without them a step that takes its parameters and returns a bounded value \
                   still runs; one that produces an artifact does not.",
        },
    ]
}

fn runtime(state: &AppState) -> Vec<Entry> {
    // What the adapter behind the execution store can do, rather than which
    // adapter it is. It is the more useful half — `file` and `duckdb` hold one
    // process, which is why a run needing a worker is refused on them — and it
    // is the half that is not a host, a database name or a password.
    let store = state.executions.as_ref().map_or_else(Vec::new, |handler| {
        let capabilities = handler.store().capabilities();
        vec![
            Setting::derived("more than one process", yes_no(capabilities.multi_process)),
            Setting::derived("attempts can be claimed", yes_no(capabilities.claimable)),
        ]
    });

    let mut pods = vec![Setting::read(
        "pod runtime",
        state.pod_runtime.as_str(),
        "AIWATCHER_POD_RUNTIME",
    )];
    // The names, never the templates. A name is a Kubernetes label by
    // construction; the template beside it carries the operator's own pod
    // fragment, which is where `envFrom` and a service account live.
    if let Some(templates) = &state.pod_templates {
        pods.extend(
            templates
                .names()
                .map(|name| Setting::read("template", name, "AIWATCHER_POD_TEMPLATES")),
        );
    }

    let mut query = vec![Setting::read(
        "engine",
        state.query_engine.as_str(),
        "AIWATCHER_QUERY_ENGINE",
    )];
    if let Some(seconds) = state.query_step_timeout_seconds {
        query.push(Setting::read(
            "step timeout",
            format!("{seconds}s"),
            "AIWATCHER_QUERY_STEP_TIMEOUT_SECONDS",
        ));
    }

    let payloads = vec![
        Setting::read(
            "policy",
            state.execution_payloads.policy.as_str(),
            "AIWATCHER_EXECUTION_PAYLOADS",
        ),
        Setting::read(
            "a run may choose its own",
            yes_no(!state.execution_payloads.locked),
            "AIWATCHER_EXECUTION_PAYLOADS_LOCKED",
        ),
    ];

    let answers = vec![
        Setting::read(
            "answers per step",
            state
                .answer_limits
                .per_step
                .map_or_else(|| "no ceiling".to_owned(), |limit| limit.to_string()),
            "AIWATCHER_MAX_ANSWERS_PER_STEP",
        ),
        Setting::read(
            "bytes per answer",
            state.answer_limits.bytes.to_string(),
            "AIWATCHER_MAX_ANSWER_BYTES",
        ),
    ];

    vec![
        Entry {
            id: "execution-store",
            label: "Managed execution",
            group: CapabilityGroup::Runtime,
            state: CapabilityState::of(state.executions.is_some()),
            variables: &["AIWATCHER_WORKFLOW_STORE"],
            settings: store,
            note: "The transactional record of what this system decided, and the one store here \
                   that is not the log or the object store. Where it holds one process, a run \
                   needing a worker is refused at start rather than left to hang.",
        },
        Entry {
            id: "workflow-definitions",
            label: "Authored workflows",
            group: CapabilityGroup::Runtime,
            state: CapabilityState::of(state.workflow_definitions.is_some()),
            variables: &["AIWATCHER_PROMPT_STORE"],
            settings: Vec::new(),
            note: "Versioned outside execution retention, in the object store the authored \
                   registries share.",
        },
        Entry {
            id: "schedules",
            label: "Unattended runs",
            group: CapabilityGroup::Runtime,
            state: CapabilityState::of(state.schedules.is_some()),
            variables: &["AIWATCHER_PROMPT_STORE"],
            settings: Vec::new(),
            note: "When a definition runs with nobody watching. The same object store, under its \
                   own prefix; without one there are no definitions to schedule either.",
        },
        Entry {
            id: "pods",
            label: "A pod per step",
            group: CapabilityGroup::Runtime,
            state: CapabilityState::of(state.pod_templates.is_some()),
            variables: &["AIWATCHER_POD_TEMPLATES", "AIWATCHER_POD_RUNTIME"],
            settings: pods,
            note: "A step names a template and an image on that template's list. With no \
                   templates, a step asking for a pod is refused at registration naming the \
                   variable; the runtime says what a pod is here, and is read whether or not any \
                   template is loaded.",
        },
        Entry {
            id: "query-engine",
            label: "Query engine",
            group: CapabilityGroup::Runtime,
            // The one row that is always configured, because a deployment
            // always has an engine: unset is `flow`, which is what everything
            // written before the setting existed is read as. What this says is
            // which engine a plan has to have been written for — whether a
            // service answers it is AIWATCHER_QUERY_URL on the work role,
            // which this process does not hold.
            state: CapabilityState::Configured,
            variables: &[
                "AIWATCHER_QUERY_ENGINE",
                "AIWATCHER_QUERY_STEP_TIMEOUT_SECONDS",
            ],
            settings: query,
            note: "One per deployment. A plan written for another engine is refused when it is \
                   started, naming the block and both engines.",
        },
        Entry {
            id: "sealed-payloads",
            label: "Where a hosted run's words go",
            group: CapabilityGroup::Runtime,
            state: CapabilityState::of(
                state.execution_payloads.policy == aiwatcher_execution::PayloadPolicy::Sealed,
            ),
            variables: &[
                "AIWATCHER_EXECUTION_PAYLOADS",
                "AIWATCHER_EXECUTION_PAYLOADS_LOCKED",
            ],
            settings: payloads,
            note: "The default is the visible one: words stay with the worker and this instance \
                   holds a reference. Sealing them puts them through the conversation archive's \
                   crypt, on its clock and erasable by subject.",
        },
        Entry {
            id: "answer-limits",
            label: "What an answer may be",
            group: CapabilityGroup::Runtime,
            // Always configured for the same reason the engine is: there is
            // always a limit, and a deployment saying "no ceiling" said so.
            state: CapabilityState::Configured,
            variables: &[
                "AIWATCHER_MAX_ANSWERS_PER_STEP",
                "AIWATCHER_MAX_ANSWER_BYTES",
            ],
            settings: answers,
            note: "An answer is a decision rather than data. The ceiling stops a task looping on \
                   its own question before the store refuses an oversized message and names the \
                   wrong thing.",
        },
    ]
}

fn integrations(state: &AppState) -> Vec<Entry> {
    let mut judge = Vec::new();
    if let Some(provider) = &state.judge_provider {
        judge.push(Setting::read(
            "profile",
            provider.clone(),
            "AIWATCHER_JUDGE_PROVIDER",
        ));
        judge.push(Setting::read(
            "questions at once",
            state.judge_concurrency.to_string(),
            "AIWATCHER_JUDGE_CONCURRENCY",
        ));
    }

    let scorers = state
        .scorer_concurrency
        .map(|at_once| {
            vec![Setting::read(
                "cases at once",
                at_once.to_string(),
                "AIWATCHER_SCORER_CONCURRENCY",
            )]
        })
        .unwrap_or_default();

    vec![
        Entry {
            id: "workflow-runner",
            label: "Rerunning somebody else's workflow",
            group: CapabilityGroup::Integration,
            state: CapabilityState::of(state.runner.is_some()),
            // Never the endpoint. It is configuration precisely so that a
            // `workflow.declared` naming its own callback cannot become a
            // request-forgery primitive, and printing it here would hand back
            // what that rule keeps off the log.
            variables: &["AIWATCHER_WORKFLOW_RUNNER", "AIWATCHER_WORKFLOW_RUNNER_URL"],
            settings: Vec::new(),
            note: "The one thing here that makes something happen rather than reporting that it \
                   did, so its absence is a 501 naming the variable and never a no-op that \
                   acknowledges a rerun nobody ran.",
        },
        Entry {
            id: "alert-channel",
            label: "Where an alert goes",
            group: CapabilityGroup::Integration,
            state: CapabilityState::of(state.alert_channel.is_some()),
            // Never the endpoint, and never the token or the signing secret.
            // The endpoint is configuration for the same reason the rerun's is
            // — so that nothing able to publish a rule can aim this process's
            // outbound POSTs — and printing it here would hand back what that
            // rule keeps out of a request body.
            variables: &[
                "AIWATCHER_ALERT_WEBHOOK_URL",
                "AIWATCHER_ALERT_WEBHOOK_SECRET",
                "AIWATCHER_ALERT_HISTORY_DAYS",
            ],
            settings: state
                .alert_channel
                .as_deref()
                .map(|channel| {
                    let channel = aiwatcher_alerts::AlertChannel::describe(channel);
                    vec![
                        Setting::derived("kind", channel.kind),
                        Setting::derived("signed", if channel.signed { "yes" } else { "no" }),
                    ]
                })
                .unwrap_or_default(),
            note: "Without it a rule is still kept and a regression is still found — what does \
                   not happen is anybody being told, which the route that tries says by name \
                   rather than answering as though it had sent something.",
        },
        Entry {
            id: "notebook-editor",
            label: "Notebook runtime",
            group: CapabilityGroup::Integration,
            state: CapabilityState::of(state.editor.is_some()),
            variables: &["AIWATCHER_ML_PIPELINE_URL", "AIWATCHER_PROMPT_STORE"],
            settings: Vec::new(),
            note: "Opening a block on a step's rows. Both an address and an object store are \
                   needed and either alone is useless, which is why one refusal names both.",
        },
        Entry {
            id: "judge",
            label: "Calibrated judge",
            group: CapabilityGroup::Integration,
            state: CapabilityState::of(state.judge_provider.is_some()),
            variables: &[
                "AIWATCHER_JUDGE_URL",
                "AIWATCHER_JUDGE_PROVIDER",
                "AIWATCHER_JUDGE_CONCURRENCY",
            ],
            settings: judge,
            note: "What a scorecard's rubric is asked. Declared rather than detected, and pinned \
                   in every result it measured; a judged run on a deployment with none is \
                   refused at start rather than left for nothing to claim.",
        },
        Entry {
            id: "scorer-service",
            label: "Scorer service",
            group: CapabilityGroup::Integration,
            state: CapabilityState::of(state.scorer_concurrency.is_some()),
            variables: &["AIWATCHER_SCORER_URL", "AIWATCHER_SCORER_CONCURRENCY"],
            settings: scorers,
            note: "A framework's own metrics — DeepEval's and Opik's — for a scorecard that names \
                   one. Its address and its bearer token are configuration this route does not \
                   report.",
        },
        Entry {
            id: "dataset-hubs",
            label: "Dataset hubs",
            group: CapabilityGroup::Integration,
            state: CapabilityState::of(state.hubs.is_some()),
            variables: &[
                "AIWATCHER_HUGGINGFACE_ENABLED",
                "AIWATCHER_KAGGLE_USERNAME",
                "AIWATCHER_KAGGLE_KEY",
            ],
            // Which hub is on, and why a search came back short, is
            // `/api/v1/dataset-hubs` — the route that asks them. This says
            // only whether anything here reaches out at all.
            settings: Vec::new(),
            note: "The only thing in this state that reaches a service aiwatcher does not run, \
                   for a question whose answer it deliberately refuses to trust: a hub says what \
                   exists, and the curated table says what is permitted.",
        },
    ]
}

/// A boolean as somebody reads it, in one place so two rows never disagree.
const fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}
