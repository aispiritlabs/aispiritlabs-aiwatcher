//! The OpenAPI document.
//!
//! This is the contract the panel is generated from: `@hey-api/openapi-ts`
//! reads it and emits typed TypeScript clients, so a field renamed in Rust
//! becomes a compile error in the frontend rather than an `undefined` at
//! runtime. `cargo run --bin aiwatcher-openapi` writes it to
//! `contracts/openapi.json`, and CI fails if that file is stale.

use utoipa::OpenApi;

#[derive(Debug, OpenApi)]
#[openapi(
    info(
        title = "aiwatcher",
        version = env!("CARGO_PKG_VERSION"),
        description = "Observability for agent runs: events in, traces and live streams out.",
    ),
    paths(
        crate::routes::list_conversations,
        crate::routes::list_dimension,
        crate::routes::list_spans,
        crate::routes::list_evaluations,
        crate::routes::get_evaluation,
        crate::routes::list_evaluation_suites,
        crate::routes::get_metrics,
        crate::routes::list_runs,
        crate::routes::get_run,
        crate::routes::get_run_events,
        crate::routes::stream_run,
        crate::routes::live_websocket,
        crate::prompts::list_prompts,
        crate::prompts::publish_prompt,
        crate::prompts::get_prompt,
        crate::prompts::get_prompt_version,
        crate::prompts::set_prompt_label,
        crate::prompts::record_optimization,
        crate::prompts::get_optimization,
        crate::prompts::rebuild_prompt,
        crate::workflows::list_workflows,
        crate::workflows::get_workflow,
        crate::workflows::list_workflow_executions,
        crate::workflows::get_workflow_execution,
        crate::workflows::stream_workflow_execution,
        crate::workflows::rerun_workflow,
        crate::routes::ingest,
        crate::routes::livez,
        crate::routes::readyz,
    ),
    components(schemas(
        aiwatcher_core::EventEnvelope,
        aiwatcher_core::RecordedEvent,
        aiwatcher_core::RecordedMetadata,
        aiwatcher_core::MessageKind,
        aiwatcher_core::Source,
        aiwatcher_core::Sdk,
        aiwatcher_core::EventType,
        aiwatcher_core::Checkpoint,
        aiwatcher_core::StreamName,
        aiwatcher_core::TraceId,
        aiwatcher_core::SpanId,
        aiwatcher_core::ports::LiveEvent,
        aiwatcher_projector::RunSummary,
        aiwatcher_projector::RunDetail,
        aiwatcher_projector::RunPage,
        aiwatcher_projector::RunStatus,
        aiwatcher_projector::ConversationPage,
        aiwatcher_projector::ConversationSummary,
        aiwatcher_projector::DimensionKind,
        aiwatcher_projector::DimensionPage,
        aiwatcher_projector::DimensionSummary,
        aiwatcher_projector::SpanPage,
        aiwatcher_projector::SpanRow,
        aiwatcher_projector::SpanOutcome,
        aiwatcher_core::ports::SpanKind,
        aiwatcher_core::ports::SpanStatus,
        aiwatcher_projector::EvaluationPage,
        aiwatcher_projector::EvaluationSummary,
        aiwatcher_projector::EvaluationDetail,
        aiwatcher_projector::EvaluationCase,
        aiwatcher_projector::EvaluationComparison,
        aiwatcher_projector::EvaluationStatus,
        aiwatcher_projector::evaluations::MetricDelta,
        aiwatcher_projector::evaluations::CaseDelta,
        aiwatcher_projector::SuitePage,
        aiwatcher_projector::SuiteSummary,
        aiwatcher_projector::WorkflowPage,
        aiwatcher_projector::WorkflowDefinition,
        aiwatcher_projector::WorkflowNode,
        aiwatcher_projector::WorkflowEdge,
        aiwatcher_projector::ExecutionPage,
        aiwatcher_projector::ExecutionSummary,
        aiwatcher_projector::ExecutionDetail,
        aiwatcher_projector::ExecutionStatus,
        aiwatcher_projector::NodeState,
        aiwatcher_projector::NodeStatus,
        aiwatcher_projector::Artifact,
        aiwatcher_projector::AgentMessage,
        aiwatcher_core::ports::RerunRequest,
        aiwatcher_core::ports::RerunAccepted,
        crate::workflows::RerunBody,
        aiwatcher_projector::MetricsSummary,
        aiwatcher_projector::metrics::MetricsWindow,
        aiwatcher_projector::metrics::Totals,
        aiwatcher_projector::metrics::Latency,
        aiwatcher_projector::metrics::Percentiles,
        aiwatcher_projector::metrics::AgentBreakdown,
        aiwatcher_projector::metrics::ModelBreakdown,
        aiwatcher_projector::metrics::ToolBreakdown,
        aiwatcher_projector::metrics::StepBreakdown,
        aiwatcher_projector::metrics::Bucket,
        aiwatcher_core::prompts::PromptName,
        aiwatcher_core::prompts::PromptVersionId,
        aiwatcher_core::prompts::PromptVersion,
        aiwatcher_core::prompts::PromptVersionSummary,
        aiwatcher_core::prompts::PromptHead,
        aiwatcher_core::prompts::PromptSummary,
        aiwatcher_core::prompts::VersionOrigin,
        aiwatcher_core::prompts::OptimizationRecord,
        aiwatcher_core::prompts::OptimizationSummary,
        aiwatcher_core::prompts::OptimizationOutcome,
        aiwatcher_core::prompts::RejectionReason,
        aiwatcher_core::prompts::Score,
        aiwatcher_prompts::PromptPage,
        aiwatcher_prompts::PublishRequest,
        aiwatcher_prompts::Published,
        aiwatcher_prompts::OptimizationRequest,
        crate::prompts::PromptDetail,
        crate::prompts::LabelRequest,
        crate::routes::EventPage,
        crate::routes::IngestRequest,
        crate::routes::IngestResponse,
        crate::stream::LiveFrame,
        crate::error::ErrorBody,
    )),
    tags(
        (name = "metrics", description = "Aggregates over retained runs"),
        (name = "runs", description = "History and projections"),
        (name = "evaluation", description = "Evaluation reports: suites, scores, regressions"),
        (name = "prompts", description = "The prompt registry: versions, labels and optimisations"),
        (name = "workflow", description = "Declared graphs, their executions, and rerunning one"),
        (name = "live", description = "SSE and WebSocket streams"),
        (name = "ingest", description = "HTTP fallback for publishing events"),
        (name = "health", description = "Kubernetes probes"),
    ),
)]
pub struct ApiDoc;

