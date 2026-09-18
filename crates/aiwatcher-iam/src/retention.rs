//! How long the audit trail is kept, on a clock of its own.
//!
//! The control plane's mutation history is the one table here that grows
//! without end: every membership change, every grant, every offer, for as long
//! as the organization exists. Until this existed nothing removed any of it,
//! which the README said in one sentence — "retention/export is not
//! implemented" — and which a workshop instance turns into a table with a year
//! of enrolments in it.
//!
//! **The clock is this crate's own.** It is not the event log's retention,
//! which is sized for a volume of telemetry; it is not the conversation
//! archive's, which is bounded by what somebody was told; and it is not the
//! workflow store's, which follows what redelivers. Tying any of them together
//! would mean raising one silently shortens or extends another, and an audit
//! trail is the worst of the four to shorten by accident. [`AuditRetention`]
//! carries its own `ttl_days` and the `policy_id` of the written policy it
//! implements.
//!
//! **Nothing over HTTP prunes.** There is no route here and there is not meant
//! to be one: an administrator who could delete the record of their own
//! administration is the one capability an audit trail must not grant. The
//! sweep is configuration — a deployment's clock, applied by the work it runs —
//! in the shape `AIWATCHER_WORKFLOW_RUNNER_URL` already uses for the same
//! reason.
//!
//! **A gap says so.** Removing rows from under a paginated read would leave an
//! administrator looking at a history that starts at sequence 4 312 with
//! nothing to say why. So a sweep writes an [`AuditWatermark`] in the same
//! transaction as the delete: what was removed, how far, before which moment,
//! under which policy. A reader who wants the removed entries reads the export
//! that froze them.

use serde::{Deserialize, Serialize};

use crate::{Error, OrganizationId, Result};

/// Seconds in a day. Retention is expressed in days because that is what a
/// written policy says; the clock underneath is Unix seconds, as everywhere
/// else in this crate.
const DAY_SECONDS: i64 = 86_400;

/// The longest a deployment may ask for: a hundred years, which is "keep it"
/// expressed as a number rather than as a special case. The point of the
/// ceiling is that `ttl_days` fits in the arithmetic below without care.
const MAX_TTL_DAYS: u32 = 36_500;

/// How long this deployment keeps its own control plane's mutation history.
///
/// Absent — a `None` wherever one of these is held — means nothing is ever
/// removed, which is what every existing installation does today and therefore
/// what an upgrade must keep doing.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamAuditRetention))]
pub struct AuditRetention {
    pub ttl_days: u32,
    /// Which written policy this implements. Recorded, never interpreted — the
    /// conversation archive's `RetentionPolicy::policy_id`, for the same reason:
    /// a number in a configuration file cannot say what it was agreed to be.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub policy_id: String,
}

impl AuditRetention {
    /// # Errors
    ///
    /// [`Error::Invalid`] for a `ttl_days` of zero or past [`MAX_TTL_DAYS`].
    ///
    /// Zero is refused rather than read as "expire immediately", which is the
    /// opposite of the conversation archive's rule and deliberately so. There,
    /// a turn nobody may keep is a legitimate thing to declare. Here, a single
    /// mistyped character would delete the entire record of who granted what,
    /// and "off" is already expressible by not configuring a retention at all —
    /// the same reading the workflow store's retention gives a zero.
    pub fn new(ttl_days: u32, policy_id: impl Into<String>) -> Result<Self> {
        if ttl_days == 0 {
            return Err(Error::Invalid(
                "an audit retention of zero days would empty the trail; leave it unset to keep \
                 every entry"
                    .into(),
            ));
        }
        if ttl_days > MAX_TTL_DAYS {
            return Err(Error::Invalid(format!(
                "an audit retention of {ttl_days} days is past the {MAX_TTL_DAYS}-day ceiling"
            )));
        }
        Ok(Self {
            ttl_days,
            policy_id: policy_id.into(),
        })
    }

