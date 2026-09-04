//! The row a reactor or a worker takes before it does any work.
//!
//! Section 9.3's `step_attempts`, as a type. One row per attempt, and it is the
//! only place in this system where a store is used as a queue — bounded on
//! purpose: one row per claim, one heartbeat per half-lease, and no fan-out.
//!
//! ## Why both a reactor and a worker claim from here
//!
//! Revision 1 of the plan gave commands their own topic. With the store
//! transactional they do not need one: a dispatch is a row, and taking it is a
//! claim. A reactor in the work role claims by **runtime** — it is the process
//! that holds the Flow and notebook clients — and a worker claims by **queue**,
//! because it is a process somebody else operates and the queue is what its
//! token authorises. Same table, two filters, and the lease means the same
//! thing to both.
//!
//! ## The lease is `aiwatcher-jobs`'
//!
//! [`aiwatcher_jobs::LEASE_SECONDS`] and [`aiwatcher_jobs::lease_expired`],
//! called rather than copied. ADR_0022's reason holds exactly: a second copy of
//! a lease rule is a silent corruption the day one of them changes. What this
//! module adds is who may *renew* one — the holder, and nobody else, so a
//! worker whose lease expired under it stops rather than writing beside its
//! replacement.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

use aiwatcher_core::MessageId;

use crate::plan::RuntimeKind;
use crate::state::{ExecutionId, StateType};

/// Which attempt, of which step, of which execution.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize, ToSchema)]
pub struct AttemptKey {
    pub execution_id: ExecutionId,
    pub step_id: String,
    pub attempt: u32,
}

impl AttemptKey {
    #[must_use]
    pub fn new(execution_id: ExecutionId, step_id: impl Into<String>, attempt: u32) -> Self {
        Self {
            execution_id,
            step_id: step_id.into(),
            attempt,
        }
    }

    /// `<execution>/<step>/<attempt>` — the same string the dispatch carried,
    /// and what a reactor asks a runtime by before it retries a timeout.
    #[must_use]
    pub fn idempotency_key(&self) -> String {
        crate::decide::idempotency_key(self.execution_id.as_str(), &self.step_id, self.attempt)
    }
}

impl std::fmt::Display for AttemptKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.idempotency_key())
    }
}

/// One dispatched attempt, and whether anybody holds it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct AttemptRow {
    pub key: AttemptKey,
    pub runtime: RuntimeKind,
    /// The command that dispatched this attempt. A reactor deduplicates by it,
    /// which is what makes a redelivered dispatch a no-op rather than a second
    /// run of the same work.
    pub command_id: MessageId,
    /// The worker queue this is claimable on. `None` for a step a reactor in
    /// the work role runs, which is claimed by runtime instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue: Option<String>,
    /// `name@version`: what a worker must match before it may claim this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_ref: Option<String>,
    pub state: StateType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_owner: Option<String>,
    /// Who held this attempt before the current holder.
    ///
    /// The signal a takeover needs, and it cannot be read off `lease_owner`:
    /// by the time a claimant sees the row, that field already names *it*. A
    /// row with a previous owner is an attempt somebody else started, whose
    /// call may have finished after their lease expired — so the runtime is
    /// asked by its idempotency key before the work is done again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub claimed_at: Option<OffsetDateTime>,
    /// When a retry may be taken. A row before its time is not claimable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub not_before: Option<OffsetDateTime>,
}

impl AttemptRow {
    /// A dispatched attempt nobody has taken yet.
    #[must_use]
    pub fn claimable(key: AttemptKey, runtime: RuntimeKind, command_id: MessageId) -> Self {
        Self {
            key,
            runtime,
            command_id,
            queue: None,
            task_ref: None,
            state: StateType::Pending,
            lease_owner: None,
            previous_owner: None,
            claimed_at: None,
            not_before: None,
        }
    }

    #[must_use]
    pub fn on_queue(mut self, queue: String, task_ref: String) -> Self {
        self.queue = Some(queue);
        self.task_ref = Some(task_ref);
        self
    }

    #[must_use]
    pub fn not_before(mut self, at: OffsetDateTime) -> Self {
        self.not_before = Some(at);
        self
    }

