//! Evaluation reports: what a suite scored, and against what.
//!
//! An evaluation is the other half of the loop the traces come from. A trace
//! answers "what did this run do"; an evaluation answers "is the thing that
//! produces those runs getting better or worse". They arrive on the same log
//! and they are folded apart:
//!
//! ```text
//! run.* / agent.* / llm.* / tool.* / step.*  → spans, metrics, the runs list
//! eval.*                                     → this module, and nothing else
//! ```
//!
//! **A report is not a trace.** It has a start, an end and a duration, and it
//! still has no business in a trace store — see `EventType::forms_span`. What
//! it has instead is parameters, metrics and a document, which is the shape
//! MLflow's `start_run` / `log_params` / `log_metrics` / `log_dict` produces
//! and the shape a producer switching off MLflow needs somewhere to put.
//!
//! Two rules carry most of the meaning here:
//!
//! * **A comparison is pinned to a dataset.** The baseline for an evaluation
//!   is the previous one of the *same suite on the same dataset*. Two numbers
//!   measured on different cases are not a comparison, and showing them beside
//!   each other says they are.
//! * **The per-case list is capped; the counters are not.** A suite with more
//!   cases than [`EvaluationConfig::max_cases_per_evaluation`] keeps the first
//!   N for the case view and still counts every one of them, so the pass rate
//!   stays true even where the detail is partial. The same trade under memory
//!   pressure: the oldest finished evaluations give up their cases and their
//!   documents, and keep their metrics — see [`EvaluationConfig`].

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use aiwatcher_core::{Checkpoint, EventType, Phase, RecordedEvent};

/// Where an evaluation got to.
///
/// The same three states a run has, for the same reason: a report that never
/// arrived and a report that failed are different facts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationStatus {
    #[default]
    Running,
    Succeeded,
    Failed,
}

/// One scored case.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EvaluationCase {
    pub case_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    /// Why the scorer said what it said. A score without its rationale is not
    /// reviewable, which is the whole complaint about opaque eval numbers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One row in the evaluations list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EvaluationSummary {
    /// The producer's `run_id`. An evaluation is an execution too, so it gets
    /// its own stream in the log and its own partition — which is what keeps a
    /// report's events in order without serialising it behind agent traffic.
    pub evaluation_id: String,
    /// What was measured. Falls back through `suite`, `suite_name`, `run_name`
    /// — MLflow's word — and finally the evaluation id, so a producer porting
    /// a `start_run(run_name=…)` call lands somewhere sensible without
    /// renaming anything.
    pub suite: String,
    /// The cases it was measured on, ideally versioned. Without this a
    /// comparison is not one; see [`EvaluationDetail::comparison`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dataset: Option<String>,
    /// What was under test: a prompt version, a model, a checkout. The join to
    /// the experiments side.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    pub status: EvaluationStatus,
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub ended_at: Option<OffsetDateTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    /// Everything that was held fixed, stringified. MLflow's `log_params`.
    pub params: BTreeMap<String, String>,
    /// Everything that was measured. MLflow's `log_metrics`.
    pub metrics: BTreeMap<String, f64>,
    pub cases_total: u64,
    pub cases_passed: u64,
    pub cases_failed: u64,
    /// From the cases where there are any, from `metrics.pass_rate` where the
    /// producer only reported the aggregate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pass_rate: Option<f64>,
    /// The service that produced it.
    pub runtime: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Size of the report document as it arrived, whether or not it was kept.
    pub report_bytes: usize,
    /// The document is not held. Either it was over
    /// [`EvaluationConfig::max_report_bytes`] — half a JSON document is not a
    /// JSON document, so it is dropped rather than cut — or it was shed to
    /// keep the projection inside its budget. `report_bytes` still says how
    /// big it was.
    pub report_dropped: bool,
    pub last_checkpoint: Checkpoint,
}

impl EvaluationSummary {
    fn new(event: &RecordedEvent) -> Self {
        Self {
            evaluation_id: event.metadata.run_id.clone(),
            suite: event.metadata.run_id.clone(),
            dataset: None,
            variant: None,
            status: EvaluationStatus::Running,
            started_at: event.metadata.occurred_at,
            ended_at: None,
            duration_ms: None,
            params: BTreeMap::new(),
            metrics: BTreeMap::new(),
            cases_total: 0,
            cases_passed: 0,
            cases_failed: 0,
            pass_rate: None,
            runtime: event.metadata.source.service.clone(),
            error: None,
            report_bytes: 0,
            report_dropped: false,
            last_checkpoint: Checkpoint::beginning(),
        }
    }
}

/// One evaluation, with everything retained for it.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct EvaluationDetail {
    pub summary: EvaluationSummary,
    pub cases: Vec<EvaluationCase>,
    /// More cases were scored than are listed. `summary.cases_total` is still
    /// the true count.
    pub cases_truncated: bool,
    /// Whatever the producer put in `data.report` — the free-form half, and
    /// the reason `log_dict` had nowhere to go before this existed.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Object)]
    pub report: Option<serde_json::Value>,
    /// Against the previous evaluation of the same suite on the same dataset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comparison: Option<EvaluationComparison>,
}

