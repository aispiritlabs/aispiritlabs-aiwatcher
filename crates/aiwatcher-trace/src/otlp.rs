//! OTLP over HTTP with a JSON body.
//!
//! Both VictoriaTraces (`/opentelemetry/v1/traces`) and the OpenTelemetry
//! Collector (`/v1/traces`) accept `application/json` OTLP, so the same
//! exporter reaches either. Point it at the Collector in production and
//! straight at VictoriaTraces when you want one less moving part.
//!
//! ## Encoding rules that are easy to get wrong
//!
//! * Trace and span ids are **lowercase hex strings** in OTLP/JSON, not the
//!   base64 the protobuf mapping would suggest.
//! * Timestamps are **strings** holding nanoseconds since the Unix epoch.
//!   JSON numbers cannot hold a nanosecond timestamp without precision loss,
//!   which is why the spec makes them strings.
//! * `kind` and `code` are the protobuf enum *numbers*.
//!
//! Getting any of these wrong produces a 200 and no data, so they are asserted
//! in the tests below.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::{Value, json};
use time::OffsetDateTime;

use aiwatcher_core::ports::{
    Attr, AttrValue, CompletedSpan, MetricKind, MetricSample, PortError, PortResult, SpanKind,
    SpanStatus, TraceStore,
};
use aiwatcher_core::ports::{MetricSink, SpanEvent};

/// Buckets for the latency histograms, in seconds. Chosen for LLM calls: sub-
/// second is rare, ten seconds is normal, a minute happens.
const LATENCY_BUCKETS: &[f64] = &[
    0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 30.0, 60.0, 120.0, 300.0,
];

/// Buckets for token counts.
const TOKEN_BUCKETS: &[f64] = &[
    16.0,
    64.0,
    256.0,
    1024.0,
    4096.0,
    16_384.0,
    65_536.0,
    262_144.0,
    1_048_576.0,
];

#[derive(Clone, Debug)]
pub struct OtlpConfig {
    /// Base URL, e.g. `http://otel-collector:4318` or
    /// `http://victoriatraces:10428/opentelemetry`.
    pub endpoint: String,
    pub service_name: String,
    pub timeout: Duration,
}

impl OtlpConfig {
    pub fn new(endpoint: impl Into<String>, service_name: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            service_name: service_name.into(),
            timeout: Duration::from_secs(10),
        }
    }

    fn url(&self, signal: &str) -> String {
        format!("{}/v1/{signal}", self.endpoint.trim_end_matches('/'))
    }
}

/// Writes finished spans to an OTLP endpoint.
#[derive(Debug)]
pub struct OtlpTraceStore {
    client: reqwest::Client,
    config: OtlpConfig,
}

impl OtlpTraceStore {
    pub fn new(config: OtlpConfig) -> PortResult<Self> {
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|source| PortError::Other {
                target: "otlp-traces",
                source: Box::new(source),
            })?;
        Ok(Self { client, config })
    }

    /// The request body, exposed so the encoding can be asserted without a
    /// server.
    #[must_use]
    pub fn encode(&self, spans: &[CompletedSpan]) -> Value {
        json!({
            "resourceSpans": [{
                "resource": { "attributes": resource_attributes(&self.config.service_name) },
                "scopeSpans": [{
                    "scope": { "name": "aiwatcher", "version": env!("CARGO_PKG_VERSION") },
                    "spans": spans.iter().map(encode_span).collect::<Vec<_>>(),
                }],
            }],
        })
    }
}

#[async_trait]
impl TraceStore for OtlpTraceStore {
    async fn write_spans(&self, spans: Vec<CompletedSpan>) -> PortResult<()> {
        if spans.is_empty() {
            return Ok(());
        }
        let body = self.encode(&spans);
        post(
            &self.client,
            &self.config.url("traces"),
            &body,
            "otlp-traces",
        )
        .await
    }
}

/// One metric series: its name plus its attribute set.
///
/// Attributes are rendered in the order they were built, which is stable
/// because they come from the same code path every time.
type SeriesKey = (String, String);

/// The running total for one series.
#[derive(Clone, Debug)]
struct Accumulated {
    /// When this series was first observed. OTLP wants it on every point.
    start: OffsetDateTime,
    count: u64,
    sum: f64,
    min: f64,
    max: f64,
    /// One counter per bucket, plus the overflow bucket. Empty for counters.
    buckets: Vec<u64>,
}

