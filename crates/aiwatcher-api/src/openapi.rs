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
    components(schemas(
        aiwatcher_auth::Identity,
        aiwatcher_auth::Role,
        aiwatcher_auth::Credential,
        aiwatcher_auth::AuthMode,
        aiwatcher_auth::PublicAuthConfig,
        crate::auth::LoggedOut,
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
        aiwatcher_core::engine::EngineDescription,
        aiwatcher_core::engine::EngineCatalog,
        aiwatcher_core::engine::EngineWorkflow,
        aiwatcher_core::engine::EngineParameter,
        aiwatcher_core::engine::ParameterKind,
        aiwatcher_core::engine::EntityKind,
        aiwatcher_core::engine::PipelineStage,
        aiwatcher_core::engine::EngineExecution,
        aiwatcher_core::engine::EnginePhase,
        aiwatcher_core::engine::LaunchAccepted,
        crate::engine::LaunchBody,
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
        aiwatcher_datasets::CurationRecipe,
        aiwatcher_datasets::SaveRecipeRequest,
        aiwatcher_datasets::SavedRecipe,
        aiwatcher_datasets::RecipePage,
        aiwatcher_datasets::PublishDatasetRequest,
        aiwatcher_datasets::DatasetVersionSummary,
        aiwatcher_datasets::DatasetVersion,
        aiwatcher_datasets::DatasetSummary,
        aiwatcher_datasets::DatasetPage,
        aiwatcher_datasets::DatasetRow,
        aiwatcher_datasets::DatasetRowsPage,
        aiwatcher_datasets::PublishedDataset,
        aiwatcher_annotations::GeometryKind,
        aiwatcher_annotations::AttributeKind,
        aiwatcher_annotations::AttributeDef,
        aiwatcher_annotations::LinkDef,
        aiwatcher_annotations::LabelClass,
        aiwatcher_annotations::LabelSchema,
        aiwatcher_annotations::Geometry,
        aiwatcher_annotations::Keypoint,
        aiwatcher_annotations::Origin,
        aiwatcher_annotations::Annotation,
        aiwatcher_annotations::UsageRights,
        aiwatcher_annotations::RightsPolicy,
        aiwatcher_annotations::ImageRecord,
        aiwatcher_annotations::RegisterImageRequest,
        aiwatcher_annotations::ImportRow,
        aiwatcher_annotations::ImportSource,
        aiwatcher_annotations::ImportRequest,
        aiwatcher_annotations::RowOutcome,
        aiwatcher_annotations::ImportReport,
        aiwatcher_annotations::integrations::hubs::HubKind,
        aiwatcher_annotations::integrations::hubs::HubFile,
        aiwatcher_annotations::integrations::hubs::HubDataset,
        aiwatcher_annotations::integrations::hubs::HubStatus,
        aiwatcher_annotations::integrations::hubs::HubSearchPage,
        aiwatcher_annotations::integrations::hubs::HubImage,
        aiwatcher_annotations::integrations::hubs::HubImagePage,
        crate::integrations::hubs::HubsPage,
        aiwatcher_annotations::ReviewState,
        aiwatcher_annotations::AnnotationRevision,
        aiwatcher_annotations::SaveRevisionRequest,
        aiwatcher_annotations::RevisionSummary,
        aiwatcher_annotations::ImageHead,
        aiwatcher_annotations::ReviewRequest,
        aiwatcher_annotations::Split,
        aiwatcher_annotations::SplitRatios,
        aiwatcher_annotations::AnnotationProject,
        aiwatcher_annotations::ProjectSummary,
        aiwatcher_annotations::ProjectPage,
        aiwatcher_annotations::ImagePage,
        aiwatcher_annotations::SaveProjectRequest,
        aiwatcher_annotations::ImageDetail,
        aiwatcher_annotations::SavedRevision,
        aiwatcher_annotations::StoredBlob,
        aiwatcher_annotations::ExclusionReason,
        aiwatcher_annotations::ExportExclusion,
        aiwatcher_annotations::ExportSample,
        aiwatcher_annotations::ExportCounts,
        aiwatcher_annotations::ExportRequest,
        aiwatcher_annotations::ExportManifest,
        aiwatcher_annotations::ExportSummary,
        aiwatcher_annotations::ExportPage,
        aiwatcher_annotations::BuiltExport,
        aiwatcher_annotations::SourceUsage,
        aiwatcher_annotations::SourceAccess,
        aiwatcher_annotations::DatasetSource,
        aiwatcher_annotations::SourceDirectory,
        aiwatcher_annotations::SourceCatalog,
        aiwatcher_annotations::SourcePage,
        aiwatcher_training::TrainingStatus,
        aiwatcher_training::EpochRecord,
        aiwatcher_training::SampleRecord,
        aiwatcher_training::CheckpointRecord,
        aiwatcher_training::ProfileRecord,
        aiwatcher_training::BestMetric,
        aiwatcher_training::TrainingRun,
        aiwatcher_training::TrainingRunSummary,
        aiwatcher_training::TrainingRunPage,
        aiwatcher_training::StartRunRequest,
        aiwatcher_training::ProgressRequest,
        aiwatcher_training::EpochInput,
        aiwatcher_training::SampleInput,
        aiwatcher_training::CheckpointInput,
        aiwatcher_training::ProfileInput,
        aiwatcher_training::FinishRunRequest,
        aiwatcher_training::ModelMetrics,
        aiwatcher_training::ModelVersion,
        aiwatcher_training::ModelVersionSummary,
        aiwatcher_training::ModelHead,
        aiwatcher_training::ModelDetail,
        aiwatcher_training::ModelPage,
        aiwatcher_training::RegisterModelRequest,
        aiwatcher_training::RegisteredModel,
        aiwatcher_training::ModelLabelRequest,
        crate::runs::EventPage,
        crate::ingest::IngestRequest,
        crate::ingest::IngestResponse,
        crate::stream::LiveFrame,
        crate::error::ErrorBody,
    )),
    tags(
        (name = "metrics", description = "Aggregates over retained runs"),
        (name = "runs", description = "History and projections"),
        (name = "evaluation", description = "Evaluation reports: suites, scores, regressions"),
        (name = "prompts", description = "The prompt registry: versions, labels and optimisations"),
        (name = "datasets", description = "Versioned datasets produced by Flow PHP curations"),
        (name = "data-curation", description = "Saved Flow PHP transformation recipes"),
        (name = "annotations", description = "Image annotation projects, drawings, reviews and training exports"),
        (name = "training", description = "Training runs, their curves, and the model versions they produce"),
        (name = "workflow", description = "Declared graphs, their executions, and rerunning one"),
        (name = "engine", description = "The orchestrator's launchable inventory, and starting one"),
        (name = "live", description = "SSE and WebSocket streams"),
        (name = "ingest", description = "HTTP fallback for publishing events"),
        (name = "auth", description = "Single sign-on: the login flow and the current caller"),
        (name = "health", description = "Kubernetes probes"),
    ),
)]
pub struct ApiDoc;

