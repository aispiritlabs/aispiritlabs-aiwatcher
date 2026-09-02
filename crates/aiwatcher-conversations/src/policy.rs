//! Why this content may be kept, for how long, and what a deployment demands
//! before it takes any.
//!
//! Three types, and the reason they are three is the same reason
//! `aiwatcher_annotations::license` is one module rather than three scattered
//! ones: [`ConsentRecord`] is what somebody **asserted**, [`RetentionPolicy`]
//! is how long they said it may be kept, and [`ArchivePolicy`] is what this
//! deployment **demands** — and only the third outranks a caller.
//!
//! The default is the strict one, which is the opposite of the usual default:
//! [`PolicyMode::Protected`] refuses a turn with no consent provenance, and
//! `AIWATCHER_CONVERSATION_POLICY=open` is how a deployment says it will sort
//! that out later. Getting this backwards would mean the safe configuration is
//! the one somebody has to remember, and the observed default in every system
//! that does it that way is "nobody remembered".

use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};
use utoipa::ToSchema;

use crate::redaction::RedactionRecord;

/// The default life of archived content, in days.
///
/// Thirty rather than "forever": the archive exists so a corpus can be built,
/// and a corpus is built from an export, which is immutable and keeps what it
/// selected. Content that has been through an export has done its job.
pub const DEFAULT_TTL_DAYS: u32 = 30;

/// The longest life a deployment allows by default. A year is already past
/// what most consent texts say.
pub const DEFAULT_MAX_TTL_DAYS: u32 = 365;

/// What makes keeping this lawful, in the producer's own words.
///
/// aiwatcher cannot check any of it and does not pretend to. What it can do is
/// refuse to hold content that never claimed one, and record the claim
/// verbatim so that an auditor asking "why was this row eligible" gets the
/// answer somebody actually gave rather than one reconstructed afterwards.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LawfulBasis {
    /// Nobody said. The default, and an export excludes it by name.
    #[default]
    Unknown,
    /// A person agreed, and `reference` says where that is recorded.
    Consent,
    /// Performing a contract with the person whose data this is.
    Contract,
    /// The producer's own assessment, with `reference` naming it.
    LegitimateInterest,
    /// Nobody's data: a generated fixture, a load test, a benchmark.
    Synthetic,
}

impl LawfulBasis {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Consent => "consent",
            Self::Contract => "contract",
            Self::LegitimateInterest => "legitimate_interest",
            Self::Synthetic => "synthetic",
        }
    }

    /// Whether this basis was stated at all.
    #[must_use]
    pub const fn is_stated(self) -> bool {
        !matches!(self, Self::Unknown)
    }
}

/// What the consent covers.
///
/// Separate scopes rather than one "may train" flag, because they are
/// genuinely different permissions and the difference is where the accidents
/// happen: an evaluation set that quietly becomes a training set is the same
/// mistake as a research-licensed corpus in a commercial model, one field over.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrainingScope {
    /// Fitting model weights.
    Train,
    /// Measuring a model. Narrower, and the one people mean when they say "we
    /// only look at it".
    Evaluate,
    /// Leaving this deployment: a published corpus, a vendor, a partner.
    Share,
}

impl TrainingScope {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Train => "train",
            Self::Evaluate => "evaluate",
            Self::Share => "share",
        }
    }
}

/// Who this is about, on what basis, and what that permits.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct ConsentRecord {
    /// Whose data this is: an account id, a tenant, `synthetic`. Not a name —
    /// a pseudonymous handle is what makes an erasure request answerable
    /// without the archive holding a second copy of the identity.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub subject: String,
    #[serde(default)]
    pub basis: LawfulBasis,
    /// Where the record lives: a ticket, a policy id, a URL. Free text on
    /// purpose — the shape differs per organisation and a schema here would
    /// only be worked around.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reference: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    #[schema(value_type = Option<String>)]
    pub granted_at: Option<OffsetDateTime>,
    /// Empty means nothing was permitted, not everything. An export demanding
    /// [`TrainingScope::Train`] excludes it by name.
    #[serde(default)]
    pub scope: Vec<TrainingScope>,
}

