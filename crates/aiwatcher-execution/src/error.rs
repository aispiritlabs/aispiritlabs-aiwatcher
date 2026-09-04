//! What a decision, a compilation or a store refuses, and why.
//!
//! Each variant says what a caller should do differently. `NotStarted` and
//! `AlreadyFinished` are both "this command does not apply", and separating
//! them is what turns a 409 body into something somebody can act on.

use thiserror::Error;

use crate::state::StateType;

/// A command that does not apply to the state it was sent to.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum DecisionError {
    #[error(
        "no execution has been started on this stream, so `{message}` addresses nothing. \
         Start one first"
    )]
    NotStarted { message: &'static str },

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
    Unhandled { message: &'static str },
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

pub type Result<T> = std::result::Result<T, StoreError>;