/// This evaluation against the one before it.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct EvaluationComparison {
    pub baseline_id: String,
    #[serde(with = "time::serde::rfc3339")]
    pub baseline_started_at: OffsetDateTime,
    /// Every metric either side reported, so one that appeared or disappeared
    /// is visible rather than silently absent.
    pub metrics: Vec<MetricDelta>,
    /// Passed on the baseline, fails now. The view that has to be read before
    /// a release.
    pub regressed: Vec<CaseDelta>,
    /// Failed on the baseline, passes now.
    pub fixed: Vec<CaseDelta>,
}

#[derive(Clone, Debug, PartialEq, Serialize, utoipa::ToSchema)]
pub struct MetricDelta {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<f64>,
    /// `current - baseline`, and `None` when either side is missing — a
    /// missing metric is not a delta of zero.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, utoipa::ToSchema)]
pub struct CaseDelta {
    pub case_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline_score: Option<f64>,
}

/// Filters for the evaluations list.
#[derive(Clone, Debug, Default, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct EvaluationFilter {
    /// Only reports that finished — or, while they are still running, started
    /// — in the last this-many seconds. See [`crate::window`].
    pub window_seconds: Option<i64>,
    pub suite: Option<String>,
    pub dataset: Option<String>,
    pub variant: Option<String>,
    pub status: Option<EvaluationStatus>,
    /// Substring over the id, the suite, the dataset, the variant and the
    /// parameter values.
    pub search: Option<String>,
    /// Cursor: the last evaluation id on the previous page. Exclusive.
    pub after: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct EvaluationPage {
    pub evaluations: Vec<EvaluationSummary>,
    /// Pass as `after` to fetch the next page. Absent on the last page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub total_known: usize,
}

/// A suite on a dataset, across every evaluation of it that is retained.
///
/// The level above a report, and the one MLflow calls an experiment.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct SuiteSummary {
    pub suite: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dataset: Option<String>,
    pub evaluations: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub running: u64,
    pub last_evaluation_id: String,
    pub last_status: EvaluationStatus,
    #[serde(with = "time::serde::rfc3339")]
    pub last_started_at: OffsetDateTime,
    /// The newest finished evaluation's metrics, and the change from the one
    /// before it. A number with no direction is the thing an evaluation page
    /// is usually accused of being.
    pub latest_metrics: BTreeMap<String, f64>,
    pub metric_deltas: BTreeMap<String, f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pass_rate: Option<f64>,
}

#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct SuitePage {
    pub suites: Vec<SuiteSummary>,
    pub total: usize,
}

/// What the evaluation projection is allowed to hold.
///
/// The same kind of memory contract as `ReadModelConfig`, and the same trap in
/// it: `max_evaluations × max_cases_per_evaluation` is not a bound, it is an
/// exposure. A producer-supplied document has no size this process gets to
/// assume, so the totals — [`Self::max_cases_total`] and [`Self::max_reports`]
/// — are what actually decide the footprint. At the defaults the projection
/// holds roughly 25 MB at saturation.
#[derive(Clone, Copy, Debug)]
pub struct EvaluationConfig {
    /// Evaluations retained. Past it, the oldest *finished* ones go first — a
    /// running evaluation is never dropped out from under a live viewer.
    pub max_evaluations: usize,
    /// Cases retained per evaluation for the case view. The counters ignore
    /// this cap, so a truncated case list still reports a true pass rate.
    pub max_cases_per_evaluation: usize,
    /// Cases retained across **all** evaluations. The cap that makes the
    /// footprint predictable, by shedding the oldest finished evaluations'
    /// cases when the total is exceeded.
    pub max_cases_total: usize,
    /// Report documents larger than this are dropped rather than cut.
    pub max_report_bytes: usize,
    /// Documents retained across all evaluations, oldest finished shed first.
    pub max_reports: usize,
}

impl Default for EvaluationConfig {
    fn default() -> Self {
        Self {
            max_evaluations: 500,
            max_cases_per_evaluation: 2_000,
            max_cases_total: 20_000,
            max_report_bytes: 64 * 1024,
            max_reports: 200,
        }
    }
}

/// One evaluation as it is held in memory.
#[derive(Clone, Debug, Default)]
struct Held {
    summary: Option<EvaluationSummary>,
    cases: Vec<EvaluationCase>,
    cases_truncated: bool,
    report: Option<serde_json::Value>,
}

/// The projection. Folded from the log like everything else here, and rebuilt
/// by a replay on restart.
#[derive(Debug, Default)]
pub struct EvaluationState {
    held: HashMap<String, Held>,
    /// Evaluation ids in first-seen order; the eviction candidate list.
    order: Vec<String>,
    /// Running totals, so the global caps are checked without walking every
    /// evaluation on every write.
    case_count: usize,
    report_count: usize,
}