impl ConsentRecord {
    #[must_use]
    pub fn permits(&self, scope: TrainingScope) -> bool {
        self.scope.contains(&scope)
    }

    /// Everything missing from this record, as sentences.
    ///
    /// Every problem rather than the first — a producer fixing one per round
    /// trip learns to send a basis it does not mean.
    #[must_use]
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if !self.basis.is_stated() {
            problems.push(
                "policy.consent.basis is required in a protected deployment: one of consent, \
                 contract, legitimate_interest, synthetic"
                    .to_owned(),
            );
        }
        if self.subject.is_empty() {
            problems.push(
                "policy.consent.subject is required in a protected deployment: an erasure \
                 request has to be answerable"
                    .to_owned(),
            );
        }
        if self.reference.is_empty() {
            problems.push(
                "policy.consent.reference is required in a protected deployment: name the \
                 record this claim comes from"
                    .to_owned(),
            );
        }
        if self.scope.is_empty() {
            problems.push(
                "policy.consent.scope is required in a protected deployment: an empty scope \
                 permits nothing"
                    .to_owned(),
            );
        }
        problems
    }
}

/// How long the content may be held, on a clock of its own.
///
/// Deliberately unrelated to the event log's retention. The log holds
/// operational fields and is sized for a volume; this holds somebody's words
/// and is bounded by what they were told. Tying them together would mean
/// raising the log's retention silently extends a consent nobody re-asked for.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct RetentionPolicy {
    pub ttl_days: u32,
    /// Which written policy this is. Recorded, never interpreted.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub policy_id: String,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            ttl_days: DEFAULT_TTL_DAYS,
            policy_id: String::new(),
        }
    }
}

impl RetentionPolicy {
    /// When content received now stops being readable.
    ///
    /// A `ttl_days` of zero means "expire immediately", which is a legitimate
    /// thing to ask for and produces a turn whose head is written and whose
    /// content the next sweep removes. It does not mean "never expire": that
    /// is not expressible here on purpose.
    #[must_use]
    pub fn expires_at(&self, received_at: OffsetDateTime) -> OffsetDateTime {
        received_at + Duration::days(i64::from(self.ttl_days))
    }
}

/// Everything a producer asserts about one turn.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct ContentPolicy {
    #[serde(default)]
    pub consent: ConsentRecord,
    #[serde(default)]
    pub retention: RetentionPolicy,
    /// What the producer's own redaction hook removed. `None` means none ran,
    /// which a protected deployment refuses — see [`ArchivePolicy::check`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redaction: Option<RedactionRecord>,
}

/// How strict this deployment is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PolicyMode {
    /// Consent provenance and a redaction record are required. The default.
    #[default]
    Protected,
    /// Whatever arrives is recorded as it arrived, with the gaps left visible.
    /// An export then excludes those rows by name, in a manifest, forever —
    /// the annotation registry's `UsageRights::Unknown` rule, restated.
    Open,
}

impl PolicyMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Protected => "protected",
            Self::Open => "open",
        }
    }
}

/// What this deployment demands, and the only thing here that outranks a
/// caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct ArchivePolicy {
    pub mode: PolicyMode,
    /// A `ttl_days` above this is clamped rather than refused, and the clamp is
    /// recorded on the turn. Refusing would push a producer to send the number
    /// this deployment happens to accept, which is a worse record than the
    /// truth plus a clamp.
    pub max_ttl_days: u32,
    /// Whether the server's own scan refusing content is fatal.
    ///
    /// Off by default, and the reason is worth stating: a scanner is a
    /// heuristic, and one that rejects a write throws away the only copy of an
    /// exchange because a hex string looked like a key. A finding always blocks
    /// the *export* — see [`crate::review`] — which is the gate that matters.
    pub reject_on_finding: bool,
}

impl Default for ArchivePolicy {
    fn default() -> Self {
        Self {
            mode: PolicyMode::Protected,
            max_ttl_days: DEFAULT_MAX_TTL_DAYS,
            reject_on_finding: false,
        }
    }
}

