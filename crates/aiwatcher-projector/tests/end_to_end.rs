// See the note in aiwatcher-bus/tests.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! The whole pipeline, end to end, over the in-memory bus.
//!
//! These are the tests that would catch a regression in the at-least-once
//! contract: what happens on a redelivery, what happens when the trace store is
//! down, and whether the checkpoint moves when it should not.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use aiwatcher_bus::adapters::memory::InMemoryBus;
use aiwatcher_bus::{Checkpointer, MessageSink, StartFrom};
use aiwatcher_core::ports::{
    CompletedSpan, MetricSample, MetricSink, PortError, PortResult, TraceStore,
};
use aiwatcher_core::{Checkpoint, EventEnvelope, EventType, Sdk, Source};
use aiwatcher_projector::pipeline::Outputs;
use aiwatcher_projector::{
    EvaluationStatus, InMemoryDeadLetters, LiveHub, Projector, ProjectorConfig, ReadModel,
    RunStatus,
};
use aiwatcher_trace::AssemblerConfig;

/// A trace store that records what it was asked to write and can be told to
/// fail.
#[derive(Debug, Default)]
struct RecordingTraceStore {
    written: Mutex<Vec<CompletedSpan>>,
    failure: Mutex<Option<PortError>>,
}

impl RecordingTraceStore {
    async fn fail_with(&self, error: PortError) {
        *self.failure.lock().await = Some(error);
    }

    async fn recover(&self) {
        *self.failure.lock().await = None;
    }

    async fn written(&self) -> Vec<CompletedSpan> {
        self.written.lock().await.clone()
    }
}

#[async_trait::async_trait]
impl TraceStore for RecordingTraceStore {
    async fn write_spans(&self, spans: Vec<CompletedSpan>) -> PortResult<()> {
        if let Some(error) = self.failure.lock().await.as_ref() {
            return Err(match error {
                PortError::Unavailable { target, message } => PortError::Unavailable {
                    target,
                    message: message.clone(),
                },
                PortError::Rejected { target, message } => PortError::Rejected {
                    target,
                    message: message.clone(),
                },
                other => PortError::Rejected {
                    target: "test",
                    message: other.to_string(),
                },
            });
        }
        self.written.lock().await.extend(spans);
        Ok(())
    }
}

#[derive(Debug, Default)]
struct RecordingMetricSink {
    samples: Mutex<Vec<MetricSample>>,
}

impl RecordingMetricSink {
    async fn named(&self, name: &str) -> Vec<MetricSample> {
        self.samples
            .lock()
            .await
            .iter()
            .filter(|sample| sample.name == name)
            .cloned()
            .collect()
    }
}

#[async_trait::async_trait]
impl MetricSink for RecordingMetricSink {
    async fn record(&self, samples: Vec<MetricSample>) -> PortResult<()> {
        self.samples.lock().await.extend(samples);
        Ok(())
    }
}

struct Harness {
    bus: Arc<InMemoryBus>,
    traces: Arc<RecordingTraceStore>,
    metrics: Arc<RecordingMetricSink>,
    dead_letters: Arc<InMemoryDeadLetters>,
    read_model: Arc<ReadModel>,
    live: Arc<LiveHub>,
    shutdown: CancellationToken,
    handle: tokio::task::JoinHandle<()>,
}

impl Harness {
    async fn start() -> Self {
        Self::start_with(ProjectorConfig {
            flush_interval: Duration::from_millis(20),
            sweep_interval: Duration::from_millis(50),
            flush_batch_size: 8,
            cold_start: StartFrom::Beginning,
            retry: aiwatcher_projector::retry::RetryPolicy {
                max_attempts: 2,
                base_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(5),
            },
            assembler: AssemblerConfig {
                orphan_timeout: time::Duration::milliseconds(200),
                ..AssemblerConfig::default()
            },
            ..ProjectorConfig::default()
        })
        .await
    }

