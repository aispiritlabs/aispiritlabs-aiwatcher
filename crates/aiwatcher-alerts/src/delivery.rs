//! One notification, from the moment it was created to the moment it stopped
//! being tried.
//!
//! ```text
//!   created ──► queued ──► completed      the receiver took it
//!                 │  ▲
//!                 │  └── available_at + backoff, while attempts remain
//!                 └────► failed           the budget ran out, or a refusal
//! ```
//!
//! The state machine is [`aiwatcher_jobs::JobState`] and the decision that
//! moves it is [`aiwatcher_jobs::after_failure`] — called, never copied. What
//! belongs here is the one thing a shard job has no opinion about: **how long
//! a retry waits**, because a receiver that answered 503 is asking for a
//! moment and a shard nobody read is not.
//!
//! Retryability is the adapter's word, through
//! [`PortError`](aiwatcher_core::ports::PortError): `Unavailable` is worth
//! coming back for, `Rejected` will be just as refused on the third attempt.
//! Getting that backwards either retries a 400 until the budget is gone or
//! discards a notification because a proxy hiccuped.

use aiwatcher_jobs::{JobState, after_failure};
use serde::{Deserialize, Serialize};

use crate::rule::RuleName;
use crate::signal::{AlertFact, AlertLink, TriggerKind};

/// How long a failed attempt waits before the next one, per attempt already
/// spent.
///
/// Half a minute, then five. The budget is three attempts
/// ([`aiwatcher_jobs::MAX_ATTEMPTS`]), so a channel that is down for longer
/// than about six minutes fails the delivery rather than holding it — which is
/// the honest outcome: the record says `failed` with the last error on it, the
/// history shows it, and somebody sends it again by hand. A queue that waited
/// for ever would report a healthy channel to anyone who did not look.
pub const RETRY_BACKOFF_SECONDS: [i64; 2] = [30, 300];

/// How long the next attempt waits, given how many have already been spent.
#[must_use]
pub fn backoff_seconds(attempts: u32) -> i64 {
    let index = attempts.saturating_sub(1) as usize;
    RETRY_BACKOFF_SECONDS
        .get(index)
        .copied()
        .or_else(|| RETRY_BACKOFF_SECONDS.last().copied())
        .unwrap_or(0)
}

/// What goes to the receiver.
///
/// Flat and `snake_case`, like the event envelope and the rerun body, so a
/// producer already parsing aiwatcher events needs no second vocabulary. It
/// carries `dedup_key` because this does not promise exactly-once: a receiver
/// that stores the key and ignores a repeat gets the guarantee this cannot
/// give it.
#[derive(Clone, Debug, Deserialize, Serialize, utoipa::ToSchema)]
#[schema(as = AlertPayload)]
pub struct AlertPayload {
    pub dedup_key: String,
    pub rule: RuleName,
    pub rule_version: String,
    pub trigger: TriggerKind,
    /// What it happened to: an execution id, an evaluation result id.
    pub subject: String,
    pub title: String,
    /// The rule's own sentence about why somebody wanted to hear this.
    pub description: String,
    pub occurred_at: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<AlertFact>,
    /// Paths on this instance, never absolute URLs — a notification carrying a
    /// link to a host this process guessed is worse than one carrying a path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<AlertLink>,
}

/// One notification's record: the queue row and the history row, which are the
/// same object because a delivery's history *is* what became of it.
#[derive(Clone, Debug, Deserialize, Serialize, utoipa::ToSchema)]
#[schema(as = AlertDelivery)]
pub struct Delivery {
    /// `sha256(rule version ‖ occurrence)`, and the key this is stored under.
    pub dedup_key: String,
    pub rule: RuleName,
    pub rule_version: String,
    pub state: JobState,
    /// Attempts already spent. Zero until the first one is made.
    pub attempts: u32,
    pub created_at: i64,
    /// Not before this. Moved by the backoff after a retryable failure.
    pub available_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered_at: Option<i64>,
    /// Why the last attempt did not land. Kept on a delivered row too, so a
    /// receiver that took it on the third attempt still shows what the first
    /// two hit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub payload: AlertPayload,
}

/// The longest an error is kept on a record.
///
/// A store's message can carry a whole response body, and a history row is
/// read by a person rather than parsed.
const MAX_ERROR_BYTES: usize = 500;

impl Delivery {
    #[must_use]
    pub fn new(payload: AlertPayload, now: i64) -> Self {
        Self {
            dedup_key: payload.dedup_key.clone(),
            rule: payload.rule.clone(),
            rule_version: payload.rule_version.clone(),
            state: JobState::Queued,
            attempts: 0,
            created_at: now,
            available_at: now,
            delivered_at: None,
            last_error: None,
            payload,
        }
    }

    /// Whether the drain should try this one now.
    #[must_use]
    pub const fn is_due(&self, now: i64) -> bool {
        matches!(self.state, JobState::Queued) && self.available_at <= now
    }

