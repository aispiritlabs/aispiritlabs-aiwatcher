//! The two halves that know what is worth saying, and turn it into a signal.
//!
//! Each reads a source this process already holds, remembers where it stopped,
//! and raises what it passed — in that order, because moving the cursor first
//! would skip an occurrence with nothing able to say which. Raising the same
//! occurrence twice costs nothing: the delivery is named by a digest of it, so
//! the second attempt lands on the object the first one wrote.
//!
//! **A first pass raises nothing.** A deployment that turns alerts on has a
//! history behind it, and paging somebody about last month's failures is the
//! one way to make a new alert channel worse than none. So a watcher with no
//! cursor writes one at where the source is now and stops, which is the same
//! decision the scheduler makes about the slots it never saw.

use std::sync::Arc;

use aiwatcher_alerts::{
    ActiveRule, AlertFact, AlertLink, AlertSignal, AlertTrigger, Registry, Result, TriggerKind,
};
use aiwatcher_evaluation::{Comparability, EvidenceState, GateVerdict, ResultStatus};
use aiwatcher_projector::readmodel::ReadModel;
use aiwatcher_projector::workflows::{ExecutionFilter, ExecutionStatus};

/// Who this asks the evaluation registry as.
///
/// A read of durable evidence names a subject, and the honest one here is this
/// process rather than a person: nobody asked, and a retention report that
/// credited a name would be wrong about who looked.
const SUBJECT: &str = "aiwatcher-alerts";

/// The names the cursors are stored under. The evaluation half keeps two,
/// because the index cursor only moves on a full page and the high-water mark
/// is what stops a short page being re-gated every tick.
const EXECUTION_WATCHER: &str = "execution-failed";
const EVALUATION_INDEX: &str = "evaluation-regressed";
const EVALUATION_SEEN: &str = "evaluation-regressed-seen";

/// How far back a pass looks for a failure it has not seen.
///
/// A day. The read model holds what it holds, so reaching further would find
/// nothing and cost a scan; reaching less far would lose the failures of an
/// outage that outlived one tick.
const EXECUTION_LOOKBACK_SECONDS: i64 = 86_400;

/// How many rows one pass reads from each source.
const BATCH: usize = 200;

/// How many pages deep a baseline is looked for in one context.
///
/// A context is one comparable measurement, so its results are a handful in a
/// deployment measuring nightly and a few hundred in one measuring per commit.
/// Four pages is generous for both, and a baseline further back than that is
/// one nobody is holding anything to.
const BASELINE_PAGES: usize = 4;

/// What one pass did, for the log line and for the tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Watched {
    /// Occurrences that matched a rule and had not been raised before.
    pub raised: usize,
    /// Occurrences this pass considered.
    pub seen: usize,
    /// Whether this pass only wrote a starting cursor.
    pub started: bool,
}

// ── Executions ───────────────────────────────────────────────────────────────

/// Managed executions that reached a terminal failure since the last pass.
///
/// # Errors
///
/// [`AlertError::Store`](aiwatcher_alerts::AlertError::Store) when the alert
/// store is unreachable. The read model is in memory and cannot fail.
pub async fn execution_failures(
    alerts: &Registry,
    read_model: &ReadModel,
    rules: &[ActiveRule],
    now: i64,
) -> Result<Watched> {
    if !rules
        .iter()
        .any(|rule| rule.rule.trigger.kind() == TriggerKind::ExecutionFailed)
    {
        return Ok(Watched::default());
    }
    let Some(since) = alerts
        .cursor(EXECUTION_WATCHER)
        .await?
        .and_then(|at| at.parse::<i64>().ok())
    else {
        alerts
            .set_cursor(EXECUTION_WATCHER, &now.to_string())
            .await?;
        return Ok(Watched {
            started: true,
            ..Watched::default()
        });
    };

    // The instance's own executions, and that is the decision rather than a
    // default. An alert rule is an instance's and there is one channel for the
    // deployment (ADR_0035), so a project's failure raised on it would put one
    // project's work into a webhook every other project's administrator reads.
    // What it costs is that a project's failed execution raises nothing yet,
    // which is named in `docs/iam-02-data-plane.md` rather than discovered.
    let page = read_model
        .workflow_executions(
            aiwatcher_projector::ReadScope::Global,
            &ExecutionFilter {
                window_seconds: Some(EXECUTION_LOOKBACK_SECONDS),
                workflow_id: None,
                status: Some(ExecutionStatus::Failed),
                search: None,
                after: None,
                limit: Some(BATCH),
            },
        )
        .await;

    let mut watched = Watched::default();
    let mut highest = since;
    for row in page.executions {
        let ended_at = row
            .ended_at
            .unwrap_or(row.last_activity_at)
            .unix_timestamp();
        if ended_at < since {
            continue;
        }
        watched.seen += 1;
        highest = highest.max(ended_at);

        let mut facts = vec![AlertFact::new("workflow", &row.workflow_id)];
        if let Some(version) = &row.version {
            facts.push(AlertFact::new("version", version));
        }
        facts.push(AlertFact::new(
            "nodes",
            format!("{} of {} failed", row.nodes_failed, row.nodes_total),
        ));
        if let Some(error) = &row.error {
            facts.push(AlertFact::new("error", error));
        }
        let signal = AlertSignal {
            trigger: TriggerKind::ExecutionFailed,
            subject: row.workflow_run_id.clone(),
            within: Some(row.workflow_id.clone()),
            occurred_at: ended_at,
            title: format!("{} failed", row.workflow_id),
            facts,
            links: vec![AlertLink::new(
                "execution",
                format!("/workflows/executions/{}", row.workflow_run_id),
            )],
        };
        watched.raised += raise(alerts, rules, &signal, now).await?;
    }

    if highest > since {
        // Last, and only over what was raised above. A failure whose second
        // ended in the same second as the cursor is read again next pass and
        // recognised by its key, which is the cheap side of the trade.
        alerts
            .set_cursor(EXECUTION_WATCHER, &highest.to_string())
            .await?;
    }
    Ok(watched)
}