    async fn start_with(config: ProjectorConfig) -> Self {
        let bus = Arc::new(InMemoryBus::new());
        let traces = Arc::new(RecordingTraceStore::default());
        let metrics = Arc::new(RecordingMetricSink::default());
        let dead_letters = Arc::new(InMemoryDeadLetters::new());
        let read_model = Arc::new(ReadModel::default());
        let live = Arc::new(LiveHub::default());

        let projector = Arc::new(Projector::new(
            Arc::clone(&bus),
            Arc::clone(&bus),
            Outputs {
                live: Arc::clone(&live) as _,
                traces: Arc::clone(&traces) as _,
                metrics: Arc::clone(&metrics) as _,
                dead_letters: Arc::clone(&dead_letters) as _,
                read_model: Arc::clone(&read_model),
            },
            config,
        ));

        let shutdown = CancellationToken::new();
        let handle = {
            let shutdown = shutdown.clone();
            tokio::spawn(async move {
                projector.run(shutdown).await.expect("projector runs");
            })
        };

        Self {
            bus,
            traces,
            metrics,
            dead_letters,
            read_model,
            live,
            shutdown,
            handle,
        }
    }

    async fn stop(self) {
        self.shutdown.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.handle).await;
    }

    /// Poll until `condition` holds or the deadline passes.
    async fn until<F, Fut>(&self, label: &str, mut condition: F)
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = bool>,
    {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if condition().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for: {label}");
    }
}

fn event(
    event_type: EventType,
    run_id: &str,
    agent: Option<&str>,
    data: serde_json::Value,
) -> EventEnvelope {
    let mut envelope = EventEnvelope::new(
        event_type,
        run_id,
        time::OffsetDateTime::now_utc(),
        Source::new("python-agent-service", Sdk::Python),
    )
    .with_data(data);
    envelope.agent_id = agent.map(ToOwned::to_owned);
    envelope
}

/// Same as [`event`], with the producer-supplied id that deduplication needs.
fn identified_event(
    event_id: &str,
    event_type: EventType,
    run_id: &str,
    agent: Option<&str>,
    data: serde_json::Value,
) -> EventEnvelope {
    let mut envelope = event(event_type, run_id, agent, data);
    envelope.event_id = Some(aiwatcher_core::MessageId::new(event_id));
    envelope
}

fn complete_run(run_id: &str) -> Vec<EventEnvelope> {
    vec![
        identified_event(
            &format!("{run_id}-1"),
            EventType::RunStarted,
            run_id,
            None,
            json!({}),
        ),
        identified_event(
            &format!("{run_id}-2"),
            EventType::AgentStarted,
            run_id,
            Some("researcher"),
            json!({}),
        ),
        identified_event(
            &format!("{run_id}-3"),
            EventType::LlmStarted,
            run_id,
            Some("researcher"),
            json!({ "call_id": "c1", "provider": "anthropic", "model": "claude-opus-5" }),
        ),
        identified_event(
            &format!("{run_id}-4"),
            EventType::LlmCompleted,
            run_id,
            Some("researcher"),
            json!({
                "call_id": "c1",
                "provider": "anthropic",
                "model": "claude-opus-5",
                "prompt_tokens": 812,
                "completion_tokens": 193,
                "cached_tokens": 400
            }),
        ),
        identified_event(
            &format!("{run_id}-5"),
            EventType::AgentCompleted,
            run_id,
            Some("researcher"),
            json!({}),
        ),
        identified_event(
            &format!("{run_id}-6"),
            EventType::RunCompleted,
            run_id,
            None,
            json!({ "status": "succeeded" }),
        ),
    ]
}

#[tokio::test]
async fn a_run_flows_from_the_log_to_spans_metrics_and_the_read_model() {
    let harness = Harness::start().await;
    harness
        .bus
        .append(complete_run("run-1"))
        .await
        .expect("appends");

    harness
        .until("three spans written", || async {
            harness.traces.written().await.len() == 3
        })
        .await;

    let spans = harness.traces.written().await;
    let names: Vec<_> = spans.iter().map(|span| span.name.as_str()).collect();
    assert!(names.contains(&"run"), "got {names:?}");
    assert!(names.contains(&"invoke_agent researcher"), "got {names:?}");
    assert!(names.contains(&"chat claude-opus-5"), "got {names:?}");

    harness
        .until("the run is marked succeeded", || async {
            harness
                .read_model
                .run("run-1")
                .await
                .is_some_and(|detail| detail.summary.status == RunStatus::Succeeded)
        })
        .await;

    let detail = harness.read_model.run("run-1").await.expect("the run");
    assert_eq!(detail.summary.llm_calls, 1);
    assert_eq!(detail.summary.input_tokens, 812);
    assert_eq!(detail.summary.output_tokens, 193);
    assert_eq!(detail.summary.cached_tokens, 400);
    assert_eq!(detail.summary.agents, vec!["researcher".to_owned()]);
    assert_eq!(detail.summary.event_count, 6);
    assert_eq!(detail.spans.len(), 3, "the waterfall is served from memory");

    let tokens = harness.metrics.named("gen_ai.client.token.usage").await;
    assert_eq!(tokens.len(), 3, "input, output, cached");

    harness.stop().await;
}

