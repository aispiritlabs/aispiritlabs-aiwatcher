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
//! The words are the proposer's to supply, or the result's to give. What a person
//! using the application said is theirs, so a proposal says which it is —
//! `written` by a reviewer, or `observed` and copied — and only an admin
//! approves an observed one into a dataset. A case of a published result holds
//! its own words: the question its cohort asked and what the variant answered,
//! both this deployment's data already, so a proposal naming where the case sits
//! is filled from there and is `measured`. A trace holds none — the Collector
//! keeps a prompt and a completion off it — and the conversation archive is not
//! a source: its words leave the seal, retention and erasure only through a
//! corpus export.

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
    /// Read from a published result's case: the question its cohort asked and
    /// what the variant answered. Neither is somebody's words that were not
    /// this deployment's data already.
    Measured,
}

/// What somebody proposes as a case.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CaseProposal {
    /// The curation dataset the case would join.
    pub dataset: String,
    /// Where it was seen.
    pub target: AssessmentTarget,
    /// The question, in the proposer's words or copied. Absent to have it read
    /// from the result a case target names, at `at`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    /// What was answered, when the proposer has it. Read with the question when
    /// that is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    /// Where the case sits in its result: the cursor `GET
    /// /evaluation-results/{id}/cases` issued, as a comparison row carries it.
    /// Opaque, handed back as it came; never a search through the result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// Why, in the proposer's words: the feedback, or what went wrong.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// The standing judgement that raised it, when one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assessment: Option<String>,
    /// Whose words the question is. Required when the proposal carries one;
    /// words read from a result are `measured` whatever this says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<ReviewContent>,
    /// The split the case joins in its dataset — `test`, `dev`, whatever that
    /// dataset calls them. A row names it in a `split` column, and a cohort of a
    /// split takes that split's rows. Absent, the case names none, and joins the
    /// cohort of every split, as every row of a dataset without the column does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split: Option<String>,
}

/// The words a proposal starts with, as the registry settled them: the
/// proposer's, or a result's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Words {
    pub question: String,
    pub answer: Option<String>,
    pub content: ReviewContent,
}

/// The proposals under way on one target, whichever dataset each would join.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = CaseReviewsOfTarget)]
pub struct TargetReviews {
    pub target: AssessmentTarget,
    /// Oldest proposal first.
    pub items: Vec<ReviewItem>,
}