impl EvaluationState {
    /// Fold one `eval.*` event in.
    pub fn apply(&mut self, event: &RecordedEvent, config: &EvaluationConfig) {
        let id = event.metadata.run_id.clone();
        if !self.held.contains_key(&id) {
            self.order.push(id.clone());
            self.held.insert(id.clone(), Held::default());
        }
        // The entry borrow and the running totals cannot be held at once, so
        // the deltas are computed first and applied after — the same shape as
        // `ReadModel::record_spans`.
        let mut cases_added = 0usize;
        let mut report_change = 0isize;
        {
            let Some(entry) = self.held.get_mut(&id) else {
                return;
            };
            let summary = entry
                .summary
                .get_or_insert_with(|| EvaluationSummary::new(event));

            summary.last_checkpoint = event.metadata.checkpoint.clone();
            if event.metadata.occurred_at < summary.started_at {
                summary.started_at = event.metadata.occurred_at;
            }
            if !event.metadata.source.service.is_empty() {
                summary.runtime = event.metadata.source.service.clone();
            }

            if let Some(suite) = identity(event, &["suite", "suite_name", "run_name"])
                .or_else(|| event.metadata.workflow_id.clone())
            {
                summary.suite = suite;
            }
            if let Some(dataset) = identity(event, &["dataset", "dataset_id", "dataset_version"]) {
                summary.dataset = Some(dataset);
            }
            if let Some(variant) = identity(event, &["variant", "variant_id"]) {
                summary.variant = Some(variant);
            }
            summary.params.extend(string_map(event.data.get("params")));
            summary
                .metrics
                .extend(number_map(event.data.get("metrics")));

            match event.event_type {
                EventType::EvalCase => {
                    let case = case_from(event);
                    summary.cases_total += 1;
                    match case.passed {
                        Some(true) => summary.cases_passed += 1,
                        Some(false) => summary.cases_failed += 1,
                        None => {}
                    }
                    if entry.cases.len() < config.max_cases_per_evaluation {
                        entry.cases.push(case);
                        cases_added += 1;
                    } else {
                        entry.cases_truncated = true;
                    }
                }
                _ => {
                    // A producer that scores in a batch reports the totals on the
                    // end event instead of sending a case each. Both shapes have
                    // to work: `record_evaluation` is one call, not a loop.
                    if let Some(total) = event
                        .data_i64("cases_total")
                        .or_else(|| event.data.get("cases").and_then(serde_json::Value::as_i64))
                    {
                        summary.cases_total = summary.cases_total.max(total.max(0) as u64);
                    }
                    if let Some(passed) = event.data_i64("cases_passed") {
                        summary.cases_passed = summary.cases_passed.max(passed.max(0) as u64);
                    }
                    if let Some(failed) = event.data_i64("cases_failed") {
                        summary.cases_failed = summary.cases_failed.max(failed.max(0) as u64);
                    }
                }
            }

            if let Some(report) = event.data.get("report") {
                let bytes = serde_json::to_string(report).map_or(0, |text| text.len());
                let held_before = entry.report.is_some();
                summary.report_bytes = bytes;
                if bytes > config.max_report_bytes {
                    summary.report_dropped = true;
                    entry.report = None;
                } else {
                    summary.report_dropped = false;
                    entry.report = Some(report.clone());
                }
                match (held_before, entry.report.is_some()) {
                    (false, true) => report_change += 1,
                    (true, false) => report_change -= 1,
                    _ => {}
                }
            }

            if let Some(Phase::End { ok }) = event.event_type.phase() {
                summary.status = if ok {
                    EvaluationStatus::Succeeded
                } else {
                    EvaluationStatus::Failed
                };
                if !ok {
                    summary.error = event
                        .data_str("error")
                        .or_else(|| event.data_str("message"))
                        .map(ToOwned::to_owned);
                }
                summary.ended_at = Some(event.metadata.occurred_at);
                summary.duration_ms = Some(
                    ((event.metadata.occurred_at - summary.started_at).whole_milliseconds()).max(0)
                        as i64,
                );
            }

            summary.pass_rate = if summary.cases_total > 0 {
                Some(summary.cases_passed as f64 / summary.cases_total as f64)
            } else {
                summary.metrics.get("pass_rate").copied()
            };
        }

        self.case_count += cases_added;
        self.report_count = self.report_count.saturating_add_signed(report_change);
        self.evict(config.max_evaluations);
        self.shed_detail(config);
    }

    /// Newest first, filtered, one page.
    #[must_use]
    pub fn page(&self, filter: &EvaluationFilter, now: OffsetDateTime) -> EvaluationPage {
        let limit = filter.limit.unwrap_or(50).clamp(1, 500);
        let needle = filter.search.as_ref().map(|text| text.to_lowercase());
        let since = crate::window::cutoff(filter.window_seconds, now);

        let mut matching: Vec<&EvaluationSummary> = self
            .order
            .iter()
            .rev()
            .filter_map(|id| self.held.get(id))
            .filter_map(|held| held.summary.as_ref())
            .filter(|row| since.is_none_or(|start| row.ended_at.unwrap_or(row.started_at) >= start))
            .filter(|row| filter.suite.as_ref().is_none_or(|want| &row.suite == want))
            .filter(|row| {
                filter
                    .dataset
                    .as_ref()
                    .is_none_or(|want| row.dataset.as_ref() == Some(want))
            })
            .filter(|row| {
                filter
                    .variant
                    .as_ref()
                    .is_none_or(|want| row.variant.as_ref() == Some(want))
            })
            .filter(|row| filter.status.is_none_or(|want| row.status == want))
            .filter(|row| needle.as_ref().is_none_or(|needle| matches(row, needle)))
            .collect();

        if let Some(cursor) = &filter.after
            && let Some(index) = matching.iter().position(|row| &row.evaluation_id == cursor)
        {
            matching.drain(0..=index);
        }

        let total_known = matching.len();
        let evaluations: Vec<EvaluationSummary> =
            matching.into_iter().take(limit).cloned().collect();
        let next_cursor = (total_known > evaluations.len())
            .then(|| evaluations.last().map(|row| row.evaluation_id.clone()))
            .flatten();

        EvaluationPage {
            evaluations,
            next_cursor,
            total_known,
        }
    }