/// Writes aggregates to an OTLP endpoint, accumulated into cumulative series.
///
/// ## Why it accumulates rather than sending each observation
///
/// The obvious encoding — one delta point per observation — loses data. Two LLM
/// calls in one flush produce two samples with the *same* series and, at
/// millisecond resolution, the same timestamp; a time-series database keyed by
/// (series, timestamp) keeps one of them. The symptom is a token counter that
/// silently reports the last call in each batch instead of the sum, which is
/// exactly the number a cost dashboard is built on.
///
/// So this keeps a running total per series and exports **cumulative**
/// temporality, which is what a metrics backend expects and what makes
/// `rate()`/`increase()` behave. The state is per-process and resets on
/// restart; a cumulative series that resets is a case every Prometheus-lineage
/// backend already handles.
#[derive(Debug)]
pub struct OtlpMetricSink {
    client: reqwest::Client,
    config: OtlpConfig,
    /// Cumulative state. A `Mutex` rather than a lock-free map because this is
    /// touched once per flush, not once per event.
    series: Mutex<HashMap<SeriesKey, Accumulated>>,
}

impl OtlpMetricSink {
    pub fn new(config: OtlpConfig) -> PortResult<Self> {
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|source| PortError::Other {
                target: "otlp-metrics",
                source: Box::new(source),
            })?;
        Ok(Self {
            client,
            config,
            series: Mutex::new(HashMap::new()),
        })
    }

    /// Fold `samples` into the running totals and render what to send.
    ///
    /// Gauges are passed through untouched: a gauge is a last-known value, and
    /// accumulating one would be meaningless.
    pub fn encode(&self, samples: &[MetricSample]) -> Value {
        let mut metrics = Vec::with_capacity(samples.len());
        let mut state = match self.series.lock() {
            Ok(state) => state,
            // A poisoned lock means a previous flush panicked. The totals are
            // still usable and losing them would be worse than the risk.
            Err(poisoned) => poisoned.into_inner(),
        };

        for sample in samples {
            if sample.kind == MetricKind::Gauge {
                metrics.push(encode_gauge(sample));
                continue;
            }

            let key = (sample.name.clone(), attribute_key(&sample.attributes));
            let bucket_count = match sample.kind {
                MetricKind::Histogram => bucket_bounds(&sample.name).len() + 1,
                _ => 0,
            };
            let entry = state.entry(key).or_insert_with(|| Accumulated {
                start: sample.at,
                count: 0,
                sum: 0.0,
                min: f64::INFINITY,
                max: f64::NEG_INFINITY,
                buckets: vec![0; bucket_count],
            });

            entry.count += 1;
            entry.sum += sample.value;
            entry.min = entry.min.min(sample.value);
            entry.max = entry.max.max(sample.value);
            if sample.kind == MetricKind::Histogram {
                let bounds = bucket_bounds(&sample.name);
                let index = bounds
                    .iter()
                    .position(|bound| sample.value <= *bound)
                    .unwrap_or(bounds.len());
                if let Some(slot) = entry.buckets.get_mut(index) {
                    *slot += 1;
                }
            }

            metrics.push(encode_cumulative(sample, entry));
        }

        json!({
            "resourceMetrics": [{
                "resource": { "attributes": resource_attributes(&self.config.service_name) },
                "scopeMetrics": [{
                    "scope": { "name": "aiwatcher", "version": env!("CARGO_PKG_VERSION") },
                    "metrics": metrics,
                }],
            }],
        })
    }
}

/// A stable string for an attribute set, used to identify a series.
fn attribute_key(attributes: &[Attr]) -> String {
    attributes
        .iter()
        .map(|(key, value)| format!("{key}={value:?}"))
        .collect::<Vec<_>>()
        .join(",")
}

#[async_trait]
impl MetricSink for OtlpMetricSink {
    async fn record(&self, samples: Vec<MetricSample>) -> PortResult<()> {
        if samples.is_empty() {
            return Ok(());
        }
        let body = self.encode(&samples);
        post(
            &self.client,
            &self.config.url("metrics"),
            &body,
            "otlp-metrics",
        )
        .await
    }
}

