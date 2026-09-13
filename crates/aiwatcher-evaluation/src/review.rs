//! From something a person noticed to a case every later run is measured on.
//!
//! Feedback on a trace, a low judgement on a case, a complaint somebody pasted —
//! each is a *proposal*, never ground truth. It becomes a regression case only
//! through people: somebody writes what the answer should have been, somebody
//! approves it, and an approved case joins a curation dataset as a new version,
//! where a cohort can be derived from it like any other. Nothing here copies a
//! judgement into an expectation on its own.
//!
//! A proposal is addressed by the dataset it would join and the thing it was
//! seen on, so proposing the same trace twice lands on the review already
//! under way rather than beside it. Each action is a revision, written
//! create-only, so two reviewers acting at once agree on what happened.
//!
//! The words are the proposer's to supply. What a person using the application
//! said is theirs, so a proposal says which it is — `written` by a reviewer, or
//! `observed` and copied — and only an admin approves an observed one into a
//! dataset. The conversation archive is not a source: its words leave the seal,
//! retention and erasure only through a corpus export.

use serde::{Deserialize, Serialize};

use crate::{
    AssessmentTarget, EvaluationError, Result, SCHEMA_VERSION, digest, require,
    store::{self, Store},
    text,
};

const WRITE_ATTEMPTS: u32 = 4;
const MAX_TEXT_BYTES: usize = 8 * 1024;

/// Whose words a proposal holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = CaseReviewContent)]
pub enum ReviewContent {
    /// A reviewer wrote the question: nobody's words but theirs.
    Written,
    /// Copied from what somebody using the application said. Theirs, so only an
    /// admin decides it becomes a case.
    Observed,
}

/// What somebody proposes as a case.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CaseProposal {
    /// The curation dataset the case would join.
    pub dataset: String,
    /// Where it was seen.
    pub target: AssessmentTarget,
    pub question: String,
    /// What was answered, when the proposer has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    /// Why, in the proposer's words: the feedback, or what went wrong.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// The standing judgement that raised it, when one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assessment: Option<String>,
    pub content: ReviewContent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = CaseReviewState)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    /// Nobody has written what the answer should be.
    Proposed,
    /// An expected answer is written, and nobody has approved it.
    Ready,
    Approved,
    Rejected,
    /// In a dataset version, for good.
    Published,
}

/// One proposal at its current revision.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = CaseReviewItem)]
pub struct ReviewItem {
    pub id: String,
    pub dataset: String,
    pub target: AssessmentTarget,
    pub question: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assessment: Option<String>,
    pub content: ReviewContent,
    pub proposed_by: String,
    pub proposed_at: i64,
    pub state: ReviewState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The dataset version it was published in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_in: Option<String>,
    pub revision: u32,
    pub recorded_by: String,
    pub recorded_at: i64,
}

impl ReviewItem {
    /// The case ID it takes in a dataset: stable, and never another case's.
    #[must_use]
    pub fn case_id(&self) -> String {
        format!("review-{}", &self.id[..16.min(self.id.len())])
    }
}

/// What a reviewer does to a proposal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = CaseReviewAction)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReviewAction {
    /// Write what the answer should have been. An approved proposal edited is
    /// approved no longer: what was approved is not what it now says.
    Expect {
        expected: String,
    },
    Approve,
    Reject {
        reason: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = CaseReviewPage)]
pub struct ReviewPage {
    pub dataset: String,
    /// Oldest proposal first.
    pub items: Vec<ReviewItem>,
}

fn proposal_id(dataset: &str, target: &AssessmentTarget) -> Result<String> {
    digest(&(
        SCHEMA_VERSION,
        "evaluation.review",
        dataset,
        target.address()?,
    ))
}

fn bounded(value: &str, field: &str) -> Result<()> {
    text(value, field)?;
    require(
        value.len() <= MAX_TEXT_BYTES && !value.contains('\0'),
        field,
        "must be at most 8 KiB of text",
    )
}

async fn current(store: &Store, dataset: &str, id: &str) -> Result<Option<ReviewItem>> {
    let mut keys: Vec<String> = store
        .0
        .list(&store::review(dataset, id))
        .await?
        .into_iter()
        .map(|entry| entry.key)
        .collect();
    keys.sort();
    match keys.last() {
        Some(key) => store.read(key).await,
        None => Ok(None),
    }
}