    /// One evaluation, with its cases, its report, and its baseline.
    #[must_use]
    pub fn detail(&self, evaluation_id: &str) -> Option<EvaluationDetail> {
        let held = self.held.get(evaluation_id)?;
        let summary = held.summary.as_ref()?.clone();
        let comparison = self
            .baseline_for(&summary)
            .map(|baseline| self.compare(&summary, held, baseline));

        Some(EvaluationDetail {
            summary,
            cases: held.cases.clone(),
            cases_truncated: held.cases_truncated,
            report: held.report.clone(),
            comparison,
        })
    }

    /// Suites, newest activity first.
    #[must_use]
    pub fn suites(&self) -> SuitePage {
        let mut grouped: HashMap<(String, Option<String>), Vec<&EvaluationSummary>> =
            HashMap::new();
        for summary in self
            .order
            .iter()
            .filter_map(|id| self.held.get(id))
            .filter_map(|held| held.summary.as_ref())
        {
            grouped
                .entry((summary.suite.clone(), summary.dataset.clone()))
                .or_default()
                .push(summary);
        }

        let mut suites: Vec<SuiteSummary> = grouped
            .into_iter()
            .map(|((suite, dataset), mut rows)| {
                rows.sort_by_key(|row| row.started_at);
                let last = rows.last().copied().unwrap_or(rows[0]);
                let finished: Vec<&&EvaluationSummary> = rows
                    .iter()
                    .filter(|row| row.status != EvaluationStatus::Running)
                    .collect();
                let latest = finished.last().copied();
                let previous = finished.len().checked_sub(2).and_then(|i| finished.get(i));
                let latest_metrics = latest.map(|row| row.metrics.clone()).unwrap_or_default();
                let metric_deltas = previous.map_or_else(BTreeMap::new, |before| {
                    latest_metrics
                        .iter()
                        .filter_map(|(name, value)| {
                            before
                                .metrics
                                .get(name)
                                .map(|old| (name.clone(), value - old))
                        })
                        .collect()
                });

                SuiteSummary {
                    suite,
                    dataset,
                    evaluations: rows.len() as u64,
                    succeeded: count(&rows, EvaluationStatus::Succeeded),
                    failed: count(&rows, EvaluationStatus::Failed),
                    running: count(&rows, EvaluationStatus::Running),
                    last_evaluation_id: last.evaluation_id.clone(),
                    last_status: last.status,
                    last_started_at: last.started_at,
                    latest_metrics,
                    metric_deltas,
                    pass_rate: latest.and_then(|row| row.pass_rate),
                }
            })
            .collect();

        suites.sort_by(|a, b| {
            b.last_started_at
                .cmp(&a.last_started_at)
                .then_with(|| a.suite.cmp(&b.suite))
        });
        let total = suites.len();
        SuitePage { suites, total }
    }

    /// How many evaluations are currently held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.held.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    /// The previous finished evaluation of the same suite **on the same
    /// dataset**.
    ///
    /// The dataset is not optional pedantry: a score measured on one set of
    /// cases and a score measured on another are two facts, and putting a
    /// delta between them claims they are one. Where the producer names no
    /// dataset, `None == None` still matches — an unversioned suite compares
    /// against itself, which is the honest reading of "we did not say".
    fn baseline_for(&self, current: &EvaluationSummary) -> Option<&EvaluationSummary> {
        self.order
            .iter()
            .filter_map(|id| self.held.get(id))
            .filter_map(|held| held.summary.as_ref())
            .filter(|row| row.evaluation_id != current.evaluation_id)
            .filter(|row| row.suite == current.suite && row.dataset == current.dataset)
            .filter(|row| row.status != EvaluationStatus::Running)
            .filter(|row| row.started_at < current.started_at)
            .max_by_key(|row| row.started_at)
    }

    fn compare(
        &self,
        current: &EvaluationSummary,
        held: &Held,
        baseline: &EvaluationSummary,
    ) -> EvaluationComparison {
        let mut names: Vec<&String> = current.metrics.keys().collect();
        names.extend(baseline.metrics.keys());
        names.sort_unstable();
        names.dedup();

        let metrics = names
            .into_iter()
            .map(|name| {
                let now = current.metrics.get(name).copied();
                let then = baseline.metrics.get(name).copied();
                MetricDelta {
                    name: name.clone(),
                    current: now,
                    baseline: then,
                    delta: now.zip(then).map(|(now, then)| now - then),
                }
            })
            .collect();

        let baseline_cases: HashMap<&str, &EvaluationCase> = self
            .held
            .get(&baseline.evaluation_id)
            .map(|held| {
                held.cases
                    .iter()
                    .map(|case| (case.case_id.as_str(), case))
                    .collect()
            })
            .unwrap_or_default();

        let mut regressed = Vec::new();
        let mut fixed = Vec::new();
        for case in &held.cases {
            let Some(before) = baseline_cases.get(case.case_id.as_str()) else {
                continue;
            };
            let delta = CaseDelta {
                case_id: case.case_id.clone(),
                current_score: case.score,
                baseline_score: before.score,
            };
            match (before.passed, case.passed) {
                (Some(true), Some(false)) => regressed.push(delta),
                (Some(false), Some(true)) => fixed.push(delta),
                _ => {}
            }
        }

        EvaluationComparison {
            baseline_id: baseline.evaluation_id.clone(),
            baseline_started_at: baseline.started_at,
            metrics,
            regressed,
            fixed,
        }
    }