#[tokio::test]
async fn the_checkpoint_advances_so_a_restart_does_not_reprocess() {
    let harness = Harness::start().await;
    harness
        .bus
        .append(complete_run("run-1"))
        .await
        .expect("appends");

    harness
        .until("the checkpoint reaches the last event", || async {
            harness
                .bus
                .load("aiwatcher-projector")
                .await
                .expect("reads")
                .is_some_and(|checkpoint| checkpoint == Checkpoint::from_global_position(6))
        })
        .await;

    harness.stop().await;
}

#[tokio::test]
async fn a_redelivered_event_does_not_double_count_tokens() {
    let harness = Harness::start().await;
    let events = complete_run("run-1");
    let completion = events[3].clone();

    harness.bus.append(events).await.expect("appends");
    harness
        .until("the first pass is folded in", || async {
            harness
                .read_model
                .run("run-1")
                .await
                .is_some_and(|detail| detail.summary.input_tokens == 812)
        })
        .await;

    // Republish the same event id: this is what a broker redelivery looks like.
    harness
        .bus
        .append(vec![completion])
        .await
        .expect("redelivers");
    tokio::time::sleep(Duration::from_millis(150)).await;

    let detail = harness.read_model.run("run-1").await.expect("the run");
    assert_eq!(
        detail.summary.input_tokens, 812,
        "a redelivery must not inflate the token count"
    );
    assert_eq!(
        harness
            .metrics
            .named("aiwatcher.events.deduplicated")
            .await
            .len(),
        1,
        "and the drop is visible as a metric"
    );

    harness.stop().await;
}

#[tokio::test]
async fn a_transient_trace_store_failure_holds_the_checkpoint_back() {
    let harness = Harness::start().await;
    harness
        .traces
        .fail_with(PortError::Unavailable {
            target: "test",
            message: "connection refused".to_owned(),
        })
        .await;

    harness
        .bus
        .append(complete_run("run-1"))
        .await
        .expect("appends");

    // Give the pipeline time to try, fail, and retry.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        harness.traces.written().await.len(),
        0,
        "nothing was written"
    );
    assert!(
        harness
            .bus
            .load("aiwatcher-projector")
            .await
            .expect("reads")
            .is_none(),
        "the checkpoint must not advance past an unwritten batch"
    );
    assert!(
        harness.dead_letters.is_empty().await,
        "a transient failure is retried, not parked"
    );

    harness.stop().await;
}

#[tokio::test]
async fn a_rejected_batch_is_parked_rather_than_retried_forever() {
    let harness = Harness::start().await;
    harness
        .traces
        .fail_with(PortError::Rejected {
            target: "test",
            message: "400 malformed span".to_owned(),
        })
        .await;

    harness
        .bus
        .append(complete_run("run-1"))
        .await
        .expect("appends");

    harness
        .until("the batch is parked", || async {
            !harness.dead_letters.is_empty().await
        })
        .await;

    let parked = harness.dead_letters.parked().await;
    assert!(parked[0].reason.contains("400 malformed span"));

    harness.traces.recover().await;
    harness.stop().await;
}

#[tokio::test]
async fn every_event_reaches_the_live_channel_before_storage() {
    let harness = Harness::start().await;
    harness
        .bus
        .append(complete_run("run-1"))
        .await
        .expect("appends");

    harness
        .until("six events are buffered for reconnects", || async {
            harness
                .live
                .replay_after(&Checkpoint::beginning())
                .await
                .map(|events| events.len())
                .unwrap_or(0)
                == 6
        })
        .await;

    let replayed = harness
        .live
        .replay_after(&Checkpoint::from_global_position(4))
        .await
        .expect("within the buffer");
    assert_eq!(replayed.len(), 2, "a reconnect at 4 gets events 5 and 6");
    assert_eq!(replayed[0].event_type, EventType::AgentCompleted);

    harness.stop().await;
}

