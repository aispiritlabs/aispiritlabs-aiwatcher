//! The human gate.
//!
//! An export defaults to refusing anything nobody has looked at. That is the
//! same rule `aiwatcher_annotations` keeps for model-proposed shapes —
//! `require_human_review` — and it is here for a stronger reason: a proposed
//! polygon that nobody checked produces a slightly worse model, and an
//! unreviewed conversation turn produces a model that has memorised somebody's
//! address.
//!
//! Review state lives in the head, beside the findings, exactly as an
//! annotation's review state lives in its head and a prompt's label lives in
//! its. It is the one field on an archived turn that changes after the write,
//! which is why the head and the content are separate objects: approving a
//! turn must not rewrite the ciphertext.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::export::format::PreferenceLabel;
use crate::redaction::{Finding, FindingKind};
use crate::{Error, Result, validate_name};

/// Whether a human has decided about this turn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = TurnReviewState)]
pub enum ReviewState {
    /// Nobody has looked. The default, and what an export excludes by name.
    #[default]
    Pending,
    /// A person read it and said it may be trained on.
    Approved,
    /// A person read it and said it may not. Kept rather than deleted: the
    /// decision is the record, and a rejected turn deleted on the spot is one
    /// the next import re-adds as pending.
    Rejected,
}

impl ReviewState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }
}

/// Who decided, when, and why.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct TurnReview {
    #[serde(default)]
    pub state: ReviewState,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reviewer: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    #[schema(value_type = Option<String>)]
    pub decided_at: Option<OffsetDateTime>,
    /// Which of two answers was better, where a reviewer said.
    ///
    /// A different axis from [`Self::state`], and keeping them separate is the
    /// point. `state` answers "may this content be used at all" — the safety
    /// gate. This answers "was it a good answer" — the quality one. A
    /// preference pair is built from two turns that both passed the first and
    /// disagree on the second, which is what stops a turn rejected for holding
    /// somebody's address becoming the rejected half of a preference pair and
    /// putting that address in the corpus anyway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preference: Option<PreferenceLabel>,
}

impl TurnReview {
    #[must_use]
    pub fn is_approved(&self) -> bool {
        self.state == ReviewState::Approved
    }
}

/// One reviewer's decision about one turn.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = TurnReviewRequest)]
pub struct ReviewRequest {
    pub state: ReviewState,
    #[serde(default)]
    pub note: String,
    /// The quality judgement, where the reviewer is making one. Only a turn
    /// this is set on can take part in a preference export, and nothing infers
    /// it — a rejection has many reasons and only one of them is "the other
    /// answer was better".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preference: Option<PreferenceLabel>,
    /// What the reviewer saw that the scanner did not — an unsafe answer, a
    /// name the rules do not recognise. Appended to the turn's findings rather
    /// than replacing them: a scanner finding a human disagreed with is a fact
    /// about both of them.
    #[serde(default)]
    pub findings: Vec<HumanFinding>,
}

/// A finding a person recorded.
///
/// Deliberately narrower than [`Finding`]: no byte offsets, because a reviewer
/// is describing a judgement rather than a match, and no `found_by`, because
/// the registry fills that in from the caller's identity. A client that could
/// name its own reviewer could file somebody else's approval.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct HumanFinding {
    pub kind: FindingKind,
    pub rule: String,
}

impl ReviewRequest {
    /// # Errors
    ///
    /// [`Error::Invalid`] when a note or a rule name is unusable, and
    /// [`Error::Invalid`] when a rejection carries no reason at all — a
    /// rejected turn nobody explained is one the next reviewer re-opens.
    pub fn validate(&self) -> Result<()> {
        if self.note.len() > crate::MAX_NAME_BYTES * 8 {
            return Err(Error::Invalid("the review note is too long".to_owned()));
        }
        for finding in &self.findings {
            validate_name(&finding.rule, "a finding's rule")?;
        }
        if self.state == ReviewState::Rejected && self.note.is_empty() && self.findings.is_empty() {
            return Err(Error::Invalid(
                "a rejection needs a note or a finding: the reason is the record".to_owned(),
            ));
        }
        Ok(())
    }

    /// The reviewer's findings, attributed to whoever is making the request.
    #[must_use]
    pub fn attributed(&self, reviewer: &str) -> Vec<Finding> {
        self.findings
            .iter()
            .map(|finding| Finding::about_turn(finding.kind, finding.rule.clone(), reviewer))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(state: ReviewState) -> ReviewRequest {
        ReviewRequest {
            state,
            note: String::new(),
            preference: None,
            findings: Vec::new(),
        }
    }

    #[test]
    fn nobody_looking_is_the_default_rather_than_approval() {
        assert_eq!(TurnReview::default().state, ReviewState::Pending);
        assert!(!TurnReview::default().is_approved());
    }

    #[test]
    fn a_rejection_with_no_reason_is_refused() {
        assert!(request(ReviewState::Rejected).validate().is_err());

        let mut with_note = request(ReviewState::Rejected);
        with_note.note = "the customer's address is in the answer".to_owned();
        assert!(with_note.validate().is_ok());

        // An approval needs no note: the common case is "this is fine", and a
        // gate that demands prose for it is a gate people click through.
        assert!(request(ReviewState::Approved).validate().is_ok());
    }

    #[test]
    fn the_safety_decision_and_the_quality_one_are_separate_fields() {
        // A turn can be safe to use and a worse answer, which is exactly the
        // rejected half of a preference pair. If the two were one field, that
        // half would be indistinguishable from a turn rejected for holding
        // somebody's address.
        let review = TurnReview {
            state: ReviewState::Approved,
            preference: Some(PreferenceLabel::Rejected),
            ..TurnReview::default()
        };
        assert!(review.is_approved());
        assert_eq!(review.preference, Some(PreferenceLabel::Rejected));
    }

    #[test]
    fn a_reviewers_finding_is_attributed_to_the_reviewer_rather_than_to_the_client() {
        let mut review = request(ReviewState::Rejected);
        review.findings = vec![HumanFinding {
            kind: FindingKind::Unsafe,
            rule: "self-harm".to_owned(),
        }];
        let attributed = review.attributed("ada@example.com");
        assert_eq!(attributed.len(), 1);
        assert_eq!(attributed[0].found_by, "ada@example.com");
        assert_eq!(attributed[0].kind, FindingKind::Unsafe);
        // And a human finding is about the turn, not a byte range in it.
        assert_eq!(attributed[0].part, None);
    }
}