    /// A row in a terminal state. What a completion or a failure writes, and
    /// what takes the attempt out of every claimant's view.
    #[must_use]
    pub fn settled(key: AttemptKey, runtime: RuntimeKind, state: StateType) -> Self {
        Self {
            key,
            runtime,
            command_id: MessageId::new(String::new()),
            queue: None,
            task_ref: None,
            state,
            lease_owner: None,
            previous_owner: None,
            claimed_at: None,
            not_before: None,
        }
    }

    /// Whether this row may be taken now.
    ///
    /// Four conditions, and the second one is the whole reason a lost claim
    /// recovers on its own: the work is not finished, nobody holds a live lease
    /// on it, and any retry delay has passed.
    ///
    /// `awaiting_input` is excluded even though it is not terminal, and that is
    /// the point of it being its own state: an attempt waiting for a person
    /// holds no lease *and* is not work anybody may pick up. It resumes on the
    /// answer it asked for, from the role that may give it — never because five
    /// minutes went by.
    #[must_use]
    pub fn is_claimable(&self, now: OffsetDateTime) -> bool {
        !self.state.is_terminal()
            && self.state != StateType::AwaitingInput
            && aiwatcher_jobs::lease_expired(self.claimed_at, now)
            && self.not_before.is_none_or(|at| now >= at)
    }

    /// Whether `owner` still holds this row.
    ///
    /// What a worker re-checks at every boundary before it writes. A lease that
    /// expired under it has been taken over, and writing beside a replacement
    /// is the one outcome the lease exists to prevent.
    #[must_use]
    pub fn is_held_by(&self, owner: &str, now: OffsetDateTime) -> bool {
        self.lease_owner.as_deref() == Some(owner)
            && !aiwatcher_jobs::lease_expired(self.claimed_at, now)
    }

    /// Take this row for `owner`, until the lease runs out.
    pub fn claim(&mut self, owner: &str, now: OffsetDateTime) {
        // Kept before it is overwritten: this is the only moment the row knows
        // it is changing hands.
        if self.lease_owner.as_deref() != Some(owner) {
            self.previous_owner = self.lease_owner.take();
        }
        self.lease_owner = Some(owner.to_owned());
        self.claimed_at = Some(now);
        self.state = StateType::Running;
    }

    /// When this claim stops being trusted.
    #[must_use]
    pub fn lease_expires_at(&self) -> Option<OffsetDateTime> {
        self.claimed_at
            .map(|at| at + time::Duration::seconds(aiwatcher_jobs::LEASE_SECONDS))
    }
}

/// What a claimant is willing to take.
///
/// A reactor names runtimes, because it is the process holding those clients. A
/// worker names queues, because that is what its token authorises. Both empty
/// takes nothing — a claimant that said what it can do is safer than one that
/// takes whatever is there.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClaimFilter {
    pub runtimes: Vec<RuntimeKind>,
    pub queues: Vec<String>,
}

impl ClaimFilter {
    /// A reactor in the work role: the runtimes whose clients it holds.
    #[must_use]
    pub fn for_runtimes(runtimes: &[RuntimeKind]) -> Self {
        Self {
            runtimes: runtimes.to_vec(),
            queues: Vec::new(),
        }
    }

    /// A worker: the queues its token authorises.
    #[must_use]
    pub fn for_queues(queues: &[String]) -> Self {
        Self {
            runtimes: Vec::new(),
            queues: queues.to_vec(),
        }
    }