#[tokio::test]
async fn an_abandoned_run_is_swept_and_shows_as_failed_spans() {
    let harness = Harness::start().await;
    harness
        .bus
        .append(vec![
            event(EventType::RunStarted, "run-abandoned", None, json!({})),
            event(
                EventType::LlmStarted,
                "run-abandoned",
                Some("a"),
                json!({ "call_id": "c1", "model": "m" }),
            ),
        ])
        .await
        .expect("appends");

    harness
        .until("the sweeper closes both spans", || async {
            harness.traces.written().await.len() == 2
        })
        .await;

    let spans = harness.traces.written().await;
    for span in &spans {
        let closed_by = span.attributes.iter().find_map(|(key, value)| {
            (key == "aiwatcher.span.closed_by").then(|| format!("{value:?}"))
        });
        assert!(
            closed_by.is_some_and(|value| value.contains("timeout")),
            "{} should be marked as swept",
            span.name
        );
    }

    harness.stop().await;
}

#[tokio::test]
async fn the_runs_list_filters_and_pages() {
    let harness = Harness::start().await;
    for index in 0..5 {
        harness
            .bus
            .append(complete_run(&format!("run-{index}")))
            .await
            .expect("appends");
    }
    harness
        .bus
        .append(vec![event(
            EventType::RunStarted,
            "run-live",
            Some("other"),
            json!({}),
        )])
        .await
        .expect("appends");

    harness
        .until("all six runs are known", || async {
            harness.read_model.len().await == 6
        })
        .await;

    let running = harness
        .read_model
        .list(&aiwatcher_projector::RunFilter {
            status: Some(RunStatus::Running),
            ..Default::default()
        })
        .await;
    assert_eq!(running.runs.len(), 1);
    assert_eq!(running.runs[0].run_id, "run-live");

    let by_agent = harness
        .read_model
        .list(&aiwatcher_projector::RunFilter {
            agent_id: Some("researcher".to_owned()),
            ..Default::default()
        })
        .await;
    assert_eq!(by_agent.runs.len(), 5);

    let by_model = harness
        .read_model
        .list(&aiwatcher_projector::RunFilter {
            model: Some("claude-opus-5".to_owned()),
            ..Default::default()
        })
        .await;
    assert_eq!(
        by_model.runs.len(),
        5,
        "the five complete runs each made a claude-opus-5 call; the bare one did not"
    );

    let unknown_model = harness
        .read_model
        .list(&aiwatcher_projector::RunFilter {
            model: Some("gpt-5".to_owned()),
            ..Default::default()
        })
        .await;
    assert!(
        unknown_model.runs.is_empty(),
        "a model nothing called matches nothing, rather than everything"
    );

    let first_page = harness
        .read_model
        .list(&aiwatcher_projector::RunFilter {
            limit: Some(2),
            ..Default::default()
        })
        .await;
    assert_eq!(first_page.runs.len(), 2);
    let cursor = first_page.next_cursor.clone().expect("more pages");

    let second_page = harness
        .read_model
        .list(&aiwatcher_projector::RunFilter {
            limit: Some(2),
            before: Some(cursor),
            ..Default::default()
        })
        .await;
    assert_eq!(second_page.runs.len(), 2);
    assert!(
        second_page
            .runs
            .iter()
            .all(|run| !first_page.runs.iter().any(|seen| seen.run_id == run.run_id)),
        "pages must not overlap"
    );

    harness.stop().await;
}