// ── Evaluations ──────────────────────────────────────────────────────────────

/// Evaluation results published since the last pass that the gate calls a
/// regression against the result before them.
///
/// # Errors
///
/// [`AlertError::Store`](aiwatcher_alerts::AlertError::Store) when either
/// store is unreachable, and [`AlertError::Invalid`](aiwatcher_alerts::AlertError::Invalid)
/// for a policy the gate will not apply — which a published rule cannot hold,
/// because the same check refused it where it was written.
pub async fn evaluation_regressions(
    alerts: &Registry,
    evaluations: &Arc<aiwatcher_evaluation::Registry>,
    rules: &[ActiveRule],
    now: i64,
) -> Result<Watched> {
    let rules: Vec<ActiveRule> = rules
        .iter()
        .filter(|rule| rule.rule.trigger.kind() == TriggerKind::EvaluationRegressed)
        .cloned()
        .collect();
    if rules.is_empty() {
        return Ok(Watched::default());
    }

    let index = alerts.cursor(EVALUATION_INDEX).await?;
    let Some(seen_until) = alerts
        .cursor(EVALUATION_SEEN)
        .await?
        .and_then(|at| at.parse::<i64>().ok())
    else {
        return start_evaluation_cursor(alerts, evaluations, now)
            .await
            .map(|()| Watched {
                started: true,
                ..Watched::default()
            });
    };

    let page = evaluations
        .list(index.as_deref(), BATCH, None, None, SUBJECT, now)
        .await
        .map_err(aiwatcher_alerts::AlertError::from)?;

    let mut watched = Watched::default();
    let mut highest = seen_until;
    for candidate in &page.evaluations {
        if candidate.receipt.committed_at < seen_until {
            continue;
        }
        watched.seen += 1;
        highest = highest.max(candidate.receipt.committed_at);
        // A tombstone, a failed measurement or one nothing could score is not
        // a regression: it is a result that cannot say, and calling it worse
        // would be this system inventing a finding.
        if !measurable(candidate) {
            continue;
        }
        let Some(baseline) = previous_of_variant(evaluations, candidate, now).await? else {
            // The first measurement of a variant in a context has nothing to
            // be worse than.
            continue;
        };
        watched.raised +=
            raise_regression(alerts, evaluations, &rules, candidate, &baseline, now).await?;
    }

    if highest > seen_until {
        alerts
            .set_cursor(EVALUATION_SEEN, &highest.to_string())
            .await?;
    }
    if let Some(next) = page.next_cursor {
        alerts.set_cursor(EVALUATION_INDEX, &next).await?;
    }
    Ok(watched)
}