async fn post(
    client: &reqwest::Client,
    url: &str,
    body: &impl Serialize,
    target: &'static str,
) -> PortResult<()> {
    let response =
        client
            .post(url)
            .json(body)
            .send()
            .await
            .map_err(|source| PortError::Unavailable {
                target,
                message: source.to_string(),
            })?;

    let status = response.status();
    if status.is_success() {
        return Ok(());
    }

    let detail = response.text().await.unwrap_or_default();
    // 4xx means the payload is wrong and will still be wrong next time; 5xx and
    // timeouts are worth a retry. The pipeline reads `is_retryable` to decide.
    if status.is_client_error() {
        Err(PortError::Rejected {
            target,
            message: format!("{status}: {detail}"),
        })
    } else {
        Err(PortError::Unavailable {
            target,
            message: format!("{status}: {detail}"),
        })
    }
}

fn resource_attributes(service_name: &str) -> Vec<Value> {
    vec![
        key_value("service.name", &AttrValue::Str(service_name.to_owned())),
        key_value(
            "service.version",
            &AttrValue::Str(env!("CARGO_PKG_VERSION").to_owned()),
        ),
        key_value(
            "telemetry.sdk.name",
            &AttrValue::Str("aiwatcher".to_owned()),
        ),
        key_value("telemetry.sdk.language", &AttrValue::Str("rust".to_owned())),
    ]
}

/// Nanoseconds since the Unix epoch, as a string. OTLP/JSON requires the string
/// form: a JSON number loses the low digits of a nanosecond timestamp.
fn unix_nanos(at: OffsetDateTime) -> String {
    at.unix_timestamp_nanos().to_string()
}

fn key_value(key: &str, value: &AttrValue) -> Value {
    json!({ "key": key, "value": any_value(value) })
}

fn any_value(value: &AttrValue) -> Value {
    match value {
        AttrValue::Bool(inner) => json!({ "boolValue": inner }),
        // OTLP integers are int64 and travel as strings for the same reason
        // timestamps do.
        AttrValue::Int(inner) => json!({ "intValue": inner.to_string() }),
        AttrValue::Double(inner) => json!({ "doubleValue": inner }),
        AttrValue::Str(inner) => json!({ "stringValue": inner }),
        AttrValue::StrList(items) => json!({
            "arrayValue": {
                "values": items
                    .iter()
                    .map(|item| json!({ "stringValue": item }))
                    .collect::<Vec<_>>(),
            }
        }),
    }
}

fn attributes(attrs: &[Attr]) -> Vec<Value> {
    attrs
        .iter()
        .map(|(key, value)| key_value(key, value))
        .collect()
}

/// The protobuf enum values for `SpanKind`.
fn span_kind_code(kind: SpanKind) -> u8 {
    match kind {
        SpanKind::Internal => 1,
        SpanKind::Server => 2,
        SpanKind::Client => 3,
        SpanKind::Producer => 4,
        SpanKind::Consumer => 5,
    }
}

fn encode_span(span: &CompletedSpan) -> Value {
    let mut encoded = json!({
        "traceId": span.trace_id.to_hex(),
        "spanId": span.span_id.to_hex(),
        "name": span.name,
        "kind": span_kind_code(span.kind),
        "startTimeUnixNano": unix_nanos(span.start),
        "endTimeUnixNano": unix_nanos(span.end),
        "attributes": attributes(&span.attributes),
        "status": match &span.status {
            // 0 = unset, 1 = ok, 2 = error.
            SpanStatus::Unset => json!({ "code": 0 }),
            SpanStatus::Ok => json!({ "code": 1 }),
            SpanStatus::Error { message } => json!({ "code": 2, "message": message }),
        },
    });

    if let Some(parent) = span.parent_span_id {
        encoded["parentSpanId"] = json!(parent.to_hex());
    }
    if !span.events.is_empty() {
        encoded["events"] = json!(
            span.events
                .iter()
                .map(encode_span_event)
                .collect::<Vec<_>>()
        );
    }
    if !span.links.is_empty() {
        encoded["links"] = json!(
            span.links
                .iter()
                .map(|link| json!({
                    "traceId": link.trace_id.to_hex(),
                    "spanId": link.span_id.to_hex(),
                    "attributes": attributes(&link.attributes),
                }))
                .collect::<Vec<_>>()
        );
    }
    encoded
}

fn encode_span_event(event: &SpanEvent) -> Value {
    json!({
        "name": event.name,
        "timeUnixNano": unix_nanos(event.at),
        "attributes": attributes(&event.attributes),
    })
}

fn encode_gauge(sample: &MetricSample) -> Value {
    let nanos = unix_nanos(sample.at);
    let mut metric = json!({
        "name": sample.name,
        "gauge": {
            "dataPoints": [{
                "asDouble": sample.value,
                "timeUnixNano": nanos,
                "attributes": attributes(&sample.attributes),
            }],
        },
    });
    if let Some(unit) = &sample.unit {
        metric["unit"] = json!(unit);
    }
    metric
}

