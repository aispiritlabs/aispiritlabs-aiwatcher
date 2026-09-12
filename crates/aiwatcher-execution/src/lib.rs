//! Owned execution: the plan, the pure decider, and the store its history lives
//! in. No I/O in this crate — only the port that has it.
//!
//! ```text
//!   definition ──► compile ──► ExecutionPlan ──► ExecutionRun
//!   editable      derived      immutable,        one attempt at one plan
//!                              plan_id                 │
//!                                                      ▼
//!    input ──► decide(state, input) ──► events + effect commands
//!      │              pure                    │
//!      │                                      ├─► the stream (why)
//!      │                                      ├─► the projection (what now)
//!      └──── the store's inbox ◄──────────────┴─► the outbox ──► the log (what happened)
//! ```
//!
//! [`decide`] is pure: no clock, no socket, no random value. Time arrives in
//! [`decide::Now`] and ids are derived from what they name, so a replay reaches
//! the same command id and a redelivered dispatch lands on the attempt it
//! already created.
//!
//! [`store`] is the one transactional operation — six writes that must land
//! together, behind a port with four adapters (`memory | file | postgres |
//! duckdb`, the last two behind cargo features). Splitting them is the
//! dual-write gap this design closes.
//!
//! It executes nothing: no Flow client, no notebook client, no cluster
//! credential. Those belong to the reactors, and every address is
//! configuration. It holds no second copy of the durable-job rules either —
//! [`aiwatcher_jobs::LEASE_SECONDS`], [`aiwatcher_jobs::after_failure`] and
//! [`aiwatcher_jobs::ORDERING`] are called, not restated.
//!
//! ADR_0025, ADR_0026.

pub mod activity;
pub mod artifact;
pub mod cache;
pub mod claim;
pub mod compile;
pub mod context;
pub mod decide;
pub mod definition;
pub mod error;
pub mod facts;
pub mod handler;
pub mod hosted;
pub mod message;
pub mod outbox;
pub mod plan;
pub mod pods;
pub mod reactor;
pub mod schedule;
pub mod start;
pub mod state;
pub mod store;
#[cfg(feature = "testing")]
pub mod testing;

pub use activity::{
    ActivityCommand, ActivityContext, ActivityError, ActivityExecutor, ActivityResult,
    ExecutorRegistry, PriorAttempt,
};
pub use artifact::{
    ArtifactCatalog, CacheEntry, CatalogedArtifact, MemoryArtifactCatalog, ObjectArtifactCatalog,
    Provenance,
};
pub use cache::cache_key;
pub use claim::{AttemptKey, AttemptRow, AttemptWrite, ClaimFilter, tally_unclaimed};
pub use compile::{CompileOptions, compile_curation};
pub use context::{ContextAction, ContextSnapshot, RunAction, allowed_run_actions};
pub use decide::{Decision, Now, decide, evolve, idempotency_key, initial_state, replay};
pub use error::{CompileError, DecisionError, DefinitionError, Result, StoreError};
pub use facts::{FactContext, PublishedBy, envelopes_for};
pub use handler::{ExecutionHandler, HandleError, Handled};
pub use message::{
    Direction, MessageMetadata, OutboxMessage, PayloadDefault, PayloadPolicy, PendingMessage,
    RecordedMessage, RunProjection, WorkflowCommand, WorkflowEvent, WorkflowMessage,
};
pub use outbox::{Published, publish_pending};
pub use plan::{
    DefinitionKind, DefinitionRevision, ExecutionPlan, PlanEdge, PlanId, PlanStep, RetryPolicy,
    RuntimeBinding, RuntimeKind,
};
pub use reactor::{Performed, Reactor};
pub use schedule::{
    Cadence, OverlapPolicy, Schedule, ScheduleReader, ScheduleStore, ScheduledDefinition,
    SlotAdmission, SlotAdmissionRequest, SlotKey, SlotOutcome, SlotRecord, SlotSettlement,
};
pub use start::{
    Decider, ExecutionTarget, Executions, Missing, RunIdentity, StartRefused, StartRequest,
    StartRun, Started, Window,
};
pub use state::{
    Execution, ExecutionId, ExecutionMode, ExecutionOwner, ExecutionState, FailureClass, RunState,
    StateType, StepError, StepState,
};
pub use store::{
    AppendOutcome, AppendRequest, ExpectedVersion, Pruned, StoreCapabilities, StreamSlice,
    WorkflowStore,
};

use sha2::{Digest, Sha256};

/// `sha256`, hex, lower case. The one everything here is addressed by.
///
/// [`aiwatcher_jobs::digest`] by another name, called rather than re-derived —
/// re-exported so a reader of `plan_id` does not have to know which crate owns
/// the hash.
#[must_use]
pub fn digest(bytes: &[u8]) -> String {
    aiwatcher_jobs::digest(bytes)
}

/// A UUID derived from a name, with no randomness in it.
///
/// The decider needs message ids and must not generate them: a replay has to
/// reach the same ids, or a redelivered dispatch creates a second attempt
/// beside the one it meant to repeat. Version 8 is the one reserved for
/// exactly this — an implementation-defined derivation — and the derivation is
/// `sha256`, so the Python worker can compute the same id from the same name.
#[must_use]
pub fn derive_uuid(name: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"aiwatcher/execution/uuid8/");
    hasher.update(name.as_bytes());
    let hash = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hash[..16]);
    // Version 8, RFC 9562 variant. Without them this is a 128-bit number that
    // some readers would refuse and others would silently accept as v4.
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_derived_id_is_the_same_one_every_replay_reaches() {
        assert_eq!(
            derive_uuid("execution-1/step/1"),
            derive_uuid("execution-1/step/1")
        );
        assert_ne!(
            derive_uuid("execution-1/step/1"),
            derive_uuid("execution-1/step/2")
        );
    }

    #[test]
    fn a_derived_id_is_a_uuid_a_reader_will_accept() {
        let derived = derive_uuid("anything");
        let parsed = uuid::Uuid::parse_str(&derived).expect("a uuid");
        assert_eq!(parsed.get_version_num(), 8, "reserved for exactly this");
        assert_eq!(derived.len(), 36);
    }

    #[test]
    fn the_hash_is_the_one_this_workspace_already_agrees_on() {
        // Called rather than re-derived: two digests that must agree byte for
        // byte and are computed in two places will disagree the day one moves.
        assert_eq!(digest(b""), aiwatcher_jobs::digest(b""));
    }
}
