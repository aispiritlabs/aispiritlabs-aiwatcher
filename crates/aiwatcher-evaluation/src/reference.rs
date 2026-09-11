use aiwatcher_core::ArtifactRef;
use serde::{Deserialize, Serialize};

use crate::{Result, require, text};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DatasetKind {
    Curation,
    Annotations,
    Conversations,
    External,
}

/// `version` is the owner's immutable revision, never a deployment label.
/// External revisions must be resolved by the producer; validation of this
/// contract is not evidence that the referenced resource exists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct VersionReference {
    pub name: String,
    pub version: String,
}

impl VersionReference {
    pub(crate) fn validate(&self, field: &str) -> Result<()> {
        text(&self.name, &format!("{field}.name"))?;
        text(&self.version, &format!("{field}.version"))?;
        require(
            !matches!(
                self.version.to_ascii_lowercase().as_str(),
                "latest" | "production" | "staging" | "main" | "head"
            ),
            field,
            "resolve mutable aliases before preparing a manifest",
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DatasetReference {
    pub kind: DatasetKind,
    pub name: String,
    pub version: String,
}

impl DatasetReference {
    pub(crate) fn validate(&self, field: &str) -> Result<()> {
        VersionReference {
            name: self.name.clone(),
            version: self.version.clone(),
        }
        .validate(field)
    }
}

pub(crate) fn artifact(value: &ArtifactRef, field: &str) -> Result<()> {
    text(&value.name, &format!("{field}.name"))?;
    text(&value.uri, &format!("{field}.uri"))?;
    require(
        value.has_digest(),
        field,
        "requires a lowercase SHA-256 digest",
    )?;
    require(
        value.size_bytes.is_some(),
        field,
        "requires size_bytes, including zero for an empty artifact",
    )
}