    /// Give up detail before summaries.
    ///
    /// An evaluation stripped of its cases and its document still shows its
    /// parameters, its metrics, its pass rate and its place in a suite's
    /// history — everything the list, the suite view and the metric comparison
    /// read. What is given up is the per-case view of an old evaluation, which
    /// is the same trade `ReadModel::shed_spans` makes for an old run's
    /// waterfall. A running evaluation is skipped: it is the one someone is
    /// most likely to be watching.
    fn shed_detail(&mut self, config: &EvaluationConfig) {
        let within_budget = |cases: usize, reports: usize| {
            cases <= config.max_cases_total && reports <= config.max_reports
        };
        if within_budget(self.case_count, self.report_count) {
            return;
        }
        for id in self.order.clone() {
            if within_budget(self.case_count, self.report_count) {
                break;
            }
            let Some(held) = self.held.get_mut(&id) else {
                continue;
            };
            let running = held
                .summary
                .as_ref()
                .is_some_and(|summary| summary.status == EvaluationStatus::Running);
            if running {
                continue;
            }
            if self.case_count > config.max_cases_total && !held.cases.is_empty() {
                self.case_count = self.case_count.saturating_sub(held.cases.len());
                held.cases.clear();
                held.cases_truncated = true;
            }
            if self.report_count > config.max_reports && held.report.take().is_some() {
                self.report_count = self.report_count.saturating_sub(1);
                if let Some(summary) = held.summary.as_mut() {
                    summary.report_dropped = true;
                }
            }
        }
    }

    /// Drop the oldest finished evaluations once over the cap.
    fn evict(&mut self, max_evaluations: usize) {
        if self.held.len() <= max_evaluations {
            return;
        }
        let mut excess = self.held.len() - max_evaluations;
        let mut keep = Vec::with_capacity(self.order.len());
        for id in std::mem::take(&mut self.order) {
            let finished = self.held.get(&id).is_some_and(|held| {
                held.summary
                    .as_ref()
                    .is_some_and(|summary| summary.status != EvaluationStatus::Running)
            });
            if excess > 0 && finished {
                if let Some(dropped) = self.held.remove(&id) {
                    self.case_count = self.case_count.saturating_sub(dropped.cases.len());
                    if dropped.report.is_some() {
                        self.report_count = self.report_count.saturating_sub(1);
                    }
                }
                excess -= 1;
            } else {
                keep.push(id);
            }
        }
        self.order = keep;
    }
}

fn count(rows: &[&EvaluationSummary], status: EvaluationStatus) -> u64 {
    rows.iter().filter(|row| row.status == status).count() as u64
}

fn matches(row: &EvaluationSummary, needle: &str) -> bool {
    [
        Some(row.evaluation_id.as_str()),
        Some(row.suite.as_str()),
        row.dataset.as_deref(),
        row.variant.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|field| field.to_lowercase().contains(needle))
        || row.params.iter().any(|(key, value)| {
            key.to_lowercase().contains(needle) || value.to_lowercase().contains(needle)
        })
}

/// The first of several payload spellings that is a non-empty string.
fn identity(event: &RecordedEvent, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| event.data_str(key))
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

