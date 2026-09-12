//! What a decision, a compilation, a registry or a store refuses, and why.
//!
//! Each variant says what a caller should do differently. `NotStarted` and
//! `AlreadyFinished` are both "this command does not apply", and separating
//! them is what turns a 409 body into something somebody can act on.

use aiwatcher_core::ports::PortError;
use thiserror::Error;

use crate::state::StateType;

/// A command that does not apply to the state it was sent to.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum DecisionError {
    #[error(
        "no execution has been started on this stream, so `{message}` addresses nothing. \
         Start one first"
    )]
    NotStarted { message: String },

    #[error(
        "this execution has already been started; starting it again would give it a second plan"
    )]
    AlreadyStarted,

    #[error("this execution is already {}; nothing more happens to it without a new one", state.as_str())]
    AlreadyFinished { state: StateType },

    #[error("this execution is not paused, so there is nothing to resume")]
    NotPaused,

    #[error("a cancel is already in progress; the steps still running are being asked to stop")]
    Cancelling,

    #[error("the plan has no step called '{step}'")]
    NoSuchStep { step: String },

    #[error(
        "{step} is {}, and only a failed or crashed step is retried. \
         Cancel the execution to stop it",
        state.as_str()
    )]
    NotRetryable { step: String, state: StateType },

    #[error(
        "the report names attempt {attempt} of {step}, which is at attempt {current}. \
         The lease for {attempt} was lost and its work has been taken over"
    )]
    StaleAttempt {
        step: String,
        attempt: u32,
        current: u32,
    },

    #[error("{step} is not waiting for an answer")]
    NotWaiting { step: String },

    #[error("the deadline for answering {step} has passed")]
    DeadlinePassed { step: String },

    #[error("{step} may be answered with one of: {}", choices.join(", "))]
    NotOneOfTheChoices { step: String, choices: Vec<String> },

    #[error("a plan with no steps executes nothing")]
    EmptyPlan,

    #[error("the plan's edges form a cycle, so no step can be first")]
    CyclicPlan,

    #[error("`{message}` is not something an execution decides on")]
    Unhandled { message: String },
}

/// Every reason a definition does not compile to a runnable plan, at once.
///
/// A `Vec<String>` rather than a first problem, for the reason
/// `aiwatcher_datasets::pipeline::order_of` already gives: somebody wiring a
/// canvas fixes what they can see, and a validator that reports one thing per
/// round trip teaches people to press the button again instead of reading it.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CompileError {
    #[error("{}", .0.join("; "))]
    Refused(Vec<String>),
}

impl CompileError {
    #[must_use]
    pub fn problems(&self) -> &[String] {
        let Self::Refused(problems) = self;
        problems
    }
}

/// What the registry of authored workflow definitions refuses.
///
/// Three answers rather than one string, because a scheduler has one question
/// about a refusal: does it say the same thing on the next tick? An object
/// store that could not be reached does not. A stored object that will not
/// read back does, and so does a definition that does not compile. Flattened
/// together — which they were — a corrupt registered workflow read as a bad
/// moment, and its slot was left due and retried every minute for ever.
///
/// The three words are [`aiwatcher_datasets::RegistryError`]'s own, for the
/// two registries a managed run may be started from. One question answered in
/// two vocabularies is two answers a release apart.
///
/// [`aiwatcher_datasets::RegistryError`]: https://docs.rs/aiwatcher-datasets
#[derive(Debug, Error)]
pub enum DefinitionError {
    #[error("the workflow definition registry could not use its object store: {0}")]
    Store(#[from] PortError),

    #[error("stored object {key} is not a workflow definition document: {message}")]
    Corrupt { key: String, message: String },

    /// Every problem at once, as [`CompileError`] reports them: the store's
    /// own door refuses what the route in front of it already refused, and a
    /// caller that reaches it directly deserves the same list.
    #[error("the workflow definition was refused: {}", .0.join("; "))]
    Refused(Vec<String>),
}

impl DefinitionError {
    /// Whether asking again would be told the same thing.
    ///
    /// [`StoreError::says_the_same_next_time`]'s question, asked of the other
    /// registry a managed run is compiled from. Only an unreachable store is
    /// worth coming back for: a store that understood the read and refused it
    /// refuses it identically, and neither a corrupt object nor a definition
    /// that does not compile changes on a timer.
    #[must_use]
    pub const fn says_the_same_next_time(&self) -> bool {
        match self {
            Self::Store(port) => !port.is_retryable(),
            Self::Corrupt { .. } | Self::Refused(_) => true,
        }
    }
}

/// What a [`WorkflowStore`](crate::store::WorkflowStore) refuses.
#[derive(Debug, Error)]
pub enum StoreError {
    /// Somebody else appended between the load and the append. The caller
    /// re-reads and decides again — a bounded retry around a pure function,
    /// which is why this is not an error the API surfaces.
    #[error("the stream is at version {actual}, not {expected}")]
    VersionConflict { expected: u64, actual: u64 },

    #[error("a message may be at most {limit} bytes; this one is {size}")]
    PayloadTooLarge { size: usize, limit: usize },

    #[error(
        "the `file` workflow store holds one process. Set AIWATCHER_WORKFLOW_STORE=postgres \
         to run a worker, a container job or a second API replica"
    )]
    SingleProcessOnly,

    #[error("{0}")]
    Backend(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Encoding(#[from] serde_json::Error),
}

impl StoreError {
    /// Whether asking again would be told the same thing.
    ///
    /// [`HandleError::says_the_same_next_time`](crate::HandleError::says_the_same_next_time)'s
    /// half about the store. A message that is too large stays too large and a
    /// single-process store stays one; a version conflict means somebody else
    /// appended, which is the store working. [`Self::Backend`] and
    /// [`Self::Io`] are the honest `false`: they have flattened whatever the
    /// adapter hit, so the safe answer is that it may have been a bad moment.
    #[must_use]
    pub const fn says_the_same_next_time(&self) -> bool {
        match self {
            Self::PayloadTooLarge { .. } | Self::SingleProcessOnly | Self::Encoding(_) => true,
            Self::VersionConflict { .. } | Self::Backend(_) | Self::Io(_) => false,
        }
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;