/// A counter or histogram point carrying the series' running total.
///
/// `aggregationTemporality: 2` is cumulative. The delta encoding (1) is the one
/// that loses same-timestamp observations — see [`OtlpMetricSink`].
fn encode_cumulative(sample: &MetricSample, state: &Accumulated) -> Value {
    let nanos = unix_nanos(sample.at);
    let start_nanos = unix_nanos(state.start);
    let point_attributes = attributes(&sample.attributes);

    let body = match sample.kind {
        MetricKind::Counter => json!({
            "sum": {
                "aggregationTemporality": 2,
                "isMonotonic": true,
                "dataPoints": [{
                    "asDouble": state.sum,
                    "timeUnixNano": nanos,
                    "startTimeUnixNano": start_nanos,
                    "attributes": point_attributes,
                }],
            }
        }),
        MetricKind::Histogram => json!({
            "histogram": {
                "aggregationTemporality": 2,
                "dataPoints": [{
                    "count": state.count.to_string(),
                    "sum": state.sum,
                    "min": state.min,
                    "max": state.max,
                    "bucketCounts": state
                        .buckets
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>(),
                    "explicitBounds": bucket_bounds(&sample.name),
                    "timeUnixNano": nanos,
                    "startTimeUnixNano": start_nanos,
                    "attributes": point_attributes,
                }],
            }
        }),
        // Handled before this is reached.
        MetricKind::Gauge => json!({}),
    };

    let mut metric = json!({ "name": sample.name });
    if let Some(unit) = &sample.unit {
        metric["unit"] = json!(unit);
    }
    if let Value::Object(fields) = body
        && let Value::Object(target) = &mut metric
    {
        target.extend(fields);
    }
    metric
}

