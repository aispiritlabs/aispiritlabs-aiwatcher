//! What a reactor asks before it reads a project's data, and again before it
//! writes any. ADR_0033.
//!
//! A bound store says which executions a process may touch at all; this is the
//! other half — whether the principal that owns one *still* holds a grant. The
//! store's answer never changes, so it can be a binding; this one changes while
//! the work runs, so it is a question asked twice.
//!
//! ```text
//!   claim ──► ownership ──► admits(Work) ──► plan ──► cache ──► step.started
//!                                                                    │
//!            report ◄── admits(Publication) ◄── lease ◄── the work ◄──┘
//! ```
//!
//! Three rules, and each is a way a project's data would otherwise leak.
//!
//! **The record is the only source.** The reactor reads
//! [`ExecutionOwnership`] from the store it is bound to and hands it here, so
//! an implementation has nowhere else to get a scope or a principal from — not
//! the plan, not a parameter, not a worker's name, not a declaration's author.
//!
//! **Before the cache lookup.** A hit is an answer about a project's data
//! whether or not any work follows, so the first ask comes before the plan is
//! even loaded — which is what [`OwnedDefinition`](crate::OwnedDefinition)
//! exists for.
//!
//! **A failure is never consent.** An implementation that cannot reach its
//! authority answers with a retryable class, and the reactor leaves the attempt
//! for the next pass rather than running it.

use async_trait::async_trait;

use crate::activity::ActivityError;
use crate::scope::ExecutionOwnership;

/// Which of the two questions the reactor is asking.
///
/// They ask the same thing of the same principal; what differs is what a
/// refusal costs. Before the work there is nothing to lose, so a retryable
/// refusal simply leaves the attempt alone. Before publication the work is
/// done and the lease is held, so every refusal is reported — and nothing is
/// recorded in the catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admitting {
    /// Before anything of this execution is read or run, the cache lookup
    /// included.
    Work,
    /// Before the outcome is recorded and the fact is written.
    Publication,
}

impl Admitting {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Work => "work",
            Self::Publication => "publication",
        }
    }
}

/// Whether the owner of one execution may have its work done right now.
///
/// Implemented outside this crate, because nothing here asks IAM anything
/// (ADR_0033). A reactor with no authority is the unscoped one every
/// deployment already runs: the store it holds sees no project's work, so
/// there is nothing to ask about.
///
/// # What an implementation may not do
///
/// Treat its own unavailability as a yes, and cache a decision across
/// attempts. A grant is a snapshot, not a capability: the reactor asks again
/// before it publishes precisely because the first answer may have expired
/// while the work ran.
#[async_trait]
pub trait ExecutionAuthority: std::fmt::Debug + Send + Sync {
    /// `Ok(())` to go on, or the class the refusal is reported under.
    ///
    /// `ownership` is `None` for an execution nobody owns. An authority for a
    /// project refuses that rather than reading it as the global side: a
    /// reactor holding one is bound to a project, and an unowned execution
    /// there is a run it may not adopt.
    ///
    /// # Errors
    ///
    /// [`ActivityError`] whose class decides what follows: a retryable one
    /// leaves the attempt claimable, and anything else fails the step.
    async fn admits(
        &self,
        ownership: Option<&ExecutionOwnership>,
        admitting: Admitting,
    ) -> Result<(), ActivityError>;
}