/// Page to where the index ends now, and remember it — raising nothing.
async fn start_evaluation_cursor(
    alerts: &Registry,
    evaluations: &Arc<aiwatcher_evaluation::Registry>,
    now: i64,
) -> Result<()> {
    let mut cursor: Option<String> = None;
    let mut newest = 0i64;
    loop {
        let page = evaluations
            .list(cursor.as_deref(), BATCH, None, None, SUBJECT, now)
            .await
            .map_err(aiwatcher_alerts::AlertError::from)?;
        for row in &page.evaluations {
            newest = newest.max(row.receipt.committed_at);
        }
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    // The high-water mark first: a crash between the two leaves a watcher that
    // re-reads a page it has already passed, which its keys absorb. The other
    // order would leave one that considers nothing published before now.
    alerts
        .set_cursor(EVALUATION_SEEN, &newest.max(now).to_string())
        .await?;
    if let Some(cursor) = cursor {
        alerts.set_cursor(EVALUATION_INDEX, &cursor).await?;
    }
    Ok(())
}

fn measurable(row: &aiwatcher_evaluation::DurableEvaluation) -> bool {
    matches!(row.state, EvidenceState::Complete | EvidenceState::Partial)
        && row.status != Some(ResultStatus::Failed)
}

/// Gate one candidate against one baseline, under every rule that covers it.
async fn raise_regression(
    alerts: &Registry,
    evaluations: &Arc<aiwatcher_evaluation::Registry>,
    rules: &[ActiveRule],
    candidate: &aiwatcher_evaluation::DurableEvaluation,
    baseline: &str,
    now: i64,
) -> Result<usize> {
    let mut raised = 0;
    for rule in rules {
        let AlertTrigger::EvaluationRegressed { context_id, policy } = &rule.rule.trigger else {
            continue;
        };
        if context_id
            .as_deref()
            .is_some_and(|wanted| wanted != candidate.receipt.context_id)
        {
            continue;
        }
        let decision = evaluations
            .gate(
                &candidate.receipt.evaluation_id,
                baseline,
                policy,
                SUBJECT,
                now,
            )
            .await
            .map_err(aiwatcher_alerts::AlertError::from)?;
        let Some(decision) = decision else { continue };
        // Only `regression`. `incomplete` is a measurement that could not say
        // and `error` is a pair that does not compare — neither is a change
        // somebody made, and reporting them here would make the one verdict
        // worth waking up for indistinguishable from a scorer that timed out.
        if decision.verdict != GateVerdict::Regression
            || decision.comparability == Comparability::Incompatible
        {
            continue;
        }

        let mut facts = vec![
            AlertFact::new("variant", &candidate.receipt.variant_id),
            AlertFact::new("baseline", baseline),
        ];
        for metric in decision.metrics.iter().filter(|metric| metric.regressed) {
            let delta = metric
                .delta
                .delta
                .map_or_else(|| "worse".to_owned(), |value| format!("{value:+.4}"));
            facts.push(AlertFact::new(
                format!("{} regressed", metric.delta.name),
                delta,
            ));
        }
        facts.extend(
            decision
                .reasons
                .iter()
                .take(3)
                .map(|reason| AlertFact::new("why", reason)),
        );

        let signal = AlertSignal {
            trigger: TriggerKind::EvaluationRegressed,
            subject: candidate.receipt.evaluation_id.clone(),
            within: Some(candidate.receipt.context_id.clone()),
            occurred_at: candidate.receipt.committed_at,
            title: format!("{} is worse than {baseline}", candidate.receipt.variant_id),
            facts,
            links: vec![AlertLink::new(
                "result",
                format!("/evaluation/results/{}", candidate.receipt.evaluation_id),
            )],
        };
        raised += raise(alerts, std::slice::from_ref(rule), &signal, now).await?;
    }
    Ok(raised)
}

/// The result published before this one, of the same variant in the same
/// context.
///
/// Same context because that is what makes two results comparable at all — it
/// is the content address of the cohort, the split, the suite and the scorers
/// together. Same variant because a different one is a different thing being
/// measured, and calling that a regression would report every experiment as a
/// problem the moment somebody ran one.
async fn previous_of_variant(
    evaluations: &Arc<aiwatcher_evaluation::Registry>,
    candidate: &aiwatcher_evaluation::DurableEvaluation,
    now: i64,
) -> Result<Option<String>> {
    let mut cursor: Option<String> = None;
    let mut previous: Option<String> = None;
    for _ in 0..BASELINE_PAGES {
        let page = evaluations
            .list(
                cursor.as_deref(),
                BATCH,
                None,
                Some(&candidate.receipt.context_id),
                SUBJECT,
                now,
            )
            .await
            .map_err(aiwatcher_alerts::AlertError::from)?;
        for row in &page.evaluations {
            if row.receipt.evaluation_id == candidate.receipt.evaluation_id {
                return Ok(previous);
            }
            if row.receipt.variant_id == candidate.receipt.variant_id && measurable(row) {
                previous = Some(row.receipt.evaluation_id.clone());
            }
        }
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    Ok(previous)
}

async fn raise(
    alerts: &Registry,
    rules: &[ActiveRule],
    signal: &AlertSignal,
    now: i64,
) -> Result<usize> {
    Ok(alerts
        .raise(rules, signal, now)
        .await?
        .iter()
        .filter(|raised| raised.created)
        .count())
}
