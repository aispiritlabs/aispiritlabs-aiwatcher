use aiwatcher_core::ArtifactRef;
use serde::{Deserialize, Serialize};

use crate::{
    DatasetReference, EvaluationContext, Result, SCHEMA_VERSION, VersionReference,
    reference::artifact, require, text,
};

/// All references are resolved before this snapshot is prepared. Configuration
/// and code are digested artifacts, so changing them cannot retarget a variant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct VariantManifest {
    #[schema(minimum = 1, maximum = 1)]
    pub schema_version: u32,
    pub experiment_id: String,
    pub dataset: DatasetReference,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<VersionReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<VersionReference>,
    pub code: ArtifactRef,
    pub generation_config: ArtifactRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_schema: Option<ArtifactRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<ArtifactRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<VersionReference>,
}

impl VariantManifest {
    fn validate(&self) -> Result<()> {
        require(
            self.schema_version == SCHEMA_VERSION,
            "variant.schema_version",
            "unsupported version",
        )?;
        text(&self.experiment_id, "variant.experiment_id")?;
        self.dataset.validate("variant.dataset")?;
        require(
            self.model.is_some() || self.prompt.is_some() || self.workflow.is_some(),
            "variant",
            "requires a model, prompt or workflow reference",
        )?;
        for (field, value) in [
            ("model", &self.model),
            ("prompt", &self.prompt),
            ("workflow", &self.workflow),
        ] {
            if let Some(value) = value {
                value.validate(&format!("variant.{field}"))?;
            }
        }
        artifact(&self.code, "variant.code")?;
        artifact(&self.generation_config, "variant.generation_config")?;
        for (field, value) in [
            ("response_schema", &self.response_schema),
            ("tools", &self.tools),
        ] {
            if let Some(value) = value {
                artifact(value, &format!("variant.{field}"))?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EvaluationOrigin {
    /// The existing logical report ID; a technical retry must reuse it.
    pub evaluation_id: String,
    /// Independent measurement, not a worker attempt counter.
    pub repetition_id: String,
    /// The envelope calls the same identity `workflow_run_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EvaluationManifest {
    #[schema(minimum = 1, maximum = 1)]
    pub schema_version: u32,
    pub origin: EvaluationOrigin,
    pub variant: VariantManifest,
    pub context: EvaluationContext,
}

impl EvaluationManifest {
    pub(crate) fn validate(&self) -> Result<()> {
        require(
            self.schema_version == SCHEMA_VERSION,
            "schema_version",
            "unsupported version",
        )?;
        text(&self.origin.evaluation_id, "origin.evaluation_id")?;
        text(&self.origin.repetition_id, "origin.repetition_id")?;
        require(
            self.origin.step_id.is_none() || self.origin.execution_id.is_some(),
            "origin.step_id",
            "a step requires its execution_id",
        )?;
        for (field, value) in [
            ("execution_id", &self.origin.execution_id),
            ("step_id", &self.origin.step_id),
        ] {
            if let Some(value) = value {
                text(value, &format!("origin.{field}"))?;
            }
        }
        self.variant.validate()?;
        self.context.validate()?;
        require(
            self.variant.dataset == self.context.dataset,
            "context.dataset",
            "must match the variant's pinned dataset",
        )
    }
}

/// No deserializer or mutable accessor: callers cannot forge the IDs or change
/// the validated snapshot. Reading stored metadata must prepare it again.
#[derive(Clone, Debug, Serialize)]
pub struct PreparedEvaluation {
    manifest: EvaluationManifest,
    variant_id: String,
    context_id: String,
}

impl PreparedEvaluation {
    pub(crate) fn new(
        manifest: EvaluationManifest,
        variant_id: String,
        context_id: String,
    ) -> Self {
        Self {
            manifest,
            variant_id,
            context_id,
        }
    }

    #[must_use]
    pub fn manifest(&self) -> &EvaluationManifest {
        &self.manifest
    }

    #[must_use]
    pub fn variant_id(&self) -> &str {
        &self.variant_id
    }

    /// Identity of context metadata, not a quality verdict or verified artifact.
    #[must_use]
    pub fn context_id(&self) -> &str {
        &self.context_id
    }
}