impl ApiDoc {
    /// The document as pretty-printed JSON.
    pub fn to_json() -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&<Self as OpenApi>::openapi())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_document_covers_every_route_the_panel_uses() {
        let document = <ApiDoc as OpenApi>::openapi();
        for path in [
            "/api/v1/conversations",
            "/api/v1/dimensions/{kind}",
            "/api/v1/spans",
            "/api/v1/evaluations",
            "/api/v1/evaluations/{evaluation_id}",
            "/api/v1/evaluation-suites",
            "/api/v1/prompts",
            "/api/v1/prompts/{name}",
            "/api/v1/prompts/{name}/versions/{version_id}",
            "/api/v1/prompts/{name}/labels/{label}",
            "/api/v1/prompts/{name}/optimizations",
            "/api/v1/prompts/{name}/optimizations/{optimization_id}",
            "/api/v1/prompts/{name}/rebuild",
            "/api/v1/workflows",
            "/api/v1/workflows/{workflow_id}",
            "/api/v1/workflows/{workflow_id}/rerun",
            "/api/v1/workflow-executions",
            "/api/v1/workflow-executions/{workflow_run_id}",
            "/api/v1/workflow-executions/{workflow_run_id}/stream",
            "/api/v1/metrics",
            "/api/v1/runs",
            "/api/v1/runs/{run_id}",
            "/api/v1/runs/{run_id}/events",
            "/api/v1/runs/{run_id}/stream",
            "/api/v1/live",
            "/api/v1/events",
        ] {
            assert!(
                document.paths.paths.contains_key(path),
                "{path} is missing from the OpenAPI document, so the generated client will not have it"
            );
        }
    }

    #[test]
    fn it_serialises() {
        let json = ApiDoc::to_json().expect("serialises");
        assert!(json.contains("\"aiwatcher\""));
        assert!(json.contains("RunSummary"));
    }
}