impl ArchivePolicy {
    /// Every reason this deployment will not take this turn.
    ///
    /// Empty means it will. Returns all of them at once, which is the same
    /// contract the annotation registry's validation keeps.
    #[must_use]
    pub fn check(&self, policy: &ContentPolicy) -> Vec<String> {
        if self.mode == PolicyMode::Open {
            return Vec::new();
        }
        let mut problems = policy.consent.problems();
        if policy.redaction.is_none() {
            problems.push(
                "policy.redaction is required in a protected deployment: name the hook that \
                 ran, even if it removed nothing"
                    .to_owned(),
            );
        }
        problems
    }

    /// The retention this deployment will actually apply, and whether it had to
    /// shorten what was asked for.
    #[must_use]
    pub fn clamp(&self, retention: &RetentionPolicy) -> (RetentionPolicy, bool) {
        if retention.ttl_days <= self.max_ttl_days {
            return (retention.clone(), false);
        }
        (
            RetentionPolicy {
                ttl_days: self.max_ttl_days,
                policy_id: retention.policy_id.clone(),
            },
            true,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete() -> ContentPolicy {
        ContentPolicy {
            consent: ConsentRecord {
                subject: "tenant-17".to_owned(),
                basis: LawfulBasis::Consent,
                reference: "https://example.invalid/policies/training".to_owned(),
                granted_at: None,
                scope: vec![TrainingScope::Train],
            },
            retention: RetentionPolicy::default(),
            redaction: Some(RedactionRecord::named("acme-scrubber@2.1")),
        }
    }

    #[test]
    fn a_protected_deployment_reports_every_missing_field_at_once() {
        let policy = ArchivePolicy::default();
        let problems = policy.check(&ContentPolicy::default());
        // Four consent fields plus the redaction record. One round trip, not
        // five.
        assert_eq!(problems.len(), 5, "{problems:?}");
        assert!(policy.check(&complete()).is_empty());
    }

    #[test]
    fn an_open_deployment_takes_what_it_is_given_and_leaves_the_gaps_visible() {
        let policy = ArchivePolicy {
            mode: PolicyMode::Open,
            ..ArchivePolicy::default()
        };
        assert!(policy.check(&ContentPolicy::default()).is_empty());
        // And the gap is still a gap: the basis is `unknown`, which is what an
        // export excludes on.
        assert!(!ContentPolicy::default().consent.basis.is_stated());
    }

    #[test]
    fn the_strict_mode_is_the_default_rather_than_the_one_to_remember() {
        assert_eq!(ArchivePolicy::default().mode, PolicyMode::Protected);
        assert_eq!(LawfulBasis::default(), LawfulBasis::Unknown);
        assert!(ConsentRecord::default().scope.is_empty());
    }

    #[test]
    fn an_empty_scope_permits_nothing_rather_than_everything() {
        let consent = ConsentRecord::default();
        for scope in [
            TrainingScope::Train,
            TrainingScope::Evaluate,
            TrainingScope::Share,
        ] {
            assert!(!consent.permits(scope), "{scope:?}");
        }
    }

    #[test]
    fn a_retention_past_the_deployments_ceiling_is_shortened_rather_than_refused() {
        let policy = ArchivePolicy::default();
        let (clamped, was_clamped) = policy.clamp(&RetentionPolicy {
            ttl_days: 10_000,
            policy_id: "p-1".to_owned(),
        });
        assert!(was_clamped);
        assert_eq!(clamped.ttl_days, DEFAULT_MAX_TTL_DAYS);
        // The policy id survives: what was asked for is still recorded.
        assert_eq!(clamped.policy_id, "p-1");

        let (kept, was_clamped) = policy.clamp(&RetentionPolicy::default());
        assert!(!was_clamped);
        assert_eq!(kept.ttl_days, DEFAULT_TTL_DAYS);
    }

    #[test]
    fn expiry_is_measured_from_arrival_rather_than_from_the_producers_clock() {
        let received = OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("valid");
        let retention = RetentionPolicy {
            ttl_days: 2,
            policy_id: String::new(),
        };
        assert_eq!(retention.expires_at(received), received + Duration::days(2));
    }
}
