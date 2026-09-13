//! What a declared variant was observed doing, from the runs that name it.
//!
//! A published result says what a variant scored on a pinned set of cases; a
//! run carrying its `variant_id` is the same variant answering somebody using
//! the application. The two are different samples on different clocks — the
//! evidence outlives the log, this is gone with the run — so they are never
//! folded into one number, and a run the variant made to answer a
//! measurement's case (`evaluation_id` on `run.started`) is counted apart and
//! in no figure here: a benchmark is not an observation.
//!
//! Folded from the read model, like [`crate::dimensions`]: the same window,
//! the same retention, no store of its own.

use serde::Serialize;
use time::OffsetDateTime;

use crate::readmodel::{RunStatus, RunSummary};

/// One variant's runs in the window.
#[derive(Clone, Debug, PartialEq, Serialize, utoipa::ToSchema)]
pub struct VariantObservations {
    pub variant_id: String,
    /// Runs naming the variant that no measurement made.
    pub runs: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub running: u64,
    /// Runs naming it that answered a measurement's cases, left out of every
    /// other figure.
    pub measured_runs: u64,
    /// How long a finished run took, end to end, by nearest rank. A run is one
    /// request answered, so this stands beside a result's per-case latency —
    /// on another clock, over other inputs. Absent when none finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<DurationSummary>,
    pub llm_calls: u64,
    /// What the runs' model calls reported. A call that reported no usage
    /// counts nothing, which the call count beside it lets a reader see.
    pub input_tokens: i64,
    pub output_tokens: i64,
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub first_seen_at: Option<OffsetDateTime>,
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub last_seen_at: Option<OffsetDateTime>,
}

/// Finished runs' durations, by nearest rank, over this many runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct DurationSummary {
    pub runs: usize,
    pub p50: i64,
    pub p90: i64,
    pub p99: i64,
    pub max: i64,
}

impl DurationSummary {
    fn of(mut durations: Vec<i64>) -> Option<Self> {
        durations.sort_unstable();
        let last = durations.len().checked_sub(1)?;
        let rank = |quantile: f64| {
            let at = ((quantile * durations.len() as f64).ceil() as usize).max(1) - 1;
            durations[at.min(last)]
        };
        Some(Self {
            runs: durations.len(),
            p50: rank(0.5),
            p90: rank(0.9),
            p99: rank(0.99),
            max: durations[last],
        })
    }
}