/// One entry of the target index: where a proposal lives.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Indexed {
    dataset: String,
    id: String,
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
    /// The split it joins, written in the row it becomes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split: Option<String>,
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
    /// Write what the answer should have been, and which split the case
    /// joins when that changes too. An approved proposal edited is approved no
    /// longer: what was approved is not what it now says.
    Expect {
        expected: String,
        /// Replaces the split the proposal named; absent keeps it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        split: Option<String>,
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

/// A split's name: one short word a dataset row and a cohort both carry.
fn split_name(value: &str, field: &str) -> Result<()> {
    require(
        !value.is_empty()
            && value.len() <= 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')),
        field,
        "must be a split's name: 1 to 64 letters, digits, `_`, `-` or `.`",
    )
}

async fn current(store: &Store, dataset: &str, id: &str) -> Result<Option<ReviewItem>> {
    crate::scope::component(id, "review.id")?;
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

/// The review already under way for this dataset and target, if one is.
pub(crate) async fn existing(
    store: &Store,
    dataset: &str,
    target: &AssessmentTarget,
) -> Result<Option<ReviewItem>> {
    text(dataset, "dataset")?;
    let found = current(store, dataset, &proposal_id(dataset, target)?).await?;
    if let Some(item) = &found {
        index(store, item).await?;
    }
    Ok(found)
}

/// Point the proposal's target at it. Written after the revision, and again on
/// every later proposal of the same target, so a crash between the two is
/// mended by the next person to propose it.
async fn index(store: &Store, item: &ReviewItem) -> Result<()> {
    store
        .create(
            &store::review_target(&item.target.address()?, &item.dataset),
            &Indexed {
                dataset: item.dataset.clone(),
                id: item.id.clone(),
            },
        )
        .await?;
    Ok(())
}

/// Every proposal seen on one target, at its current revision, oldest first.
pub(crate) async fn for_target(store: &Store, target: &AssessmentTarget) -> Result<TargetReviews> {
    let mut items = Vec::new();
    for entry in store
        .0
        .list(&store::review_targets(&target.address()?))
        .await?
    {
        if let Some(indexed) = store.read::<Indexed>(&entry.key).await?
            && let Some(item) = current(store, &indexed.dataset, &indexed.id).await?
        {
            items.push(item);
        }
    }
    items.sort_by(|a, b| a.proposed_at.cmp(&b.proposed_at).then(a.id.cmp(&b.id)));
    Ok(TargetReviews {
        target: target.clone(),
        items,
    })
}

/// Propose a case; answers the review and whether this proposal started it.
pub(crate) async fn propose(
    store: &Store,
    proposal: &CaseProposal,
    words: Words,
    proposed_by: &str,
    now: i64,
) -> Result<(ReviewItem, bool)> {
    text(&proposal.dataset, "dataset")?;
    bounded(&words.question, "question")?;
    if let Some(answer) = &words.answer {
        bounded(answer, "answer")?;
    }
    require(
        proposal.note.len() <= MAX_TEXT_BYTES,
        "note",
        "must be at most 8 KiB of text",
    )?;
    if let Some(split) = &proposal.split {
        split_name(split, "split")?;
    }
    let id = proposal_id(&proposal.dataset, &proposal.target)?;
    if let Some(existing) = current(store, &proposal.dataset, &id).await? {
        index(store, &existing).await?;
        return Ok((existing, false));
    }
    let item = ReviewItem {
        id: id.clone(),
        dataset: proposal.dataset.clone(),
        target: proposal.target.clone(),
        question: words.question,
        answer: words.answer,
        note: proposal.note.clone(),
        assessment: proposal.assessment.clone(),
        content: words.content,
        split: proposal.split.clone(),
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
        index(store, &item).await?;
        return Ok((item, true));
    }
    let existing = current(store, &proposal.dataset, &id)
        .await?
        .ok_or(EvaluationError::Contested)?;
    index(store, &existing).await?;
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
        ReviewAction::Expect { expected, split } => {
            bounded(expected, "expected")?;
            if let Some(split) = split {
                split_name(split, "split")?;
            }
            let split = split.clone().or_else(|| item.split.clone());
            if item.expected.as_deref() == Some(expected.as_str())
                && item.split == split
                && item.state == ReviewState::Ready
            {
                return Ok(None);
            }
            next.split = split;
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
                if current.published_in.as_deref() != Some(version) {
                    return Err(EvaluationError::Contested);
                }
                done = Some(current);
                break;
            }
            // The dataset contains this approved snapshot. A later edit or
            // rejection must never be marked as if those new words were in it.
            if &current != item {
                return Err(EvaluationError::Contested);
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
            split: None,
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
                split: None,
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
    fn moving_a_case_to_another_split_takes_its_approval_away_and_a_bad_name_is_refused() {
        let approved = ReviewItem {
            split: Some("test".into()),
            ..item(ReviewState::Approved, ReviewContent::Written)
        };
        let kept = acted(
            &approved,
            &ReviewAction::Expect {
                expected: "Nairobi".into(),
                split: None,
            },
            "grace",
            false,
            2,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            (kept.split.as_deref(), kept.state),
            (Some("test"), ReviewState::Ready),
            "no split named keeps the one it had"
        );
        let moved = acted(
            &kept,
            &ReviewAction::Expect {
                expected: "Nairobi".into(),
                split: Some("dev".into()),
            },
            "grace",
            false,
            3,
        )
        .unwrap()
        .expect("another split is a change");
        assert_eq!(moved.split.as_deref(), Some("dev"));
        assert!(
            acted(
                &moved,
                &ReviewAction::Expect {
                    expected: "Nairobi".into(),
                    split: Some("dev".into()),
                },
                "grace",
                false,
                4,
            )
            .unwrap()
            .is_none(),
            "saying the same again is no revision"
        );
        let refused = acted(
            &moved,
            &ReviewAction::Expect {
                expected: "Nairobi".into(),
                split: Some("held out".into()),
            },
            "grace",
            false,
            4,
        )
        .unwrap_err();
        assert!(refused.to_string().contains("split's name"), "{refused}");
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
