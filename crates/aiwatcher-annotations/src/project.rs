//! Projects, the images registered into them, and the revisions drawn on those
//! images.
//!
//! Three rules carry this module, and each of them is a way of being wrong that
//! is expensive to discover late:
//!
//! 1. An image declares the **family** it belongs to, not just itself. One
//!    house published as a plan, its mirror and a garage variant is four images
//!    and one building; splitting them apart measures memorisation.
//! 2. An image declares its **usage rights**, and the field is not optional.
//!    The best public floor-plan corpora are non-commercial, and a licence
//!    breach does not show up in a metric.
//! 3. A revision is **immutable and content-addressed**; the review state that
//!    promotes one to the truth lives in the head, exactly as a prompt's labels
//!    do.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::schema::LabelSchema;
use crate::{Error, Result, validate_name};

/// Which side of the split a family is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Split {
    Train,
    Validation,
    Test,
}

impl Split {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Train => "train",
            Self::Validation => "validation",
            Self::Test => "test",
        }
    }
}

/// How families are dealt out. Percentages, summing to 100.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, ToSchema)]
pub struct SplitRatios {
    pub train: u32,
    pub validation: u32,
    pub test: u32,
}

impl Default for SplitRatios {
    fn default() -> Self {
        Self {
            train: 70,
            validation: 15,
            test: 15,
        }
    }
}

impl SplitRatios {
    pub fn validate(self) -> Result<()> {
        if self.train + self.validation + self.test == 100 {
            return Ok(());
        }
        Err(Error::Invalid(format!(
            "split ratios must add up to 100; {}+{}+{} is {}",
            self.train,
            self.validation,
            self.test,
            self.train + self.validation + self.test
        )))
    }
}

/// One annotation project: its vocabulary, its split policy, its overrides.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct AnnotationProject {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub schema: LabelSchema,
    #[serde(default)]
    pub splits: SplitRatios,
    /// Mixed into the split hash. Changing it re-deals every family, which is
    /// occasionally what you want and never what you want by accident — so it
    /// is stored, and an export records the one it used.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub split_salt: String,
    /// `group_id` to a fixed side, for the houses that have to be in the test
    /// set. Keyed by family for the same reason the hash is.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub split_overrides: BTreeMap<String, Split>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// A project as it appears in a list, with the counts a reader wants first.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ProjectSummary {
    #[serde(flatten)]
    pub project: AnnotationProject,
    pub images: usize,
    pub accepted: usize,
    pub groups: usize,
    pub instances: usize,
    /// Instances per class over accepted revisions only. The number that says
    /// whether there are enough doors yet.
    pub per_class: BTreeMap<String, usize>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct ProjectPage {
    pub projects: Vec<AnnotationProject>,
}

/// What a caller sends to create or re-describe a project.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveProjectRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub classes: Vec<crate::schema::LabelClass>,
    #[serde(default)]
    pub splits: SplitRatios,
    #[serde(default)]
    pub split_salt: String,
    #[serde(default)]
    pub split_overrides: BTreeMap<String, Split>,
}

impl SaveProjectRequest {
    pub fn validate(&self) -> Result<()> {
        validate_name(&self.name, "a project")?;
        self.splits.validate()?;
        for group in self.split_overrides.keys() {
            validate_name(group, "a group")?;
        }
        Ok(())
    }
}