/// Propose a case; answers the review and whether this proposal started it.
pub(crate) async fn propose(
    store: &Store,
    proposal: &CaseProposal,
    proposed_by: &str,
    now: i64,
) -> Result<(ReviewItem, bool)> {
    text(&proposal.dataset, "dataset")?;
    bounded(&proposal.question, "question")?;
    if let Some(answer) = &proposal.answer {
        bounded(answer, "answer")?;
    }
    require(
        proposal.note.len() <= MAX_TEXT_BYTES,
        "note",
        "must be at most 8 KiB of text",
    )?;
    let id = proposal_id(&proposal.dataset, &proposal.target)?;
    if let Some(existing) = current(store, &proposal.dataset, &id).await? {
        return Ok((existing, false));
    }
    let item = ReviewItem {
        id: id.clone(),
        dataset: proposal.dataset.clone(),
        target: proposal.target.clone(),
        question: proposal.question.clone(),
        answer: proposal.answer.clone(),
        note: proposal.note.clone(),
        assessment: proposal.assessment.clone(),
        content: proposal.content,
        proposed_by: proposed_by.to_owned(),
        proposed_at: now,
        state: ReviewState::Proposed,
        expected: None,
        expected_by: None,
        decided_by: None,
        reason: None,
        published_in: None,
        revision: 1,
        recorded_by: proposed_by.to_owned(),
        recorded_at: now,
    };
    if store
        .create(&store::review_revision(&proposal.dataset, &id, 1), &item)
        .await?
    {
        return Ok((item, true));
    }
    let existing = current(store, &proposal.dataset, &id)
        .await?
        .ok_or(EvaluationError::Contested)?;
    Ok((existing, false))
}

/// Act on a proposal. `None` when there is no such proposal in that dataset.
pub(crate) async fn act(
    store: &Store,
    dataset: &str,
    id: &str,
    action: &ReviewAction,
    subject: &str,
    admin: bool,
    now: i64,
) -> Result<Option<ReviewItem>> {
    for _ in 0..WRITE_ATTEMPTS {
        let Some(item) = current(store, dataset, id).await? else {
            return Ok(None);
        };
        let Some(next) = acted(&item, action, subject, admin, now)? else {
            return Ok(Some(item));
        };
        if store
            .create(&store::review_revision(dataset, id, next.revision), &next)
            .await?
        {
            return Ok(Some(next));
        }
    }
    Err(EvaluationError::Contested)
}

/// The revision an action writes, or `None` when the proposal already says it.
fn acted(
    item: &ReviewItem,
    action: &ReviewAction,
    subject: &str,
    admin: bool,
    now: i64,
) -> Result<Option<ReviewItem>> {
    require(
        item.state != ReviewState::Published,
        "state",
        &format!(
            "was published in {}; a published case is part of that version for good",
            item.published_in.as_deref().unwrap_or("a dataset version")
        ),
    )?;
    let mut next = ReviewItem {
        revision: item.revision + 1,
        recorded_by: subject.to_owned(),
        recorded_at: now,
        ..item.clone()
    };
    match action {
        ReviewAction::Expect { expected } => {
            bounded(expected, "expected")?;
            if item.expected.as_deref() == Some(expected.as_str())
                && item.state == ReviewState::Ready
            {
                return Ok(None);
            }
            next.expected = Some(expected.clone());
            next.expected_by = Some(subject.to_owned());
            next.state = ReviewState::Ready;
            next.decided_by = None;
            next.reason = None;
        }
        ReviewAction::Approve => {
            if item.state == ReviewState::Approved {
                return Ok(None);
            }
            require(
                item.state == ReviewState::Ready,
                "state",
                "has no expected answer to approve; write one first",
            )?;
            if item.content == ReviewContent::Observed && !admin {
                return Err(EvaluationError::Unavailable(
                    crate::EvidenceState::Forbidden,
                ));
            }
            next.state = ReviewState::Approved;
            next.decided_by = Some(subject.to_owned());
        }
        ReviewAction::Reject { reason } => {
            bounded(reason, "reason")?;
            if item.state == ReviewState::Rejected && item.reason.as_deref() == Some(reason) {
                return Ok(None);
            }
            next.state = ReviewState::Rejected;
            next.decided_by = Some(subject.to_owned());
            next.reason = Some(reason.clone());
        }
    }
    Ok(Some(next))
}