/// A JSON object read as `String -> String`.
///
/// Non-string values are stringified rather than dropped: a parameter is a
/// label, and a producer that logs `temperature: 0.2` means the same thing as
/// one that logs `"0.2"`.
fn string_map(value: Option<&serde_json::Value>) -> BTreeMap<String, String> {
    value
        .and_then(serde_json::Value::as_object)
        .map(|object| {
            object
                .iter()
                .map(|(key, value)| {
                    let text = match value {
                        serde_json::Value::String(text) => text.clone(),
                        other => other.to_string(),
                    };
                    (key.clone(), text)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A JSON object read as `String -> f64`. Anything that is not a number is
/// skipped: a metric that is not a number is not a metric.
fn number_map(value: Option<&serde_json::Value>) -> BTreeMap<String, f64> {
    value
        .and_then(serde_json::Value::as_object)
        .map(|object| {
            object
                .iter()
                .filter_map(|(key, value)| {
                    value
                        .as_f64()
                        .or_else(|| value.as_bool().map(|flag| f64::from(u8::from(flag))))
                        .map(|number| (key.clone(), number))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn case_from(event: &RecordedEvent) -> EvaluationCase {
    EvaluationCase {
        case_id: identity(event, &["case_id", "case", "id"])
            .unwrap_or_else(|| event.metadata.message_id.to_string()),
        score: event.data_f64("score"),
        passed: event
            .data
            .get("passed")
            .or_else(|| event.data.get("success"))
            .and_then(serde_json::Value::as_bool),
        duration_ms: event.data_i64("duration_ms"),
        reason: event
            .data_str("reason")
            .or_else(|| event.data_str("rationale"))
            .map(|text| truncated(text, 500)),
        error: event.data_str("error").map(|text| truncated(text, 500)),
    }
}

/// Free text from a producer, bounded. A rationale is worth keeping; an
/// unbounded one is a memory leak with a good excuse.
fn truncated(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use time::macros::datetime;

    use aiwatcher_core::{EventEnvelope, Sdk, Source};

    use super::*;

    fn now() -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }

    fn event(
        id: &str,
        event_type: EventType,
        at: OffsetDateTime,
        data: serde_json::Value,
    ) -> RecordedEvent {
        EventEnvelope::new(
            event_type,
            id,
            at,
            Source::new("evaluation-service", Sdk::Python),
        )
        .with_data(data)
        .record(1, 1, at, None)
    }

    fn fold(events: &[RecordedEvent]) -> EvaluationState {
        let mut state = EvaluationState::default();
        let config = EvaluationConfig::default();
        for event in events {
            state.apply(event, &config);
        }
        state
    }

    /// The shape `record_evaluation` produces: one start, one end, everything
    /// in the payload. This is the MLflow call it replaces —
    /// `start_run` / `log_params` / `log_metrics` / `log_dict` — with no loop
    /// over cases anywhere in it.
    #[test]
    fn a_batch_report_becomes_one_row_carrying_its_params_metrics_and_document() {
        let state = fold(&[
            event(
                "eval-1",
                EventType::EvalStarted,
                datetime!(2026-08-28 09:00:00 UTC),
                json!({
                    "suite": "catalog-floor-plan",
                    "dataset": "house-catalog@3",
                    "params": { "model": "gpt-5-mini", "threshold": 0.9 },
                }),
            ),
            event(
                "eval-1",
                EventType::EvalCompleted,
                datetime!(2026-08-28 09:04:00 UTC),
                json!({
                    "metrics": { "mean_score": 0.91, "pass_rate": 0.75 },
                    "cases_total": 4,
                    "cases_passed": 3,
                    "report": { "cases": [{ "id": "K-1", "score": 0.94 }] },
                }),
            ),
        ]);

        let detail = state.detail("eval-1").expect("the evaluation");
        let summary = &detail.summary;
        assert_eq!(summary.suite, "catalog-floor-plan");
        assert_eq!(summary.dataset.as_deref(), Some("house-catalog@3"));
        assert_eq!(summary.status, EvaluationStatus::Succeeded);
        assert_eq!(summary.duration_ms, Some(240_000));
        assert_eq!(summary.params["model"], "gpt-5-mini");
        assert_eq!(
            summary.params["threshold"], "0.9",
            "a numeric parameter is a label, not a metric"
        );
        assert_eq!(summary.metrics["mean_score"], 0.91);
        assert_eq!(summary.cases_total, 4);
        assert_eq!(summary.pass_rate, Some(0.75));
        assert!(detail.report.is_some(), "log_dict has somewhere to go");
        assert!(!summary.report_dropped);
    }

    /// MLflow's word for the thing being measured is `run_name`, and a port
    /// that renames nothing should still land under a readable suite.
    #[test]
    fn an_mlflow_shaped_call_is_named_by_its_run_name() {
        let state = fold(&[event(
            "eval-7",
            EventType::EvalCompleted,
            datetime!(2026-08-28 09:00:00 UTC),
            json!({ "run_name": "floor-plan-nightly", "metrics": { "score": 1.0 } }),
        )]);

        let detail = state.detail("eval-7").expect("the evaluation");
        assert_eq!(detail.summary.suite, "floor-plan-nightly");
        assert_eq!(
            detail.summary.status,
            EvaluationStatus::Succeeded,
            "an end event with no start is still a finished evaluation"
        );
    }

    #[test]
    fn an_evaluation_that_failed_keeps_the_reason() {
        let state = fold(&[
            event(
                "eval-2",
                EventType::EvalStarted,
                datetime!(2026-08-28 09:00:00 UTC),
                json!({ "suite": "s" }),
            ),
            event(
                "eval-2",
                EventType::EvalFailed,
                datetime!(2026-08-28 09:00:30 UTC),
                json!({ "error": "the judge model returned 429" }),
            ),
        ]);

        let summary = state.detail("eval-2").expect("the evaluation").summary;
        assert_eq!(summary.status, EvaluationStatus::Failed);
        assert_eq!(
            summary.error.as_deref(),
            Some("the judge model returned 429")
        );
    }

    fn suite_run(
        id: &str,
        dataset: &str,
        at: OffsetDateTime,
        cases: &[(&str, bool, f64)],
    ) -> Vec<RecordedEvent> {
        let mut events = vec![event(
            id,
            EventType::EvalStarted,
            at,
            json!({ "suite": "catalog", "dataset": dataset }),
        )];
        for (case_id, passed, score) in cases {
            events.push(event(
                id,
                EventType::EvalCase,
                at,
                json!({ "case_id": case_id, "passed": passed, "score": score }),
            ));
        }
        let mean = cases.iter().map(|(_, _, score)| score).sum::<f64>() / cases.len() as f64;
        events.push(event(
            id,
            EventType::EvalCompleted,
            at + time::Duration::seconds(60),
            json!({ "metrics": { "mean_score": mean } }),
        ));
        events
    }

    #[test]
    fn a_case_that_passed_on_the_baseline_and_fails_now_is_a_regression() {
        let mut events = suite_run(
            "eval-a",
            "cases@1",
            datetime!(2026-08-28 09:00:00 UTC),
            &[("K-1", true, 0.9), ("K-2", false, 0.4)],
        );
        events.extend(suite_run(
            "eval-b",
            "cases@1",
            datetime!(2026-08-28 10:00:00 UTC),
            &[("K-1", false, 0.3), ("K-2", true, 0.95)],
        ));
        let state = fold(&events);

        let comparison = state
            .detail("eval-b")
            .expect("the evaluation")
            .comparison
            .expect("a baseline on the same dataset");
        assert_eq!(comparison.baseline_id, "eval-a");
        assert_eq!(
            comparison
                .regressed
                .iter()
                .map(|c| c.case_id.as_str())
                .collect::<Vec<_>>(),
            vec!["K-1"]
        );
        assert_eq!(
            comparison
                .fixed
                .iter()
                .map(|c| c.case_id.as_str())
                .collect::<Vec<_>>(),
            vec!["K-2"]
        );
        let mean = comparison
            .metrics
            .iter()
            .find(|metric| metric.name == "mean_score")
            .expect("the metric");
        // 0.625 against 0.65: one case fixed, one regressed, and the mean
        // still fell. Which is exactly why the case list is shown next to the
        // metric rather than instead of it.
        assert!(mean.delta.is_some_and(|delta| (delta + 0.025).abs() < 1e-9));
    }

    /// Two scores measured on different cases are two facts. Putting a delta
    /// between them claims they are one.
    #[test]
    fn an_evaluation_on_a_different_dataset_is_not_a_baseline() {
        let mut events = suite_run(
            "eval-old",
            "cases@1",
            datetime!(2026-08-28 09:00:00 UTC),
            &[("K-1", true, 0.9)],
        );
        events.extend(suite_run(
            "eval-new",
            "cases@2",
            datetime!(2026-08-28 10:00:00 UTC),
            &[("K-1", true, 0.5)],
        ));
        let state = fold(&events);

        assert!(
            state
                .detail("eval-new")
                .expect("the evaluation")
                .comparison
                .is_none(),
            "the dataset changed, so there is nothing to compare against"
        );
    }

    #[test]
    fn a_metric_only_one_side_reported_has_no_delta() {
        let state = fold(&[
            event(
                "eval-1",
                EventType::EvalCompleted,
                datetime!(2026-08-28 09:00:00 UTC),
                json!({ "suite": "s", "metrics": { "mean_score": 0.5 } }),
            ),
            event(
                "eval-2",
                EventType::EvalCompleted,
                datetime!(2026-08-28 10:00:00 UTC),
                json!({ "suite": "s", "metrics": { "mean_score": 0.6, "cost_usd": 1.2 } }),
            ),
        ]);

        let comparison = state
            .detail("eval-2")
            .expect("the evaluation")
            .comparison
            .expect("a baseline");
        let cost = comparison
            .metrics
            .iter()
            .find(|metric| metric.name == "cost_usd")
            .expect("the new metric is listed rather than hidden");
        assert_eq!(cost.current, Some(1.2));
        assert_eq!(cost.baseline, None);
        assert_eq!(cost.delta, None, "a missing metric is not a delta of zero");
    }

    /// Half a JSON document is not a JSON document, so an oversized report is
    /// dropped and said to be dropped.
    #[test]
    fn a_report_over_the_cap_is_dropped_rather_than_cut() {
        let mut state = EvaluationState::default();
        let config = EvaluationConfig {
            max_report_bytes: 64,
            ..EvaluationConfig::default()
        };
        state.apply(
            &event(
                "eval-1",
                EventType::EvalCompleted,
                datetime!(2026-08-28 09:00:00 UTC),
                json!({ "suite": "s", "report": { "text": "x".repeat(500) } }),
            ),
            &config,
        );

        let detail = state.detail("eval-1").expect("the evaluation");
        assert!(detail.report.is_none());
        assert!(detail.summary.report_dropped);
        assert!(
            detail.summary.report_bytes > 64,
            "the size it arrived at is still reported"
        );
    }

    #[test]
    fn the_case_list_is_capped_and_the_counters_are_not() {
        let mut state = EvaluationState::default();
        let config = EvaluationConfig {
            max_cases_per_evaluation: 3,
            ..EvaluationConfig::default()
        };
        for index in 0..10 {
            state.apply(
                &event(
                    "eval-1",
                    EventType::EvalCase,
                    datetime!(2026-08-28 09:00:00 UTC),
                    json!({ "case_id": format!("c{index}"), "passed": index % 2 == 0 }),
                ),
                &config,
            );
        }

        let detail = state.detail("eval-1").expect("the evaluation");
        assert_eq!(detail.cases.len(), 3);
        assert!(detail.cases_truncated);
        assert_eq!(detail.summary.cases_total, 10);
        assert_eq!(detail.summary.cases_passed, 5);
        assert_eq!(
            detail.summary.pass_rate,
            Some(0.5),
            "a truncated case list still reports a true pass rate"
        );
    }

    /// `max_evaluations × max_cases_per_evaluation` is an exposure, not a
    /// bound. This is the cap that makes the footprint predictable, and the
    /// trade it makes is the same one the spans budget makes: lose the deepest
    /// view of the oldest thing, keep every summary.
    #[test]
    fn cases_are_shed_from_the_oldest_evaluations_once_the_global_budget_is_exceeded() {
        let mut state = EvaluationState::default();
        let config = EvaluationConfig {
            max_cases_total: 4,
            ..EvaluationConfig::default()
        };
        for evaluation in 0..3 {
            let id = format!("eval-{evaluation}");
            let at = datetime!(2026-08-28 09:00:00 UTC) + time::Duration::minutes(evaluation);
            for case in 0..3 {
                state.apply(
                    &event(
                        &id,
                        EventType::EvalCase,
                        at,
                        json!({ "case_id": format!("c{case}"), "passed": true }),
                    ),
                    &config,
                );
            }
            state.apply(
                &event(&id, EventType::EvalCompleted, at, json!({ "suite": "s" })),
                &config,
            );
        }

        assert!(state.case_count <= 4, "held {} cases", state.case_count);
        let oldest = state.detail("eval-0").expect("the evaluation");
        assert!(
            oldest.cases.is_empty(),
            "the oldest gives up its cases first"
        );
        assert!(oldest.cases_truncated, "and says the list is incomplete");
        assert_eq!(
            oldest.summary.cases_total, 3,
            "while the counters, the pass rate and the metrics survive"
        );
        assert_eq!(oldest.summary.pass_rate, Some(1.0));
        assert!(
            !state
                .detail("eval-2")
                .expect("the evaluation")
                .cases
                .is_empty(),
            "the newest keeps its cases"
        );
    }

    #[test]
    fn suites_group_by_suite_and_dataset_and_carry_the_change() {
        let mut events = suite_run(
            "eval-a",
            "cases@1",
            datetime!(2026-08-28 09:00:00 UTC),
            &[("K-1", true, 0.8)],
        );
        events.extend(suite_run(
            "eval-b",
            "cases@1",
            datetime!(2026-08-28 10:00:00 UTC),
            &[("K-1", true, 0.9)],
        ));
        events.extend(suite_run(
            "eval-c",
            "cases@2",
            datetime!(2026-08-28 11:00:00 UTC),
            &[("K-1", true, 0.5)],
        ));
        let page = fold(&events).suites();

        assert_eq!(page.total, 2, "a dataset version is its own suite row");
        let newest = &page.suites[0];
        assert_eq!(newest.dataset.as_deref(), Some("cases@2"));
        let versioned = page
            .suites
            .iter()
            .find(|row| row.dataset.as_deref() == Some("cases@1"))
            .expect("the first dataset");
        assert_eq!(versioned.evaluations, 2);
        assert_eq!(versioned.last_evaluation_id, "eval-b");
        assert!(
            (versioned.metric_deltas["mean_score"] - 0.1).abs() < 1e-9,
            "a number with no direction is what an evaluation page is usually accused of being"
        );
    }

    #[test]
    fn an_unfinished_evaluation_is_not_evicted_out_from_under_a_viewer() {
        let mut state = EvaluationState::default();
        let config = EvaluationConfig {
            max_evaluations: 2,
            ..EvaluationConfig::default()
        };
        state.apply(
            &event(
                "watching",
                EventType::EvalStarted,
                datetime!(2026-08-28 09:00:00 UTC),
                json!({ "suite": "s" }),
            ),
            &config,
        );
        for index in 0..5 {
            state.apply(
                &event(
                    &format!("done-{index}"),
                    EventType::EvalCompleted,
                    datetime!(2026-08-28 09:00:00 UTC),
                    json!({ "suite": "s" }),
                ),
                &config,
            );
        }

        assert!(state.detail("watching").is_some());
        assert!(state.len() <= 3, "held {} evaluations", state.len());
    }

    /// A report is in the window when it finished in it.
    ///
    /// Not when it started: a twenty-minute batch is normal here, and dating a
    /// report by its start would drop the run that has just told you something
    /// out of the view you opened to read it.
    #[test]
    fn the_window_dates_a_report_by_when_it_finished() {
        let state = fold(&[
            event(
                "eval-old",
                EventType::EvalCompleted,
                datetime!(2026-08-28 09:00:00 UTC),
                json!({ "suite": "alpha" }),
            ),
            event(
                "eval-long",
                EventType::EvalStarted,
                datetime!(2026-08-28 09:00:00 UTC),
                json!({ "suite": "alpha" }),
            ),
            event(
                "eval-long",
                EventType::EvalCompleted,
                datetime!(2026-08-28 11:55:00 UTC),
                json!({ "suite": "alpha" }),
            ),
        ]);

        let page = state.page(
            &EvaluationFilter {
                window_seconds: Some(900),
                ..EvaluationFilter::default()
            },
            datetime!(2026-08-28 12:00:00 UTC),
        );

        assert_eq!(
            page.evaluations
                .iter()
                .map(|row| row.evaluation_id.as_str())
                .collect::<Vec<_>>(),
            vec!["eval-long"],
        );
    }

    #[test]
    fn the_list_pages_from_a_cursor_and_search_narrows_it() {
        let events: Vec<RecordedEvent> = (0..5)
            .map(|index| {
                event(
                    &format!("eval-{index}"),
                    EventType::EvalCompleted,
                    datetime!(2026-08-28 09:00:00 UTC) + time::Duration::minutes(index),
                    json!({ "suite": if index % 2 == 0 { "alpha" } else { "beta" } }),
                )
            })
            .collect();
        let state = fold(&events);

        let first = state.page(
            &EvaluationFilter {
                limit: Some(2),
                ..EvaluationFilter::default()
            },
            now(),
        );
        assert_eq!(first.evaluations.len(), 2);
        assert_eq!(first.evaluations[0].evaluation_id, "eval-4", "newest first");
        let cursor = first.next_cursor.clone().expect("more to read");

        let second = state.page(
            &EvaluationFilter {
                limit: Some(2),
                after: Some(cursor),
                ..EvaluationFilter::default()
            },
            now(),
        );
        assert_eq!(second.evaluations[0].evaluation_id, "eval-2");

        let searched = state.page(
            &EvaluationFilter {
                search: Some("ALPHA".to_owned()),
                ..EvaluationFilter::default()
            },
            now(),
        );
        assert_eq!(searched.total_known, 3);
    }
}
