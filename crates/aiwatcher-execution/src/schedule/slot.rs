//! One slot, and what this system did about it.
//!
//! The state the tick keeps, held apart from [`ScheduledDefinition`], which is
//! **configuration** owned by whoever sets it. The tick may not write that: a
//! loop that read every schedule and wrote the whole object back overwrote any
//! edit or DELETE that landed in between, and a deleted schedule came back
//! enabled. Re-reading before the write does not close it — the two have
//! different writers and different lifetimes.
//!
//! It lives in the **workflow store** rather than in a second object, because
//! two things need a decision taken atomically against state another replica
//! can see:
//!
//! * `overlap = skip` must ask "does this definition have an unfinished run" of
//!   the same transaction that takes the slot. Asked of the read model it was
//!   answered from an asynchronous fold that is *empty in the `work` role*, so
//!   skip never skipped where the tick runs.
//! * A transient failure must not become a recorded refusal. A slot taken under
//!   a lease does not depend on the global cursor, so a tick that dies holding
//!   one leaves work the next tick picks up — rather than costing the day's run
//!   and leaving a note claiming it was refused.
//!
//! An object store offers no compare-and-set; the workflow store is
//! transactional by construction.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

use super::rule::OverlapPolicy;
use crate::plan::DefinitionKind;

/// The slot a definition is due for, as a key.
///
/// The slot is the *intended* instant and never the moment the tick noticed
/// it: a run started late after an outage belongs to the nine o'clock it was
/// for, which is also what makes the derived execution id stable across a
/// replay.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize, ToSchema)]
pub struct SlotKey {
    pub definition_kind: DefinitionKind,
    pub definition_name: String,
    #[serde(with = "time::serde::rfc3339")]
    pub slot: OffsetDateTime,
}

impl SlotKey {
    #[must_use]
    pub fn new(
        definition_kind: DefinitionKind,
        name: impl Into<String>,
        slot: OffsetDateTime,
    ) -> Self {
        Self {
            definition_kind,
            definition_name: name.into(),
            slot,
        }
    }
}

/// What the tick decided about one slot. Terminal: a settled slot is done.
///
/// **The scheduler's own decision, and never the run's outcome.** Whether the
/// run then succeeded is on the event log, which the Workflows view folds, and
/// a second copy here would be free to disagree with it (ADR_0026).
///
/// The three the previous release's `FiringOutcome` had, unchanged, because
/// the panel already renders them and they are already what a person reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SlotOutcome {
    /// A run was started. Whether it *succeeded* is the log's answer, by the
    /// id beside this.
    Started,
    /// `OverlapPolicy::Skip`, and the run named beside this had not finished.
    Skipped,
    /// Nothing could be started and trying again would not help — the
    /// definition stopped compiling, or was deleted. The one outcome that is a
    /// problem, and deliberately not what a store outage produces.
    Refused,
}

/// What a caller says about a slot it holds.
///
/// [`Self::TryAgain`] is. A failure that *may* succeed
/// next time must not be written down as a decision: it drops the lease and
/// leaves the slot due. Only a caller that has read the failure can tell the
/// two apart — a 4xx is about the definition and will say the same thing every
/// tick, a 5xx is about something being unreachable — which is why this is the
/// caller's word and not a flag the store infers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SlotSettlement {
    Started { execution_id: String },
    Skipped { execution_id: String },
    Refused { detail: String },
    TryAgain { detail: String },
}

impl SlotSettlement {
    /// The decision this settlement records, if it records one.
    #[must_use]
    pub const fn outcome(&self) -> Option<SlotOutcome> {
        match self {
            Self::Started { .. } => Some(SlotOutcome::Started),
            Self::Skipped { .. } => Some(SlotOutcome::Skipped),
            Self::Refused { .. } => Some(SlotOutcome::Refused),
            Self::TryAgain { .. } => None,
        }
    }

    /// The run it names, if it names one.
    #[must_use]
    pub fn execution_id(&self) -> Option<&str> {
        match self {
            Self::Started { execution_id } | Self::Skipped { execution_id } => Some(execution_id),
            Self::Refused { .. } | Self::TryAgain { .. } => None,
        }
    }

    /// The words, if there are any.
    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::Refused { detail } | Self::TryAgain { detail } => Some(detail),
            Self::Started { .. } | Self::Skipped { .. } => None,
        }
    }
}

/// A slot as the store holds it.
///
/// Flat rather than an outcome carrying its own fields, and that is not
/// laziness: it is one row in `schedule_slots` and one shape the panel already
/// renders, so a column, a field and a rendered line are the same three things
/// end to end.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct SlotRecord {
    #[serde(flatten)]
    pub key: SlotKey,
    /// `None` while it is still being decided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<SlotOutcome>,
    /// The run this slot started, or the one that blocked it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    /// Why it was refused — or, while it is unsettled, what the last attempt
    /// ran into. A slot that has been failing every tick says what is wrong
    /// with it rather than looking untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Who is trying, and since when. Absent once it is settled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub leased_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

impl SlotRecord {
    /// Whether this slot may be taken now.
    ///
    /// Settled slots never are. An unsettled one is available when nobody
    /// holds it or the holder's lease has run out — the claim table's rule
    /// ([`crate::claim::AttemptRow::is_claimable`]), for the same reason: a
    /// process that died holding a slot must not keep it for ever.
    #[must_use]
    pub fn is_available(&self, now: OffsetDateTime) -> bool {
        if self.outcome.is_some() {
            return false;
        }
        let Some(leased_at) = self.leased_at else {
            return true;
        };
        now - leased_at >= time::Duration::seconds(aiwatcher_jobs::LEASE_SECONDS)
    }
}

/// What a caller asks for when it wants to run one slot.
#[derive(Clone, Debug)]
pub struct SlotAdmissionRequest {
    pub key: SlotKey,
    /// This process, so a takeover after a lease expires can say who held it.
    pub owner: String,
    /// Whether a run that has not finished blocks this one.
    pub overlap: OverlapPolicy,
    pub now: OffsetDateTime,
}

/// The store's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SlotAdmission {
    /// Take it. Exactly one caller gets this for a given slot until the lease
    /// expires or it is settled.
    Admitted,
    /// Somebody already decided about this slot. Not a failure — it is the
    /// expected answer for the second of two replicas, and for a replay of an
    /// interval this instance has already processed.
    Settled { outcome: SlotOutcome },
    /// Another caller holds it and its lease has not run out.
    Held { owner: String },
    /// `OverlapPolicy::Skip`, and this definition has a run that has not
    /// finished. The caller settles the slot as [`SlotOutcome::Skipped`]; the
    /// store does not do it, because "skipped" is a decision and this call is
    /// only ever asked whether it *may* start.
    Blocked { execution_id: String },
}