/// Mark approved proposals published in a dataset version.
pub(crate) async fn published(
    store: &Store,
    items: &[ReviewItem],
    version: &str,
    subject: &str,
    now: i64,
) -> Result<Vec<ReviewItem>> {
    let mut marked = Vec::with_capacity(items.len());
    for item in items {
        let mut done = None;
        for _ in 0..WRITE_ATTEMPTS {
            let Some(current) = current(store, &item.dataset, &item.id).await? else {
                break;
            };
            if current.state == ReviewState::Published {
                done = Some(current);
                break;
            }
            let next = ReviewItem {
                state: ReviewState::Published,
                published_in: Some(version.to_owned()),
                revision: current.revision + 1,
                recorded_by: subject.to_owned(),
                recorded_at: now,
                ..current
            };
            if store
                .create(
                    &store::review_revision(&item.dataset, &item.id, next.revision),
                    &next,
                )
                .await?
            {
                done = Some(next);
                break;
            }
        }
        marked.push(done.ok_or(EvaluationError::Contested)?);
    }
    Ok(marked)
}

/// Every proposal for one dataset, at its current revision, oldest first.
pub(crate) async fn page(store: &Store, dataset: &str) -> Result<ReviewPage> {
    text(dataset, "dataset")?;
    let mut keys: Vec<String> = store
        .0
        .list(&store::reviews(dataset))
        .await?
        .into_iter()
        .map(|entry| entry.key)
        .collect();
    keys.sort();
    let mut items: Vec<ReviewItem> = Vec::new();
    // `…/{id}/{revision}`, zero padded: the last key per id is its current one.
    for (at, key) in keys.iter().enumerate() {
        let group = |key: &str| key.rsplit_once('/').map(|(head, _)| head.to_owned());
        if keys
            .get(at + 1)
            .is_some_and(|next| group(next) == group(key))
        {
            continue;
        }
        if let Some(item) = store.read::<ReviewItem>(key).await? {
            items.push(item);
        }
    }
    items.sort_by(|a, b| a.proposed_at.cmp(&b.proposed_at).then(a.id.cmp(&b.id)));
    Ok(ReviewPage {
        dataset: dataset.to_owned(),
        items,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(state: ReviewState, content: ReviewContent) -> ReviewItem {
        ReviewItem {
            id: "a".repeat(64),
            dataset: "capitals".into(),
            target: AssessmentTarget::Trace {
                trace_id: "t".into(),
            },
            question: "What is the capital of Kenya?".into(),
            answer: Some("Mombasa".into()),
            note: String::new(),
            assessment: None,
            content,
            proposed_by: "ada".into(),
            proposed_at: 1,
            state,
            expected: (state != ReviewState::Proposed).then(|| "Nairobi".into()),
            expected_by: None,
            decided_by: None,
            reason: None,
            published_in: None,
            revision: 1,
            recorded_by: "ada".into(),
            recorded_at: 1,
        }
    }

    #[test]
    fn nothing_is_approved_without_an_expected_answer_and_an_edit_takes_the_approval_away() {
        let proposed = item(ReviewState::Proposed, ReviewContent::Written);
        let refused = acted(&proposed, &ReviewAction::Approve, "grace", false, 2).unwrap_err();
        assert!(refused.to_string().contains("write one first"), "{refused}");

        let approved = item(ReviewState::Approved, ReviewContent::Written);
        let edited = acted(
            &approved,
            &ReviewAction::Expect {
                expected: "Nairobi, Kenya".into(),
            },
            "grace",
            false,
            2,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            edited.state,
            ReviewState::Ready,
            "what was approved is not what it says"
        );
        assert_eq!(edited.revision, 2);
        assert!(
            acted(&approved, &ReviewAction::Approve, "grace", false, 2)
                .unwrap()
                .is_none(),
            "approving again is not a new revision"
        );
    }

    #[test]
    fn somebody_else_s_words_are_approved_only_by_an_admin_and_a_published_case_stays() {
        let observed = item(ReviewState::Ready, ReviewContent::Observed);
        assert!(matches!(
            acted(&observed, &ReviewAction::Approve, "grace", false, 2),
            Err(EvaluationError::Unavailable(
                crate::EvidenceState::Forbidden
            ))
        ));
        assert!(
            acted(&observed, &ReviewAction::Approve, "admin", true, 2)
                .unwrap()
                .is_some()
        );
        let published = ReviewItem {
            published_in: Some("v2".into()),
            ..item(ReviewState::Published, ReviewContent::Written)
        };
        let refused = acted(
            &published,
            &ReviewAction::Reject {
                reason: "changed my mind".into(),
            },
            "grace",
            false,
            2,
        )
        .unwrap_err();
        assert!(refused.to_string().contains("published in v2"), "{refused}");
    }
}