/// A restart must come back with its read model populated.
///
/// The read model and the live buffer are in memory; resuming from the stored
/// checkpoint would bring the process back with an empty runs list and an empty
/// metrics view while the events sat in the log unread. Replaying rebuilds
/// them, and the derived span ids make the re-export an overwrite.
#[tokio::test]
async fn a_restart_rebuilds_the_read_model_from_the_log() {
    let bus = Arc::new(InMemoryBus::new());
    bus.append(complete_run("run-1")).await.expect("appends");
    bus.append(complete_run("run-2")).await.expect("appends");
    // A checkpoint as far as it goes: a resuming projector would read nothing.
    bus.save("aiwatcher-projector", &Checkpoint::from_global_position(12))
        .await
        .expect("checkpoints");

    let read_model = Arc::new(ReadModel::default());
    let traces = Arc::new(RecordingTraceStore::default());
    let projector = Arc::new(Projector::new(
        Arc::clone(&bus),
        Arc::clone(&bus),
        Outputs {
            live: Arc::new(LiveHub::default()) as _,
            traces: Arc::clone(&traces) as _,
            metrics: Arc::new(RecordingMetricSink::default()) as _,
            dead_letters: Arc::new(InMemoryDeadLetters::new()) as _,
            read_model: Arc::clone(&read_model),
        },
        ProjectorConfig {
            flush_interval: Duration::from_millis(20),
            rebuild_on_start: true,
            ..ProjectorConfig::default()
        },
    ));

    let shutdown = CancellationToken::new();
    let handle = {
        let shutdown = shutdown.clone();
        tokio::spawn(async move { projector.run(shutdown).await.expect("runs") })
    };

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while read_model.len().await < 2 && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert_eq!(
        read_model.len().await,
        2,
        "both runs are back despite the checkpoint being past them"
    );
    let metrics = read_model
        .metrics(&aiwatcher_projector::MetricsFilter::default())
        .await;
    assert_eq!(metrics.totals.runs, 2);
    assert_eq!(metrics.totals.input_tokens, 1624, "812 per run");

    shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
}

#[tokio::test]
async fn a_shutdown_writes_the_spans_that_were_still_open() {
    let harness = Harness::start_with(ProjectorConfig {
        flush_interval: Duration::from_millis(20),
        // Long enough that the sweeper will not fire during the test.
        sweep_interval: Duration::from_secs(60),
        cold_start: StartFrom::Beginning,
        ..ProjectorConfig::default()
    })
    .await;

    harness
        .bus
        .append(vec![
            event(EventType::RunStarted, "run-open", None, json!({})),
            event(EventType::AgentStarted, "run-open", Some("a"), json!({})),
        ])
        .await
        .expect("appends");

    harness
        .until("the events are ingested", || async {
            harness.read_model.len().await == 1
        })
        .await;
    assert_eq!(
        harness.traces.written().await.len(),
        0,
        "nothing has ended yet"
    );

    let traces = Arc::clone(&harness.traces);
    harness.stop().await;
    assert_eq!(
        traces.written().await.len(),
        2,
        "the drain on shutdown wrote both open spans"
    );
}

/// An evaluation report goes through the same pipeline as a run and comes out
/// somewhere else entirely: the evaluation projection, and nothing in the
/// trace store.
///
/// This is the whole point of the split. The at-least-once machinery — the
/// log, deduplication, the checkpoint, the live fan-out — is worth reusing for
/// a report. Span assembly is not.
#[tokio::test]
async fn an_evaluation_report_reaches_the_projection_and_never_the_trace_store() {
    let harness = Harness::start().await;

    harness
        .bus
        .append(vec![
            identified_event(
                "eval-1",
                EventType::EvalStarted,
                "nightly-2026-08-28",
                None,
                json!({ "suite": "catalog", "dataset": "cases@3", "params": { "model": "gpt-5-mini" } }),
            ),
            identified_event(
                "eval-2",
                EventType::EvalCase,
                "nightly-2026-08-28",
                None,
                json!({ "case_id": "K-1", "passed": false, "score": 0.4 }),
            ),
            identified_event(
                "eval-3",
                EventType::EvalCompleted,
                "nightly-2026-08-28",
                None,
                json!({ "metrics": { "mean_score": 0.4 }, "report": { "failures": ["K-1"] } }),
            ),
        ])
        .await
        .expect("appends");

    harness
        .until("the evaluation to be projected", || async {
            harness
                .read_model
                .evaluation("nightly-2026-08-28")
                .await
                .is_some_and(|detail| detail.summary.status == EvaluationStatus::Succeeded)
        })
        .await;

    let detail = harness
        .read_model
        .evaluation("nightly-2026-08-28")
        .await
        .expect("the evaluation");
    assert_eq!(detail.summary.suite, "catalog");
    assert_eq!(detail.summary.metrics["mean_score"], 0.4);
    assert_eq!(detail.cases.len(), 1);
    assert!(detail.report.is_some());

    assert!(
        harness.traces.written().await.is_empty(),
        "a report is not a trace"
    );
    assert!(
        harness.read_model.is_empty().await,
        "and it is not a run either"
    );

    harness.stop().await;
}