fn bucket_bounds(metric_name: &str) -> &'static [f64] {
    if metric_name.contains("token") {
        TOKEN_BUCKETS
    } else {
        LATENCY_BUCKETS
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use aiwatcher_core::ports::attr;
    use aiwatcher_core::{SpanId, TraceId};

    use super::*;

    fn span() -> CompletedSpan {
        let trace_id = TraceId::derive("run-1");
        CompletedSpan {
            trace_id,
            span_id: SpanId::derive(trace_id, "llm:call-1"),
            parent_span_id: Some(SpanId::derive(trace_id, "agent:researcher")),
            name: "chat claude-opus-5".to_owned(),
            kind: SpanKind::Client,
            start: datetime!(2026-08-27 18:20:11.5 UTC),
            end: datetime!(2026-08-27 18:20:12.74 UTC),
            status: SpanStatus::Ok,
            attributes: vec![attr("gen_ai.usage.input_tokens", 812i64)],
            events: vec![SpanEvent {
                name: "gen_ai.first_token".to_owned(),
                at: datetime!(2026-08-27 18:20:11.9 UTC),
                attributes: Vec::new(),
            }],
            links: Vec::new(),
        }
    }

    fn store() -> OtlpTraceStore {
        OtlpTraceStore::new(OtlpConfig::new("http://localhost:4318", "aiwatcher"))
            .expect("client builds")
    }

    /// A sink with empty accumulation state, so each test starts from zero.
    fn metric_sink() -> OtlpMetricSink {
        OtlpMetricSink::new(OtlpConfig::new("http://localhost:4318", "aiwatcher"))
            .expect("client builds")
    }

    #[test]
    fn ids_are_encoded_as_lowercase_hex_not_base64() {
        let span = span();
        let encoded = store().encode(std::slice::from_ref(&span));
        let wire = &encoded["resourceSpans"][0]["scopeSpans"][0]["spans"][0];

        assert_eq!(wire["traceId"], span.trace_id.to_hex());
        assert_eq!(wire["spanId"], span.span_id.to_hex());
        assert_eq!(
            wire["parentSpanId"],
            span.parent_span_id.expect("has a parent").to_hex()
        );
        assert_eq!(
            wire["traceId"].as_str().expect("string").len(),
            32,
            "a trace id is 16 bytes of hex"
        );
    }

    #[test]
    fn timestamps_are_nanosecond_strings() {
        let encoded = store().encode(&[span()]);
        let wire = &encoded["resourceSpans"][0]["scopeSpans"][0]["spans"][0];

        // 18:20:11.5 UTC on 2026-08-27, to the nanosecond.
        let start = wire["startTimeUnixNano"].as_str().expect("a string");
        assert_eq!(start, "1787854811500000000");
        assert!(
            wire["endTimeUnixNano"].is_string(),
            "a JSON number would lose the low digits"
        );
        assert_eq!(
            wire["events"][0]["timeUnixNano"], "1787854811900000000",
            "span events carry the same encoding"
        );
    }

    #[test]
    fn kinds_and_statuses_use_the_protobuf_enum_numbers() {
        let encoded = store().encode(&[span()]);
        let wire = &encoded["resourceSpans"][0]["scopeSpans"][0]["spans"][0];
        assert_eq!(wire["kind"], 3, "client");
        assert_eq!(wire["status"]["code"], 1, "ok");

        let mut failed = span();
        failed.status = SpanStatus::Error {
            message: "rate limited".to_owned(),
        };
        failed.kind = SpanKind::Internal;
        let encoded = store().encode(&[failed]);
        let wire = &encoded["resourceSpans"][0]["scopeSpans"][0]["spans"][0];
        assert_eq!(wire["kind"], 1, "internal");
        assert_eq!(wire["status"]["code"], 2, "error");
        assert_eq!(wire["status"]["message"], "rate limited");
    }

    #[test]
    fn integer_attributes_travel_as_strings() {
        let encoded = store().encode(&[span()]);
        let attribute = &encoded["resourceSpans"][0]["scopeSpans"][0]["spans"][0]["attributes"][0];
        assert_eq!(attribute["key"], "gen_ai.usage.input_tokens");
        assert_eq!(
            attribute["value"]["intValue"], "812",
            "OTLP int64 is a JSON string"
        );
    }

    #[test]
    fn a_span_without_a_parent_omits_the_field_rather_than_sending_null() {
        let mut root = span();
        root.parent_span_id = None;
        let encoded = store().encode(&[root]);
        let wire = &encoded["resourceSpans"][0]["scopeSpans"][0]["spans"][0];
        assert!(
            wire.get("parentSpanId").is_none(),
            "a null parent id is rejected by some collectors"
        );
    }

    #[test]
    fn a_histogram_sample_lands_in_exactly_one_bucket() {
        let sink = metric_sink();
        let encoded = sink.encode(&[MetricSample {
            name: "gen_ai.client.operation.duration".to_owned(),
            kind: MetricKind::Histogram,
            value: 1.24,
            unit: Some("s".to_owned()),
            at: datetime!(2026-08-27 18:20:12 UTC),
            attributes: vec![attr("gen_ai.request.model", "claude-opus-5")],
        }]);
        let point = &encoded["resourceMetrics"][0]["scopeMetrics"][0]["metrics"][0]["histogram"]["dataPoints"]
            [0];

        assert_eq!(point["count"], "1");
        assert_eq!(point["sum"], 1.24);
        let counts: Vec<u64> = point["bucketCounts"]
            .as_array()
            .expect("an array")
            .iter()
            .map(|value| value.as_str().expect("a string").parse().expect("a number"))
            .collect();
        assert_eq!(counts.iter().sum::<u64>(), 1, "exactly one observation");
        // 1.24 falls in the (1.0, 2.0] bucket, index 5.
        assert_eq!(counts[5], 1);
    }

    #[test]
    fn token_histograms_get_token_shaped_buckets() {
        let sink = metric_sink();
        let encoded = sink.encode(&[MetricSample {
            name: "gen_ai.client.token.usage".to_owned(),
            kind: MetricKind::Histogram,
            value: 812.0,
            unit: Some("{token}".to_owned()),
            at: datetime!(2026-08-27 18:20:12 UTC),
            attributes: Vec::new(),
        }]);
        let bounds = &encoded["resourceMetrics"][0]["scopeMetrics"][0]["metrics"][0]["histogram"]["dataPoints"]
            [0]["explicitBounds"];
        assert_eq!(bounds[0], 16.0, "not the latency buckets");
    }

    #[test]
    fn a_counter_is_a_monotonic_cumulative_sum() {
        let sink = metric_sink();
        let encoded = sink.encode(&[MetricSample {
            name: "aiwatcher.spans.written".to_owned(),
            kind: MetricKind::Counter,
            value: 3.0,
            unit: None,
            at: datetime!(2026-08-27 18:20:12 UTC),
            attributes: Vec::new(),
        }]);
        let sum = &encoded["resourceMetrics"][0]["scopeMetrics"][0]["metrics"][0]["sum"];
        assert_eq!(sum["aggregationTemporality"], 2, "cumulative");
        assert_eq!(sum["isMonotonic"], true);
        assert_eq!(sum["dataPoints"][0]["asDouble"], 3.0);
    }

    /// The regression this exporter's accumulation exists for.
    ///
    /// Two LLM calls in one flush produce two samples with the same series and,
    /// at millisecond resolution, the same timestamp. Sent as separate delta
    /// points, a time-series database keyed by (series, timestamp) keeps one of
    /// them — so a token counter silently reports the last call in the batch
    /// instead of the sum, which is the number a cost dashboard is built on.
    #[test]
    fn two_observations_of_one_series_accumulate_rather_than_overwriting() {
        let sink = metric_sink();
        let sample = |value: f64| MetricSample {
            name: "gen_ai.client.token.usage".to_owned(),
            kind: MetricKind::Histogram,
            value,
            unit: Some("{token}".to_owned()),
            at: datetime!(2026-08-27 18:20:12 UTC),
            attributes: vec![attr("gen_ai.token.type", "input")],
        };

        // Both in one flush, exactly as two LLM calls in one batch arrive.
        let encoded = sink.encode(&[sample(812.0), sample(1420.0)]);
        let points = &encoded["resourceMetrics"][0]["scopeMetrics"][0]["metrics"];
        let last = &points[1]["histogram"]["dataPoints"][0];

        assert_eq!(last["count"], "2", "both observations were counted");
        assert_eq!(last["sum"], 2232.0, "812 + 1420, not just the last one");
        assert_eq!(last["min"], 812.0);
        assert_eq!(last["max"], 1420.0);
        assert_eq!(
            points[0]["histogram"]["dataPoints"][0]["sum"], 812.0,
            "the first point carries the total as of that observation"
        );

        // And across flushes, which is how a second run's tokens arrive.
        let encoded = sink.encode(&[sample(100.0)]);
        let histogram =
            &encoded["resourceMetrics"][0]["scopeMetrics"][0]["metrics"][0]["histogram"];
        assert_eq!(histogram["aggregationTemporality"], 2, "cumulative");
        assert_eq!(histogram["dataPoints"][0]["count"], "3");
        assert_eq!(histogram["dataPoints"][0]["sum"], 2332.0);
    }

    #[test]
    fn different_attribute_sets_are_different_series() {
        let sink = metric_sink();
        let sample = |token_type: &str, value: f64| MetricSample {
            name: "gen_ai.client.token.usage".to_owned(),
            kind: MetricKind::Histogram,
            value,
            unit: None,
            at: datetime!(2026-08-27 18:20:12 UTC),
            attributes: vec![attr("gen_ai.token.type", token_type)],
        };

        let encoded = sink.encode(&[sample("input", 800.0), sample("output", 200.0)]);
        let metrics = &encoded["resourceMetrics"][0]["scopeMetrics"][0]["metrics"];
        assert_eq!(metrics[0]["histogram"]["dataPoints"][0]["sum"], 800.0);
        assert_eq!(
            metrics[1]["histogram"]["dataPoints"][0]["sum"], 200.0,
            "output must not inherit input's running total"
        );
    }

    #[test]
    fn a_gauge_reports_the_latest_value_rather_than_a_total() {
        let sink = metric_sink();
        let sample = |value: f64| MetricSample {
            name: "aiwatcher.spans.open".to_owned(),
            kind: MetricKind::Gauge,
            value,
            unit: None,
            at: datetime!(2026-08-27 18:20:12 UTC),
            attributes: Vec::new(),
        };

        sink.encode(&[sample(7.0)]);
        let encoded = sink.encode(&[sample(3.0)]);
        let point = &encoded["resourceMetrics"][0]["scopeMetrics"][0]["metrics"][0]["gauge"]["dataPoints"]
            [0];
        assert_eq!(
            point["asDouble"], 3.0,
            "accumulating a gauge would be meaningless"
        );
    }

    #[test]
    fn the_endpoint_is_suffixed_per_signal() {
        let config = OtlpConfig::new("http://victoriatraces:10428/opentelemetry/", "aiwatcher");
        assert_eq!(
            config.url("traces"),
            "http://victoriatraces:10428/opentelemetry/v1/traces"
        );
        assert_eq!(
            config.url("metrics"),
            "http://victoriatraces:10428/opentelemetry/v1/metrics"
        );
    }
}