    /// The receiver took it.
    pub const fn sent(&mut self, now: i64) {
        self.attempts += 1;
        self.state = JobState::Completed;
        self.delivered_at = Some(now);
    }

    /// An attempt did not land. Requeued with a backoff while the budget and
    /// the error allow, failed otherwise.
    pub fn attempt_failed(&mut self, error: &str, retryable: bool, now: i64) {
        self.attempts += 1;
        self.last_error = Some(truncate(error));
        self.state = after_failure(self.attempts, retryable);
        if matches!(self.state, JobState::Queued) {
            self.available_at = now + backoff_seconds(self.attempts);
        }
    }

    /// Somebody looked at a failed delivery and asked for it again.
    ///
    /// The budget starts over, which is the point: a channel that was down for
    /// an hour is a different fact from a payload the receiver refuses, and
    /// the difference is that a person decided this one was worth retrying.
    pub const fn requeue(&mut self, now: i64) {
        self.state = JobState::Queued;
        self.attempts = 0;
        self.available_at = now;
    }
}

fn truncate(error: &str) -> String {
    let cleaned: String = error
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if cleaned.len() <= MAX_ERROR_BYTES {
        return cleaned;
    }
    let mut end = MAX_ERROR_BYTES;
    while end > 0 && !cleaned.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &cleaned[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delivery() -> Delivery {
        Delivery::new(
            AlertPayload {
                dedup_key: "key".to_owned(),
                rule: RuleName::parse("nightly").expect("a name"),
                rule_version: "v1".to_owned(),
                trigger: TriggerKind::ExecutionFailed,
                subject: "exec-1".to_owned(),
                title: "it failed".to_owned(),
                description: "the nightly import must not die quietly".to_owned(),
                occurred_at: 0,
                facts: Vec::new(),
                links: Vec::new(),
            },
            100,
        )
    }

    #[test]
    fn a_new_delivery_is_due_at_once() {
        let delivery = delivery();
        assert!(delivery.is_due(100));
        assert_eq!(delivery.attempts, 0);
    }

    #[test]
    fn an_outage_is_retried_after_a_backoff_and_is_not_due_before_it() {
        let mut delivery = delivery();
        delivery.attempt_failed("the receiver is down", true, 100);
        assert_eq!(delivery.state, JobState::Queued);
        assert_eq!(delivery.available_at, 130);
        assert!(!delivery.is_due(129));
        assert!(delivery.is_due(130));

        delivery.attempt_failed("still down", true, 130);
        assert_eq!(delivery.available_at, 430, "the second wait is longer");
    }

    #[test]
    fn a_refusal_fails_immediately_rather_than_spending_the_budget() {
        let mut delivery = delivery();
        delivery.attempt_failed("400 the body is not what I take", false, 100);
        assert_eq!(delivery.state, JobState::Failed);
        assert_eq!(delivery.attempts, 1);
    }

    #[test]
    fn the_budget_runs_out_and_the_record_says_what_kept_failing() {
        let mut delivery = delivery();
        for at in [100, 130, 430] {
            delivery.attempt_failed("the receiver is down", true, at);
        }
        assert_eq!(delivery.state, JobState::Failed);
        assert_eq!(delivery.attempts, aiwatcher_jobs::MAX_ATTEMPTS);
        assert_eq!(
            delivery.last_error.as_deref(),
            Some("the receiver is down"),
            "the history says why, rather than that it stopped"
        );
    }

    #[test]
    fn a_hand_retry_starts_the_budget_over_and_keeps_what_went_wrong() {
        let mut delivery = delivery();
        delivery.attempt_failed("410 gone", false, 100);
        delivery.requeue(900);
        assert_eq!(delivery.state, JobState::Queued);
        assert_eq!(delivery.attempts, 0);
        assert!(delivery.is_due(900));
        assert!(delivery.last_error.is_some());
    }

    #[test]
    fn a_delivered_record_keeps_the_attempts_that_missed() {
        let mut delivery = delivery();
        delivery.attempt_failed("502", true, 100);
        delivery.sent(130);
        assert_eq!(delivery.state, JobState::Completed);
        assert_eq!(delivery.attempts, 2);
        assert_eq!(delivery.delivered_at, Some(130));
        assert_eq!(delivery.last_error.as_deref(), Some("502"));
    }

    #[test]
    fn an_error_longer_than_a_history_row_is_cut_rather_than_stored_whole() {
        let mut delivery = delivery();
        delivery.attempt_failed(&"x".repeat(2_000), true, 100);
        let kept = delivery.last_error.expect("an error");
        assert!(kept.len() <= MAX_ERROR_BYTES + 3);
        assert!(kept.ends_with('…'));
    }

    #[test]
    fn a_control_character_never_reaches_a_history_row() {
        let mut delivery = delivery();
        delivery.attempt_failed("bad\nthings\r\thappened", true, 100);
        assert_eq!(delivery.last_error.as_deref(), Some("bad things  happened"));
    }
}