    /// Whether this claimant would take that row.
    #[must_use]
    pub fn matches(&self, row: &AttemptRow) -> bool {
        match &row.queue {
            // A pulled attempt belongs to whoever holds its queue, and to
            // nobody else — a reactor listing `python_task` must not take work
            // scheduled for somebody's worker.
            Some(queue) => self.queues.iter().any(|allowed| allowed == queue),
            None => self.runtimes.contains(&row.runtime),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> OffsetDateTime {
        OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(seconds)
    }

    fn row() -> AttemptRow {
        AttemptRow::claimable(
            AttemptKey::new(ExecutionId::new("exec-1"), "extract", 1),
            RuntimeKind::FlowPhp,
            MessageId::new("cmd-1"),
        )
    }

    #[test]
    fn a_dispatched_attempt_nobody_holds_is_claimable() {
        assert!(row().is_claimable(at(0)));
    }

    #[test]
    fn a_row_that_changed_hands_remembers_who_held_it() {
        // The signal a takeover needs. It cannot be read off `lease_owner`: by
        // the time a claimant sees the row, that field already names it.
        let mut row = row();
        row.claim("reactor-a", at(0));
        assert_eq!(row.previous_owner, None, "nobody held it before");

        row.claim("reactor-b", at(aiwatcher_jobs::LEASE_SECONDS + 1));
        assert_eq!(row.previous_owner.as_deref(), Some("reactor-a"));

        // Renewing under the same name is not a change of hands.
        row.claim("reactor-b", at(aiwatcher_jobs::LEASE_SECONDS + 2));
        assert_eq!(row.previous_owner.as_deref(), Some("reactor-a"));
    }

    #[test]
    fn a_live_claim_keeps_everybody_else_out_and_expires_on_its_own() {
        let mut row = row();
        row.claim("reactor-a", at(0));
        assert!(!row.is_claimable(at(10)));
        assert!(row.is_held_by("reactor-a", at(10)));
        assert!(!row.is_held_by("reactor-b", at(10)));

        // The lease is `aiwatcher-jobs`', called rather than copied.
        let past = at(aiwatcher_jobs::LEASE_SECONDS + 1);
        assert!(row.is_claimable(past), "a lost claim recovers on its own");
        assert!(
            !row.is_held_by("reactor-a", past),
            "and the holder stops rather than writing beside its replacement"
        );
    }

    #[test]
    fn an_attempt_waiting_for_a_person_is_not_picked_up_because_time_passed() {
        // The reason `awaiting_input` is a state of its own rather than a name
        // for `paused`: it resumes on the answer it asked for, from the role
        // that may give it — and a lease expiring is neither.
        let mut row = row();
        row.state = StateType::AwaitingInput;
        assert!(!row.is_claimable(at(aiwatcher_jobs::LEASE_SECONDS + 1)));
    }

    #[test]
    fn a_settled_attempt_is_out_of_every_claimants_view() {
        let mut row = row();
        row.state = StateType::Completed;
        assert!(!row.is_claimable(at(0)));
    }

    #[test]
    fn a_retry_is_not_claimable_before_its_delay_has_passed() {
        let row = row().not_before(at(30));
        assert!(!row.is_claimable(at(10)));
        assert!(row.is_claimable(at(30)));
    }

    #[test]
    fn a_reactor_does_not_take_work_scheduled_for_somebodys_worker() {
        // A pulled attempt belongs to whoever holds its queue. A reactor that
        // listed `python_task` and took one would be running a registered
        // function in the process that holds the object store's credentials.
        let pulled = AttemptRow::claimable(
            AttemptKey::new(ExecutionId::new("exec-1"), "stage", 1),
            RuntimeKind::PythonTask,
            MessageId::new("cmd-1"),
        )
        .on_queue("houses".to_owned(), "stage@1".to_owned());

        assert!(!ClaimFilter::for_runtimes(&[RuntimeKind::PythonTask]).matches(&pulled));
        assert!(ClaimFilter::for_queues(&["houses".to_owned()]).matches(&pulled));
        assert!(!ClaimFilter::for_queues(&["other".to_owned()]).matches(&pulled));
    }

    #[test]
    fn a_reactor_takes_the_runtimes_whose_clients_it_holds_and_no_others() {
        let flow = row();
        assert!(ClaimFilter::for_runtimes(&[RuntimeKind::FlowPhp]).matches(&flow));
        assert!(!ClaimFilter::for_runtimes(&[RuntimeKind::Marimo]).matches(&flow));
        assert!(
            !ClaimFilter::default().matches(&flow),
            "a claimant that named nothing takes nothing"
        );
    }

    #[test]
    fn an_attempt_key_is_the_idempotency_key_the_dispatch_carried() {
        assert_eq!(
            AttemptKey::new(ExecutionId::new("exec-1"), "extract", 2).idempotency_key(),
            "exec-1/extract/2"
        );
    }
}