impl ApiDoc {
    /// The whole contract: this document's metadata, plus every module's own.
    ///
    /// The list is the API's areas, and adding one means adding a line here
    /// and a `router()` line in [`crate::routes`] — the two places a new area
    /// has to appear, and both of them fail loudly if only one is done.
    #[must_use]
    pub fn document() -> utoipa::openapi::OpenApi {
        let mut document = <Self as OpenApi>::openapi();
        for module in [
            crate::runs::openapi(),
            crate::metrics::openapi(),
            crate::evaluations::openapi(),
            crate::live::openapi(),
            crate::ingest::openapi(),
            crate::health::openapi(),
            crate::prompts::openapi(),
            crate::datasets::openapi(),
            crate::annotations::openapi(),
            crate::training::openapi(),
            crate::workflows::openapi(),
            crate::engine::openapi(),
            crate::integrations::hubs::openapi(),
            crate::auth::openapi(),
        ] {
            document.merge(module);
        }
        document
    }
}

impl ApiDoc {
    /// The document as pretty-printed JSON.
    pub fn to_json() -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&Self::document())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_document_covers_every_route_the_panel_uses() {
        let document = ApiDoc::document();
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
            "/api/v1/datasets",
            "/api/v1/dataset-rows",
            "/api/v1/curations",
            "/api/v1/workflows",
            "/api/v1/workflows/{workflow_id}",
            "/api/v1/workflows/{workflow_id}/rerun",
            "/api/v1/workflow-executions",
            "/api/v1/workflow-executions/{workflow_run_id}",
            "/api/v1/workflow-executions/{workflow_run_id}/stream",
            "/api/v1/engine",
            "/api/v1/engine/workflows",
            "/api/v1/engine/workflows/{workflow_id}",
            "/api/v1/engine/launches",
            "/api/v1/engine/launches/{reference}",
            "/api/v1/metrics",
            "/api/v1/runs",
            "/api/v1/runs/{run_id}",
            "/api/v1/runs/{run_id}/events",
            "/api/v1/runs/{run_id}/stream",
            "/api/v1/events/stream",
            "/api/v1/live",
            "/api/v1/events",
        ] {
            assert!(
                document.paths.paths.contains_key(path),
                "{path} is missing from the OpenAPI document, so the generated client will not have it"
            );
        }
    }

    /// Every operation a module's facade declares, as `METHOD /path`.
    ///
    /// Read off the serialised JSON rather than utoipa's structs, because
    /// that is the artifact the panel's client is generated from — and a
    /// check that agrees with the generator is worth more than one that
    /// agrees with the library.
    fn operations(document: &utoipa::openapi::OpenApi) -> std::collections::BTreeSet<String> {
        let json: serde_json::Value =
            serde_json::to_value(document).expect("the document serialises");
        let mut found = std::collections::BTreeSet::new();
        let Some(paths) = json.get("paths").and_then(serde_json::Value::as_object) else {
            return found;
        };
        for (path, item) in paths {
            let Some(item) = item.as_object() else {
                continue;
            };
            for method in item.keys() {
                found.insert(format!("{} {path}", method.to_uppercase()));
            }
        }
        found
    }

    #[test]
    fn every_module_facade_reaches_the_document() {
        // The failure this catches is the one the facade layout introduces: a
        // module can have a perfectly good `router()` and `openapi()` and
        // still be missing from `document()`, in which case its routes serve
        // traffic the generated client has no method for. Nothing else in the
        // build notices, because both halves compile.
        let modules = [
            ("runs", crate::runs::openapi()),
            ("metrics", crate::metrics::openapi()),
            ("evaluations", crate::evaluations::openapi()),
            ("live", crate::live::openapi()),
            ("ingest", crate::ingest::openapi()),
            ("health", crate::health::openapi()),
            ("prompts", crate::prompts::openapi()),
            ("datasets", crate::datasets::openapi()),
            ("annotations", crate::annotations::openapi()),
            ("training", crate::training::openapi()),
            ("workflows", crate::workflows::openapi()),
            ("engine", crate::engine::openapi()),
            ("hubs", crate::integrations::hubs::openapi()),
            ("auth", crate::auth::openapi()),
        ];
        let merged = operations(&ApiDoc::document());

        let mut declared = std::collections::BTreeSet::new();
        for (name, module) in &modules {
            let module = operations(module);
            assert!(!module.is_empty(), "the {name} facade declares nothing");
            for operation in module {
                assert!(
                    merged.contains(&operation),
                    "{name} declares {operation}, which never reached the document"
                );
                declared.insert(operation);
            }
        }

        // And nothing reached it from anywhere else. An operation in the
        // document that no facade declares is an operation whose module
        // nobody can find.
        let orphans: Vec<_> = merged.difference(&declared).collect();
        assert!(
            orphans.is_empty(),
            "the document holds operations no facade declares: {orphans:?}"
        );
    }

    #[test]
    fn it_serialises() {
        let json = ApiDoc::to_json().expect("serialises");
        assert!(json.contains("\"aiwatcher\""));
        assert!(json.contains("RunSummary"));
    }
}
