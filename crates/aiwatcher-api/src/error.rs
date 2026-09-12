//! One error type, one JSON shape.
//!
//! Every failure the API can return renders as the same object, so the
//! generated TypeScript client has one error type to handle rather than a
//! different shape per endpoint.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("evaluation: {0}")]
    Evaluation(#[from] aiwatcher_evaluation::EvaluationError),
    #[error("durable evaluations require an object store (AIWATCHER_PROMPT_STORE)")]
    EvaluationDisabled,
    #[error("{0} not found")]
    NotFound(String),

    #[error("invalid request: {0}")]
    BadRequest(String),

    #[error("the event log is unavailable: {0}")]
    LogUnavailable(String),

    #[error("ingest is not enabled on this instance")]
    IngestDisabled,

    #[error("this instance has no prompt registry configured (AIWATCHER_PROMPT_STORE)")]
    RegistryDisabled,

    #[error("this instance has no dataset registry configured (AIWATCHER_PROMPT_STORE)")]
    DatasetRegistryDisabled,

    #[error("this instance has no workflow definition registry (AIWATCHER_PROMPT_STORE)")]
    WorkflowDefinitionsDisabled,

    #[error("this instance has no annotation registry configured (AIWATCHER_PROMPT_STORE)")]
    AnnotationRegistryDisabled,

    #[error("this instance has no training registry configured (AIWATCHER_PROMPT_STORE)")]
    TrainingRegistryDisabled,

    /// The one registry whose absence is the *default*, and deliberately so:
    /// a deployment that has not decided how it governs conversation content
    /// must not be quietly holding any. See ADR_0021.
    #[error(
        "this instance keeps no conversation archive (AIWATCHER_CONVERSATION_ARCHIVE, \
         AIWATCHER_CONVERSATION_KEYS)"
    )]
    ConversationArchiveDisabled,

    #[error(
        "this instance searches no dataset hubs (AIWATCHER_HUGGINGFACE_ENABLED, AIWATCHER_KAGGLE_USERNAME/AIWATCHER_KAGGLE_KEY)"
    )]
    HubsDisabled,

    /// A configured hub answered badly, or cannot answer this question at all.
    ///
    /// Distinct from [`Self::HubsDisabled`] because the two send a reader to
    /// different places: that one is an environment variable somebody has to
    /// set, this one is a corpus, a credential or a hub having a bad morning.
    #[error("{0}")]
    HubUnreachable(String),

    #[error("this instance has no workflow runner configured (AIWATCHER_WORKFLOW_RUNNER)")]
    RunnerDisabled,

    /// No notebook runtime address, or no object store to read a step's rows
    /// from. Both are needed and either alone is useless, so one variant says
    /// so rather than two that a caller would have to tell apart.
    #[error(
        "this instance opens no notebook editor (AIWATCHER_ML_PIPELINE_URL, AIWATCHER_PROMPT_STORE)"
    )]
    EditorDisabled,

    /// The notebook runtime would not stage this step's rows. Same split as
    /// `Runner`: unreachable is a 503 worth repeating, and a refusal is a 502
    /// that will refuse identically forever.
    #[error("the notebook runtime would not open this editor: {0}")]
    Editor(aiwatcher_core::ports::PortError),

    /// A worker asked for the bytes of an attempt on an instance with no
    /// object store. Claiming and settling still work — a task that takes its
    /// parameters and returns a bounded value needs no artifact — so this is
    /// its own variant rather than a condition on the whole worker surface.
    #[error("this instance stores no attempt artifacts (AIWATCHER_PROMPT_STORE)")]
    WorkerArtifactsDisabled,

    /// The object store would not give up or take a worker's rows. Same split
    /// as `Editor`: unreachable is a 503 worth repeating, and a refusal is a
    /// 502 that will refuse identically forever.
    #[error("this attempt's artifacts could not be reached: {0}")]
    WorkerArtifacts(aiwatcher_core::ports::PortError),

    /// A read of what a run produced, on an instance with no object store.
    ///
    /// One variant rather than two, because the catalog that indexes a step's
    /// artifacts and the store that holds their bytes are present exactly
    /// together: both come from `AIWATCHER_PROMPT_STORE`, so their absence is
    /// one fact with one fix. Separate from `WorkerArtifactsDisabled` because
    /// the reader is a person on a step's view rather than a claimant, and a
    /// code naming a worker would send them looking at one.
    #[error("this instance keeps no step artifacts (AIWATCHER_PROMPT_STORE)")]
    StepArtifactsDisabled,

    /// A worker named an attempt it does not hold: the lease expired, somebody
    /// took it over, or it was never dispatched.
    ///
    /// A 409 rather than a 404 or a 403, and the difference is what the worker
    /// does next. Its work is discarded — that is the reactor's rule, that a
    /// claimant whose lease went must not write beside its replacement — and
    /// the right response is to go back to claiming rather than to retry this
    /// call or to re-authenticate.
    #[error("this worker no longer holds {0}")]
    LeaseLost(String),
    #[error("attempt {0} already has a different recorded outcome")]
    WorkerReportConflict(String),

    /// The one `Option` in [`AppState`](crate::state::AppState) that is never
    /// `None` in the server binary: an execution store needs no more
    /// configuration than a directory. What answers this is a router built
    /// without one — a test, and any embedder that wires the reads and not the
    /// writes.
    #[error("this instance has no workflow store configured (AIWATCHER_WORKFLOW_STORE)")]
    ExecutionsDisabled,

    /// A definition that does not compile to something this deployment can
    /// run: a chain that is not a chain, a notebook nobody pinned, or a step
    /// that needs a process the configured store cannot give it.
    ///
    /// Every reason at once, in `details`, for
    /// [`order_of`](aiwatcher_datasets::order_of)'s reason: somebody wiring a
    /// canvas fixes what they can see, and one problem per round trip teaches
    /// them to press the button again instead of reading it.
    #[error("{summary}")]
    PlanRefused {
        summary: String,
        problems: Vec<String>,
    },

    /// A question a worker stopped to ask that nobody could answer as asked:
    /// a blank prompt, a role a gate may not name, a policy with no deadline
    /// behind it.
    ///
    /// The same 422 with the same `details`, because it is the same rule set —
    /// `aiwatcher_core::human_input` owns what a valid question is, and a
    /// canvas block, a workflow step and a worker's park are three surfaces
    /// authoring one.
    #[error("this attempt's question was refused")]
    QuestionRefused { problems: Vec<String> },

    /// A command the execution's own state would not accept.
    #[error(transparent)]
    Execution(#[from] aiwatcher_execution::HandleError),

    /// A worker's append to a hosted execution's history, refused.
    #[error(transparent)]
    HostedAppend(#[from] aiwatcher_execution::hosted::HostedError),

    #[error("this instance has no identity provider configured (AIWATCHER_AUTH_MODE)")]
    AuthDisabled,

    #[error("authentication is required")]
    Unauthenticated,

    /// Authenticated, and not allowed to do this. Names the role required and
    /// the one held, because the fix is a group membership in the identity
    /// provider and "forbidden" alone does not say which one.
    #[error("this needs the {needed} role; you have {held}")]
    Forbidden {
        needed: aiwatcher_auth::Role,
        held: aiwatcher_auth::Role,
    },

    #[error(transparent)]
    Auth(#[from] aiwatcher_auth::AuthError),

    #[error("{what} is too large: {size} bytes, over the {limit} byte limit")]
    TooLarge {
        what: &'static str,
        size: usize,
        limit: usize,
    },

    #[error(transparent)]
    Registry(#[from] aiwatcher_prompts::RegistryError),

    #[error(transparent)]
    DatasetRegistry(#[from] aiwatcher_datasets::RegistryError),

    /// The other registry a managed run may be compiled from. No `#[from]`:
    /// a definition that does not compile becomes a 422 carrying every
    /// problem, which is [`ApiError::PlanRefused`] rather than this.
    #[error(transparent)]
    WorkflowDefinitions(aiwatcher_execution::DefinitionError),

    #[error(transparent)]
    AnnotationRegistry(#[from] aiwatcher_annotations::Error),

    #[error(transparent)]
    TrainingRegistry(#[from] aiwatcher_training::Error),

    #[error(transparent)]
    ConversationArchive(#[from] aiwatcher_conversations::Error),

    /// A rerun the orchestrator would not take. Distinct from every other
    /// variant here in one way that matters: it is the only failure that is
    /// about work aiwatcher asked somebody else to do.
    #[error("the workflow runner refused the rerun: {0}")]
    Runner(aiwatcher_core::ports::PortError),

    #[error(transparent)]
    Bus(#[from] aiwatcher_bus::BusError),

    #[error(transparent)]
    Core(#[from] aiwatcher_core::CoreError),
}

/// The body every error response carries.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ErrorBody {
    /// Stable machine-readable discriminator. Switch on this, not on `message`.
    pub code: &'static str,
    pub message: String,
    /// One line per problem, where a request can fail in more than one way at
    /// once. An annotation is the case that needs it: a labeller fixing one
    /// error per round trip stops using the tool, so every problem in a
    /// drawing is reported together. Absent everywhere else, which keeps the
    /// one JSON shape one shape.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<String>,
}

impl ApiError {
    /// The status a caller would have been given.
    ///
    /// Public because the scheduler has to tell a refusal it will get again
    /// from one it may not: a definition that stopped compiling is a 4xx and
    /// says the same thing every tick, while an unreachable store is a 5xx and
    /// may not. Reading that off the status is reading the classification this
    /// module already made — the alternative was matching on prose.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        self.parts().0
    }

    fn parts(&self) -> (StatusCode, &'static str) {
        match self {
            Self::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            Self::BadRequest(_) | Self::Core(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            Self::IngestDisabled => (StatusCode::FORBIDDEN, "ingest_disabled"),
            // 501, not 403: the endpoint exists in the contract and this
            // deployment did not wire a store behind it. A client can tell
            // "you may not" from "nobody can here", which is the difference
            // between a permission problem and a configuration one.
            Self::RegistryDisabled
            | Self::DatasetRegistryDisabled
            | Self::WorkflowDefinitionsDisabled
            | Self::AnnotationRegistryDisabled
            | Self::EvaluationDisabled
            | Self::TrainingRegistryDisabled
            | Self::ConversationArchiveDisabled => {
                (StatusCode::NOT_IMPLEMENTED, "registry_disabled")
            }
            // Same shape once more, with the sharpest reason of the set: an
            // empty search result would read as "there is no such corpus",
            // which is a fact about the world rather than about this
            // deployment's configuration.
            Self::HubsDisabled => (StatusCode::NOT_IMPLEMENTED, "hubs_disabled"),
            // The hub is configured and did not deliver. A 502 rather than an
            // empty page: "this corpus has no images" and "Hugging Face
            // answered 404" are different answers, and only one of them is
            // about the corpus.
            Self::HubUnreachable(_) => (StatusCode::BAD_GATEWAY, "hub_unreachable"),
            // Same reasoning, and the message names the variable to set. A
            // null runner that answered 202 would be worse than this: it would
            // report success for a rerun that never happened.
            Self::RunnerDisabled => (StatusCode::NOT_IMPLEMENTED, "runner_disabled"),
            Self::EditorDisabled => (StatusCode::NOT_IMPLEMENTED, "editor_disabled"),
            Self::Editor(error) => match error {
                aiwatcher_core::ports::PortError::Rejected { .. } => {
                    (StatusCode::BAD_GATEWAY, "editor_refused")
                }
                _ => (StatusCode::SERVICE_UNAVAILABLE, "editor_unavailable"),
            },
            Self::WorkerArtifactsDisabled => {
                (StatusCode::NOT_IMPLEMENTED, "worker_artifacts_disabled")
            }
            Self::StepArtifactsDisabled => (StatusCode::NOT_IMPLEMENTED, "step_artifacts_disabled"),
            Self::WorkerArtifacts(error) => match error {
                aiwatcher_core::ports::PortError::Rejected { .. } => {
                    (StatusCode::BAD_GATEWAY, "worker_artifacts_refused")
                }
                _ => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "worker_artifacts_unavailable",
                ),
            },
            Self::LeaseLost(_) => (StatusCode::CONFLICT, "lease_lost"),
            Self::WorkerReportConflict(_) => (StatusCode::CONFLICT, "worker_report_conflict"),
            Self::ExecutionsDisabled => (StatusCode::NOT_IMPLEMENTED, "executions_disabled"),
            // The same 422 a refused pipeline gets, for the same reason: the
            // request was well formed and the thing it describes cannot be
            // run. Every problem with it rides in `details`.
            Self::PlanRefused { .. } => (StatusCode::UNPROCESSABLE_ENTITY, "plan_refused"),
            // And once more, for the third surface that authors a question.
            Self::QuestionRefused { .. } => (StatusCode::UNPROCESSABLE_ENTITY, "question_refused"),
            Self::Execution(error) => execution_parts(error),
            Self::HostedAppend(error) => hosted_parts(error),
            // Same shape again, and the same reason: the sign-in routes exist
            // in the contract and this deployment configured no provider.
            Self::AuthDisabled => (StatusCode::NOT_IMPLEMENTED, "auth_disabled"),
            Self::Unauthenticated => (StatusCode::UNAUTHORIZED, "unauthenticated"),
            Self::Forbidden { .. } => (StatusCode::FORBIDDEN, "forbidden"),
            Self::Auth(error) => auth_parts(error),
            Self::TooLarge { .. } => (StatusCode::PAYLOAD_TOO_LARGE, "too_large"),
            Self::LogUnavailable(_) => (StatusCode::SERVICE_UNAVAILABLE, "log_unavailable"),
            Self::Bus(error) if error.is_retryable() => {
                (StatusCode::SERVICE_UNAVAILABLE, "log_unavailable")
            }
            Self::Bus(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            Self::Registry(error) => registry_parts(error),
            Self::DatasetRegistry(error) => dataset_registry_parts(error),
            Self::WorkflowDefinitions(error) => definition_registry_parts(error),
            Self::AnnotationRegistry(error) => annotation_registry_parts(error),
            Self::Evaluation(error) => match error {
                aiwatcher_evaluation::EvaluationError::Invalid { .. } => {
                    (StatusCode::BAD_REQUEST, "invalid_evaluation")
                }
                aiwatcher_evaluation::EvaluationError::Conflict => {
                    (StatusCode::CONFLICT, "evaluation_conflict")
                }
                aiwatcher_evaluation::EvaluationError::Contested => {
                    (StatusCode::CONFLICT, "assessment_contested")
                }
                aiwatcher_evaluation::EvaluationError::Unavailable(
                    aiwatcher_evaluation::EvidenceState::Forbidden,
                ) => (StatusCode::FORBIDDEN, "evidence_forbidden"),
                aiwatcher_evaluation::EvaluationError::Unavailable(
                    aiwatcher_evaluation::EvidenceState::Expired
                    | aiwatcher_evaluation::EvidenceState::DeletedSource,
                ) => (StatusCode::GONE, "evidence_gone"),
                _ => (StatusCode::SERVICE_UNAVAILABLE, "evidence_unavailable"),
            },
            Self::TrainingRegistry(error) => training_registry_parts(error),
            Self::ConversationArchive(error) => conversation_archive_parts(error),
            // The same retryable/not split the registry makes, for the same
            // reason: an orchestrator that is down is a 503 worth repeating,
            // and one that refused the request is a 502 that will refuse it
            // identically forever.
            Self::Runner(error) if error.is_retryable() => {
                (StatusCode::SERVICE_UNAVAILABLE, "runner_unavailable")
            }
            Self::Runner(_) => (StatusCode::BAD_GATEWAY, "runner_rejected"),
        }
    }
}

/// A refused start, as the error shape every route already answers with.
///
/// The use case's refusal carries *why*, and this is the one place that turns
/// each reason into a status. The scheduler reads the same refusal and asks it
/// [`says_the_same_next_time`](aiwatcher_execution::StartRefused::says_the_same_next_time)
/// instead — which is the whole point of the split, because the status is a
/// lossy encoding of that question: 502 and 500 are 5xx by number and
/// permanent by meaning.
impl From<aiwatcher_execution::StartRefused> for ApiError {
    fn from(refused: aiwatcher_execution::StartRefused) -> Self {
        use aiwatcher_execution::{Missing, StartRefused};
        match refused {
            StartRefused::NotConfigured(Missing::WorkflowStore) => Self::ExecutionsDisabled,
            StartRefused::NotConfigured(Missing::PipelineRegistry) => Self::DatasetRegistryDisabled,
            StartRefused::NotConfigured(Missing::WorkflowRegistry) => {
                Self::WorkflowDefinitionsDisabled
            }
            StartRefused::Unknown(what) => Self::NotFound(what),
            StartRefused::Invalid(why) => Self::BadRequest(why),
            StartRefused::Refused { summary, problems } => Self::PlanRefused { summary, problems },
            StartRefused::Pipelines(error) => Self::DatasetRegistry(error),
            StartRefused::Definitions(error) => error.into(),
            StartRefused::Command(error) => Self::Execution(error),
        }
    }
}

/// A refused read of an authored workflow definition, as a status.
///
/// The same three answers the dataset registry already gives, because it is
/// the same question: a store that could not be reached is a 503 worth
/// repeating, one that understood the read and refused it is a 502 that will
/// refuse it identically, and an object that will not read back is a 500 that
/// no amount of waiting fixes. It was one 503 for all three — a promise that
/// a corrupt definition would come back.
impl From<aiwatcher_execution::DefinitionError> for ApiError {
    fn from(error: aiwatcher_execution::DefinitionError) -> Self {
        use aiwatcher_execution::DefinitionError;
        match error {
            // Every problem at once, the way the route in front of the store
            // already answers: a summary and `details`, never one sentence
            // with semicolons in it.
            DefinitionError::Refused(problems) => Self::PlanRefused {
                summary: "workflow definition is invalid".to_owned(),
                problems,
            },
            other => Self::WorkflowDefinitions(other),
        }
    }
}

/// An authentication failure, as a status the caller can act on.
///
/// Three outcomes, and which one it is decides what the panel does: 401 means
/// sign in, 403 means ask an administrator for a group, and 5xx means the
/// identity provider is the problem and signing in again will not help.
fn auth_parts(error: &aiwatcher_auth::AuthError) -> (StatusCode, &'static str) {
    use aiwatcher_auth::AuthError;
    match error {
        // Authenticated by the provider and granted nothing here. A 401 would
        // send the panel back to a sign-in that would succeed and land in the
        // same place, which is the loop this distinction exists to avoid.
        AuthError::NotEntitled(_) => (StatusCode::FORBIDDEN, "forbidden"),
        error if error.is_retryable() => (
            StatusCode::SERVICE_UNAVAILABLE,
            "identity_provider_unavailable",
        ),
        error if error.is_caller_fault() => (StatusCode::UNAUTHORIZED, "unauthenticated"),
        // A misconfiguration reaching a request: the instance started, and
        // something about the provider does not work. Not the caller's problem
        // and not fixable by retrying.
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "auth_unavailable"),
    }
}

/// A registry failure, as a status the caller can act on.
///
/// The distinction that matters is retryable against not: an unreachable
/// object store is a 503 the client should come back from, and a rejected
/// request is a 4xx that will be rejected identically forever.
fn registry_parts(error: &aiwatcher_prompts::RegistryError) -> (StatusCode, &'static str) {
    use aiwatcher_prompts::RegistryError;
    match error {
        RegistryError::UnknownPrompt(_)
        | RegistryError::UnknownVersion { .. }
        | RegistryError::UnknownOptimization { .. } => (StatusCode::NOT_FOUND, "not_found"),
        RegistryError::Invalid(_) | RegistryError::InvalidIdentifier { .. } => {
            (StatusCode::BAD_REQUEST, "bad_request")
        }
        RegistryError::TooLarge { .. } => (StatusCode::PAYLOAD_TOO_LARGE, "too_large"),
        // The model registry's answer to the same act, for the same reason:
        // a refused promotion is a decision about content, not a conflict.
        RegistryError::NotAdmitted { .. } => {
            (StatusCode::UNPROCESSABLE_ENTITY, "promotion_refused")
        }
        RegistryError::Store(store) if store.is_retryable() => {
            (StatusCode::SERVICE_UNAVAILABLE, "registry_unavailable")
        }
        RegistryError::Store(_) => (StatusCode::BAD_GATEWAY, "registry_rejected"),
        // A stored object that will not parse is this system's fault, not the
        // caller's, and it is not going to fix itself on a retry.
        RegistryError::Corrupt { .. } | RegistryError::Integrity { .. } => {
            (StatusCode::INTERNAL_SERVER_ERROR, "registry_corrupt")
        }
    }
}

/// An annotation failure, as a status the caller can act on.
///
/// One extra outcome over the other two registries: a drawing that did not
/// validate is a 422 rather than a 400, because the request was well formed and
/// the *content* was refused. The panel keeps the shape on the canvas and
/// renders the reasons beside it; a 400 would read as "the tool is broken".
fn annotation_registry_parts(error: &aiwatcher_annotations::Error) -> (StatusCode, &'static str) {
    use aiwatcher_annotations::Error;
    match error {
        Error::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
        Error::Invalid(_) => (StatusCode::BAD_REQUEST, "bad_request"),
        Error::Rejected(_) => (StatusCode::UNPROCESSABLE_ENTITY, "annotation_rejected"),
        Error::TooLarge { .. } => (StatusCode::PAYLOAD_TOO_LARGE, "too_large"),
        Error::Store(store) if store.is_retryable() => {
            (StatusCode::SERVICE_UNAVAILABLE, "registry_unavailable")
        }
        Error::Store(_) => (StatusCode::BAD_GATEWAY, "registry_rejected"),
        Error::Corrupt { .. } => (StatusCode::INTERNAL_SERVER_ERROR, "registry_corrupt"),
    }
}

/// A training failure, as a status the caller can act on.
///
/// Two outcomes this registry has that the others do not. A run id that has
/// already finished is a 409: the request is well formed, and the state it
/// assumed is gone. A promotion the registry refused is a 422 with the reason,
/// because "this model has no held-out score" is a finding rather than a typo.
fn training_registry_parts(error: &aiwatcher_training::Error) -> (StatusCode, &'static str) {
    use aiwatcher_training::Error;
    match error {
        Error::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
        Error::Invalid(_) => (StatusCode::BAD_REQUEST, "bad_request"),
        // A closed run and a reused run id are conflicts with state, not bad
        // requests; a refused promotion is a decision about content.
        Error::Refused(message) if message.contains("run") => (StatusCode::CONFLICT, "run_closed"),
        Error::Refused(_) => (StatusCode::UNPROCESSABLE_ENTITY, "promotion_refused"),
        Error::TooLarge { .. } => (StatusCode::PAYLOAD_TOO_LARGE, "too_large"),
        Error::Store(store) if store.is_retryable() => {
            (StatusCode::SERVICE_UNAVAILABLE, "registry_unavailable")
        }
        Error::Store(_) => (StatusCode::BAD_GATEWAY, "registry_rejected"),
        Error::Corrupt { .. } => (StatusCode::INTERNAL_SERVER_ERROR, "registry_corrupt"),
    }
}

/// A conversation archive failure, as a status the caller can act on.
///
/// Two outcomes no other registry here has. Erased content is a **410**: the
/// turn was there, it is gone, and a 404 would make a completed erasure
/// indistinguishable from a turn that never existed — which is exactly the
/// distinction an auditor came for. And a key this deployment no longer holds
/// is a 500 rather than a 404, because it is a configuration problem in this
/// process: the content is intact and unreadable, and telling the caller it is
/// missing would send them to look for a backup that would not help.
fn conversation_archive_parts(
    error: &aiwatcher_conversations::Error,
) -> (StatusCode, &'static str) {
    use aiwatcher_conversations::Error;
    match error {
        Error::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
        Error::Erased(_, _) => (StatusCode::GONE, "erased"),
        Error::Invalid(_) => (StatusCode::BAD_REQUEST, "bad_request"),
        // The same 422 the annotation registry uses, and for the same reason:
        // the request was well formed and its *content* was refused, with every
        // problem at once rather than the first.
        Error::Rejected(_) => (StatusCode::UNPROCESSABLE_ENTITY, "turn_rejected"),
        Error::Refused(_) => (StatusCode::UNPROCESSABLE_ENTITY, "export_refused"),
        Error::TooLarge { .. } => (StatusCode::PAYLOAD_TOO_LARGE, "too_large"),
        Error::Crypto(_) => (StatusCode::INTERNAL_SERVER_ERROR, "archive_key_missing"),
        Error::Store(store) if store.is_retryable() => {
            (StatusCode::SERVICE_UNAVAILABLE, "registry_unavailable")
        }
        Error::Store(_) => (StatusCode::BAD_GATEWAY, "registry_rejected"),
        Error::Corrupt { .. } => (StatusCode::INTERNAL_SERVER_ERROR, "registry_corrupt"),
    }
}

fn dataset_registry_parts(error: &aiwatcher_datasets::RegistryError) -> (StatusCode, &'static str) {
    use aiwatcher_datasets::RegistryError;
    match error {
        RegistryError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
        RegistryError::Invalid(_) => (StatusCode::BAD_REQUEST, "bad_request"),
        // The same 422 the annotation registry uses, and for the same reason:
        // the request was well formed and the thing it describes cannot be
        // run. Every problem with it rides in `details`.
        RegistryError::Rejected(_) => (StatusCode::UNPROCESSABLE_ENTITY, "pipeline_rejected"),
        RegistryError::TooLarge { .. } => (StatusCode::PAYLOAD_TOO_LARGE, "too_large"),
        RegistryError::Store(store) if store.is_retryable() => {
            (StatusCode::SERVICE_UNAVAILABLE, "registry_unavailable")
        }
        RegistryError::Store(_) => (StatusCode::BAD_GATEWAY, "registry_rejected"),
        RegistryError::Corrupt { .. } => (StatusCode::INTERNAL_SERVER_ERROR, "registry_corrupt"),
    }
}

/// A refused definition read, as a status — the dataset registry's three,
/// for the registry a workflow is compiled from.
fn definition_registry_parts(
    error: &aiwatcher_execution::DefinitionError,
) -> (StatusCode, &'static str) {
    use aiwatcher_execution::DefinitionError;
    match error {
        DefinitionError::Store(port) if port.is_retryable() => {
            (StatusCode::SERVICE_UNAVAILABLE, "registry_unavailable")
        }
        DefinitionError::Store(_) => (StatusCode::BAD_GATEWAY, "registry_rejected"),
        DefinitionError::Corrupt { .. } => (StatusCode::INTERNAL_SERVER_ERROR, "registry_corrupt"),
        // Reached only by constructing the variant directly: `From` turns a
        // refusal into the 422 that carries its problems.
        DefinitionError::Refused(_) => (StatusCode::UNPROCESSABLE_ENTITY, "plan_refused"),
    }
}

/// A workflow failure, as a status the caller can act on.
///
/// Three outcomes, and which one it is decides what the panel does. A command
/// the state would not accept is a 409 — the request is well formed and what it
/// assumed is no longer true. Contention is a 503 the client should simply
/// repeat: somebody else appended while this decision was being made, which is
/// the store working rather than failing. And a plan the store has nowhere to
/// run is a 422 naming the variable, because it is a fact about this
/// deployment that the message has to carry back to whoever pressed the button.
fn execution_parts(error: &aiwatcher_execution::HandleError) -> (StatusCode, &'static str) {
    use aiwatcher_execution::HandleError;
    match error {
        HandleError::Decision(_) => (StatusCode::CONFLICT, "command_refused"),
        HandleError::Contended { .. } => (StatusCode::SERVICE_UNAVAILABLE, "execution_contended"),
        HandleError::NeedsMultiProcess { .. } => (StatusCode::UNPROCESSABLE_ENTITY, "plan_refused"),
        HandleError::Store(aiwatcher_execution::StoreError::PayloadTooLarge { .. }) => {
            (StatusCode::PAYLOAD_TOO_LARGE, "too_large")
        }
        // The store *worked*: somebody else appended first. A hosted decider
        // reads this and reloads, so answering 503 with the rest of the store's
        // failures would tell it to wait for something that is not going to
        // change. `handle` never surfaces one — it re-reads and decides again —
        // so this arm is the hosted append's, where the decision is the
        // worker's and this process cannot make it a second time.
        HandleError::Store(aiwatcher_execution::StoreError::VersionConflict { .. }) => {
            (StatusCode::CONFLICT, "version_conflict")
        }
        HandleError::Store(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "workflow_store_unavailable",
        ),
    }
}

/// Why a worker's append was refused, as a status.
///
/// The split that matters is 409 against 400: a conflict and a run this engine
/// decides are both things the *caller* acts on — reload, or start the run in
/// the mode it meant — while a batch that is empty or too long is a request
/// that will never be accepted however many times it is sent.
fn hosted_parts(error: &aiwatcher_execution::hosted::HostedError) -> (StatusCode, &'static str) {
    use aiwatcher_execution::hosted::HostedError;
    match error {
        HostedError::NotHosted { .. } => (StatusCode::CONFLICT, "not_hosted"),
        // Also about the run's state rather than the request, and also
        // something the caller acts on: wait, or take it over once it has run
        // out. Asking *for* the lease answers 200 either way — being told who
        // has it is an answer to that question. Appending while somebody else
        // decides is not.
        HostedError::LeaseHeld { .. } => (StatusCode::CONFLICT, "lease_held"),
        HostedError::Empty | HostedError::TooManyMessages { .. } => {
            (StatusCode::BAD_REQUEST, "invalid_append")
        }
        // A fact about the run rather than about the request, and one the
        // caller acts on by sealing the payload here — or by starting a run
        // under the policy it meant. The run's policy is fixed once it starts,
        // so this will not become true by waiting.
        HostedError::PayloadPolicyMismatch { .. } => (StatusCode::CONFLICT, "payload_policy"),
        HostedError::Handle(handle) => execution_parts(handle),
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = self.parts();
        if status.is_server_error() {
            tracing::error!(error = %self, "request failed");
        } else {
            tracing::debug!(error = %self, "request rejected");
        }
        let details = match &self {
            Self::AnnotationRegistry(aiwatcher_annotations::Error::Rejected(problems)) => {
                problems.clone()
            }
            // Same reason: a producer fixing one policy problem per round trip
            // learns to send a basis it does not mean.
            Self::ConversationArchive(aiwatcher_conversations::Error::Rejected(problems)) => {
                problems.clone()
            }
            // And the same again for a curation canvas: somebody wiring blocks
            // fixes what they can see, all at once.
            Self::DatasetRegistry(aiwatcher_datasets::RegistryError::Rejected(problems)) => {
                problems.clone()
            }
            // And once more for a plan: the compiler reports everything wrong
            // with a definition in one pass, so the canvas can draw all of it.
            Self::PlanRefused { problems, .. } => problems.clone(),
            // A worker's question, refused by the rule every gate is refused
            // by. Every problem at once, as everywhere else.
            Self::QuestionRefused { problems } => problems.clone(),
            _ => Vec::new(),
        };
        let mut response = (
            status,
            Json(ErrorBody {
                code,
                message: self.to_string(),
                details,
            }),
        )
            .into_response();

        // What RFC 9110 asks a 401 to carry. No browser dialog: `Bearer` does
        // not trigger one, unlike `Basic`.
        if status == StatusCode::UNAUTHORIZED {
            response.headers_mut().insert(
                axum::http::header::WWW_AUTHENTICATE,
                axum::http::HeaderValue::from_static("Bearer"),
            );
        }
        response
    }
}

pub type ApiResult<T> = std::result::Result<T, ApiError>;