    /// Entries that occurred strictly before this moment may be removed.
    ///
    /// Half-open, as every window in this crate is: an entry written exactly on
    /// the boundary is kept.
    #[must_use]
    pub fn cutoff(&self, now: i64) -> i64 {
        now.saturating_sub(i64::from(self.ttl_days).saturating_mul(DAY_SECONDS))
    }
}

/// Where an organization's audit trail begins, and why it does not begin at 1.
///
/// Written in the same transaction as the delete it describes, so there is no
/// moment at which rows are gone and nothing says so. A store that has never
/// pruned an organization has no watermark for it, which reads as "the trail is
/// whole".
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamAuditWatermark))]
pub struct AuditWatermark {
    pub organization: OrganizationId,
    /// The highest sequence removed so far. Everything a reader can still page
    /// begins after it.
    ///
    /// Derived from the rows actually deleted rather than from the cutoff, so a
    /// system clock that stepped backwards — leaving one old entry above a
    /// newer one — narrows this rather than claiming to have removed something
    /// still there. The next sweep takes what it left.
    pub pruned_through_sequence: i64,
    /// The cutoff of the sweep that last moved this: entries older than this
    /// moment are the ones that went.
    pub pruned_before: i64,
    /// The retention in force when it did, recorded because the configuration
    /// may have changed since.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub policy_id: String,
    pub ttl_days: u32,
    pub pruned_at: i64,
    /// Across every sweep, not only the last one.
    pub removed_total: i64,
}

/// What an organization's audit trail holds now.
///
/// One read that answers both halves of the question a paginated audit cannot:
/// how much is there, and — when a sweep has been through — where it begins and
/// why. `watermark` absent means the trail is whole.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamAuditBounds))]
pub struct AuditBounds {
    pub organization: OrganizationId,
    /// The lowest sequence still stored, or `None` when nothing is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_sequence: Option<i64>,
    /// The highest, and what an export pins its range to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sequence: Option<i64>,
    pub entries: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watermark: Option<AuditWatermark>,
}

/// What one pass of the sweep did.
///
/// `organizations` counts the ones that lost an entry, not the ones looked at:
/// a sweep that runs hourly and finds nothing is the normal case and should say
/// nothing rather than report a number that never changes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamAuditPruneReport))]
pub struct PruneReport {
    pub organizations: usize,
    pub removed: i64,
    /// The moment entries had to precede in order to go. One per pass, because
    /// one clock reading covers the whole pass.
    pub cutoff: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_retention_of_no_days_is_refused_rather_than_read_as_immediately() {
        assert!(matches!(
            AuditRetention::new(0, "policy-1"),
            Err(Error::Invalid(_))
        ));
        assert!(AuditRetention::new(1, "policy-1").is_ok());
    }

    #[test]
    fn a_retention_past_the_ceiling_is_refused() {
        assert!(AuditRetention::new(MAX_TTL_DAYS, "").is_ok());
        assert!(matches!(
            AuditRetention::new(MAX_TTL_DAYS + 1, ""),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn the_cutoff_is_the_retention_behind_now() {
        let retention = AuditRetention::new(30, "policy-1").expect("a valid retention");
        assert_eq!(retention.cutoff(30 * DAY_SECONDS), 0);
        assert_eq!(retention.cutoff(31 * DAY_SECONDS), DAY_SECONDS);
    }

    #[test]
    fn a_clock_far_enough_back_saturates_rather_than_wrapping() {
        // A wrapped cutoff would land in the far future and take the whole
        // trail. There is no clock this defends against in practice; there is
        // also no reason for the arithmetic to be the thing that decides.
        let retention = AuditRetention::new(MAX_TTL_DAYS, "").expect("a valid retention");
        assert_eq!(retention.cutoff(i64::MIN), i64::MIN);
        assert_eq!(
            retention.cutoff(0),
            -(i64::from(MAX_TTL_DAYS) * DAY_SECONDS)
        );
    }
}
