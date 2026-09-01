//! The model registry: what a training run produced, and what may be promoted.
//!
//! This is the half that makes the training module worth having *here* rather
//! than in Weights & Biases. A model version names the export it was trained on
//! and the run that produced it; an agent span names a model. That is the join
//! — from a floor plan coming back with bad geometry, to the checkpoint that
//! produced it, to the labelled images behind it — and it only exists because
//! both ends are in one system.
//!
//! Two rules carry it, and both are lifted from the prompt registry's verdict
//! rule for the same reason: a producer that reports its own score picked the
//! thing it is reporting.
//!
//! * a version whose dataset is a mutable name is recorded and is **not
//!   reproducible**;
//! * a label is refused unless the version has a **held-out** measurement.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::{Error, Result, validate_slug};

/// What a version measured, split by which data measured it.
///
/// Two maps rather than one, because the distinction is the whole point.
/// `validation` is what the training loop watched and selected against;
/// `test` is what nothing was allowed to look at. A registry that merged them
/// would let a model be promoted on the number its own early stopping
/// maximised.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct ModelMetrics {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub validation: BTreeMap<String, f64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub test: BTreeMap<String, f64>,
}

impl ModelMetrics {
    #[must_use]
    pub fn has_held_out(&self) -> bool {
        !self.test.is_empty()
    }

    /// How far the held-out number fell short of the one selection watched.
    ///
    /// The number worth following across a series of versions, exactly as
    /// `overfit_gap` is for a prompt optimisation. `None` when the same metric
    /// was not measured on both.
    #[must_use]
    pub fn overfit_gap(&self, metric: &str) -> Option<f64> {
        Some(self.validation.get(metric)? - self.test.get(metric)?)
    }
}

/// One registered model version.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ModelVersion {
    pub name: String,
    /// SHA-256 of what makes this version *this* version: the run, the
    /// checkpoint, the dataset and the measurements. Registering the same
    /// thing twice is one version.
    pub version: String,
    pub run_id: String,
    /// `project@export-sha256` from the annotation registry.
    pub dataset: String,
    /// Where the weights are. A pointer, like every other artifact here.
    pub checkpoint_uri: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub framework: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub code: String,
    #[serde(default)]
    pub metrics: ModelMetrics,
    /// False when the dataset is a mutable name rather than an immutable
    /// export reference. Recorded rather than refused, and it is what blocks
    /// a promotion.
    pub reproducible: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

impl ModelVersion {
    /// Whether a label may point at this version, and why not when it may not.
    ///
    /// The two reasons are deliberately different sentences: "nothing measured
    /// it on data it had not seen" invites a held-out evaluation, and "it names
    /// a dataset nobody can reconstruct" invites an export. Collapsing them
    /// into "not promotable" would invite neither.
    pub fn check_promotable(&self) -> Result<()> {
        if !self.reproducible {
            return Err(Error::Refused(format!(
                "{}@{} was trained on {}, which is a name rather than an immutable export \
                 reference; nothing can reconstruct what it learned from",
                self.name,
                &self.version[..12.min(self.version.len())],
                self.dataset
            )));
        }
        if !self.metrics.has_held_out() {
            return Err(Error::Refused(format!(
                "{}@{} has no held-out measurement; the validation score is the number training \
                 selected against, so promoting on it promotes the selection",
                self.name,
                &self.version[..12.min(self.version.len())]
            )));
        }
        Ok(())
    }
}

/// A version as it appears in a list.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ModelVersionSummary {
    pub version: String,
    pub run_id: String,
    pub dataset: String,
    pub reproducible: bool,
    #[serde(default)]
    pub metrics: ModelMetrics,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

impl ModelVersionSummary {
    #[must_use]
    pub fn of(version: &ModelVersion) -> Self {
        Self {
            version: version.version.clone(),
            run_id: version.run_id.clone(),
            dataset: version.dataset.clone(),
            reproducible: version.reproducible,
            metrics: version.metrics.clone(),
            created_at: version.created_at,
        }
    }
}

/// One model's mutable head: its versions, newest first, and its labels.
///
/// Labels live here and nowhere else, exactly as a prompt's do: a version is
/// immutable and `production` is not.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ModelHead {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default)]
    pub versions: Vec<ModelVersionSummary>,
    /// Label to version id. `production` is the one a deployment reads.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

impl ModelHead {
    #[must_use]
    pub fn latest(&self) -> Option<&ModelVersionSummary> {
        self.versions.first()
    }

    #[must_use]
    pub fn labelled(&self, label: &str) -> Option<&ModelVersionSummary> {
        let version = self.labels.get(label)?;
        self.versions.iter().find(|entry| &entry.version == version)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct ModelPage {
    pub models: Vec<ModelHead>,
}

/// One model, its head and one version's full record.
///
/// `head` is a field rather than a flattened one, unlike the annotation
/// registry's equivalent. Flattening reads better in the JSON and produces a
/// TypeScript intersection type that the panel then cannot narrow, so the
/// nesting is the price of the client being generated rather than written.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ModelDetail {
    pub head: ModelHead,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<ModelVersion>,
}

/// The label a deployment reads to answer "which version is live".
pub const PRODUCTION: &str = "production";

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RegisterModelRequest {
    pub name: String,
    /// The run that produced it. Read for its dataset, framework and code, so
    /// a version cannot claim provenance the run does not have.
    pub run_id: String,
    pub checkpoint_uri: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub metrics: ModelMetrics,
    #[serde(default)]
    pub notes: String,
}

impl RegisterModelRequest {
    pub fn validate(&self) -> Result<()> {
        validate_slug(&self.name, "a model name")?;
        validate_slug(&self.run_id, "a run id")?;
        if self.checkpoint_uri.trim().is_empty() {
            return Err(Error::Invalid(
                "a model version needs the checkpoint it came from".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelLabelRequest {
    pub label: String,
    pub version: String,
}

/// What registering returned.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct RegisteredModel {
    pub version: ModelVersion,
    pub head: ModelHead,
    /// False when this exact version already existed.
    pub created: bool,
    /// Why a label cannot point here yet, if it cannot. Returned on the
    /// *registration* so the answer arrives with the thing it is about, rather
    /// than when somebody later tries to promote it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promotion_blocked: Option<String>,
}