/// One row per variant asked about, in the order asked, whether or not any
/// run named it — "never observed" is an answer, and an absent row would read
/// as a variant nobody asked about.
pub fn compute<'a>(
    runs: impl IntoIterator<Item = &'a RunSummary>,
    variant_ids: &[&str],
    window_seconds: Option<i64>,
    now: OffsetDateTime,
) -> Vec<VariantObservations> {
    let since = crate::window::cutoff(window_seconds, now);
    let mut rows: Vec<(VariantObservations, Vec<i64>)> = variant_ids
        .iter()
        .map(|variant_id| {
            (
                VariantObservations {
                    variant_id: (*variant_id).to_owned(),
                    runs: 0,
                    succeeded: 0,
                    failed: 0,
                    running: 0,
                    measured_runs: 0,
                    duration_ms: None,
                    llm_calls: 0,
                    input_tokens: 0,
                    output_tokens: 0,
                    first_seen_at: None,
                    last_seen_at: None,
                },
                Vec::new(),
            )
        })
        .collect();
    for run in runs {
        if since.is_some_and(|start| run.last_event_at < start) {
            continue;
        }
        let Some((row, durations)) = run
            .variant_id
            .as_deref()
            .and_then(|named| rows.iter_mut().find(|(row, _)| row.variant_id == named))
        else {
            continue;
        };
        if run.evaluation_id.is_some() {
            row.measured_runs += 1;
            continue;
        }
        row.runs += 1;
        match run.status {
            RunStatus::Succeeded => row.succeeded += 1,
            RunStatus::Failed => row.failed += 1,
            RunStatus::Running => row.running += 1,
        }
        if let Some(duration) = run.duration_ms {
            durations.push(duration);
        }
        row.llm_calls += run.llm_calls;
        row.input_tokens += run.input_tokens;
        row.output_tokens += run.output_tokens;
        row.first_seen_at = Some(
            row.first_seen_at
                .map_or(run.started_at, |seen| seen.min(run.started_at)),
        );
        row.last_seen_at = Some(
            row.last_seen_at
                .map_or(run.last_event_at, |seen| seen.max(run.last_event_at)),
        );
    }
    rows.into_iter()
        .map(|(mut row, durations)| {
            row.duration_ms = DurationSummary::of(durations);
            row
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use aiwatcher_core::{Checkpoint, TraceId};

    use super::*;

    fn run(run_id: &str, variant: Option<&str>, status: RunStatus, took: i64) -> RunSummary {
        let started = datetime!(2026-09-13 10:00:00 UTC);
        let ended = started + time::Duration::milliseconds(took);
        RunSummary {
            run_id: run_id.to_owned(),
            conversation_id: None,
            trace_id: TraceId::derive(run_id),
            status,
            agents: Vec::new(),
            runtimes: Vec::new(),
            workflow: None,
            variant_id: variant.map(ToOwned::to_owned),
            evaluation_id: None,
            published_by: None,
            caller_run_id: None,
            workflow_topology: None,
            nodes_run: Vec::new(),
            started_at: started,
            last_event_at: ended,
            ended_at: (status != RunStatus::Running).then_some(ended),
            duration_ms: (status != RunStatus::Running).then_some(took),
            event_count: 4,
            llm_calls: 1,
            tool_calls: 0,
            input_tokens: 10,
            output_tokens: 3,
            cached_tokens: 0,
            error: None,
            last_checkpoint: Checkpoint::beginning(),
        }
    }

    #[test]
    fn a_variant_s_runs_are_counted_and_its_finished_runs_ranked() {
        let runs: Vec<RunSummary> = (1..=10)
            .map(|at| {
                run(
                    &format!("r{at}"),
                    Some("v1"),
                    RunStatus::Succeeded,
                    at * 100,
                )
            })
            .chain([
                run("failed", Some("v1"), RunStatus::Failed, 50),
                run("going", Some("v1"), RunStatus::Running, 0),
                run("other", Some("v2"), RunStatus::Succeeded, 9_000),
                run("nobody", None, RunStatus::Succeeded, 9_000),
            ])
            .collect();

        let [row] = compute(&runs, &["v1"], None, datetime!(2026-09-13 11:00:00 UTC))
            .try_into()
            .expect("one row per variant asked about");

        assert_eq!(
            (row.runs, row.succeeded, row.failed, row.running),
            (12, 10, 1, 1)
        );
        assert_eq!(
            row.duration_ms,
            Some(DurationSummary {
                runs: 11,
                p50: 500,
                p90: 900,
                p99: 1_000,
                max: 1_000,
            })
        );
        assert_eq!(
            (row.llm_calls, row.input_tokens, row.output_tokens),
            (12, 120, 36)
        );
    }

    #[test]
    fn a_run_made_for_a_measurement_is_counted_apart_and_in_no_figure() {
        let mut measured = run("benchmark", Some("v1"), RunStatus::Succeeded, 5_000);
        measured.evaluation_id = Some("answers-candidate".to_owned());
        let runs = [
            measured,
            run("served", Some("v1"), RunStatus::Succeeded, 200),
        ];

        let row = &compute(&runs, &["v1"], None, datetime!(2026-09-13 11:00:00 UTC))[0];

        assert_eq!((row.runs, row.measured_runs), (1, 1));
        assert_eq!(
            row.duration_ms.as_ref().map(|summary| summary.max),
            Some(200)
        );
        assert_eq!(row.input_tokens, 10);
    }

    #[test]
    fn a_variant_nothing_named_is_a_row_of_nothing_rather_than_no_row() {
        let rows = compute(
            &[run("other", Some("v2"), RunStatus::Succeeded, 10)],
            &["v1", "v2"],
            None,
            datetime!(2026-09-13 11:00:00 UTC),
        );

        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].runs, rows[0].duration_ms.is_none()), (0, true));
        assert!(rows[0].last_seen_at.is_none());
        assert_eq!(rows[1].runs, 1);
    }

    #[test]
    fn a_run_outside_the_window_is_not_observed() {
        let rows = compute(
            &[run("old", Some("v1"), RunStatus::Succeeded, 10)],
            &["v1"],
            Some(60),
            datetime!(2026-09-13 11:00:00 UTC),
        );

        assert_eq!(rows[0].runs, 0);
    }
}
