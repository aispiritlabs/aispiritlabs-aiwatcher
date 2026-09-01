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
use serde_json::Value;
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::schema::LabelSchema;
use crate::shapes::{Annotation, Origin};
use crate::{Error, Result, validate_name};

/// What a drawing on a page actually is.
///
/// A section and an elevation are the same house drawn a different way, and a
/// model trained to read plans reads neither. Recording the view is what keeps
/// them out of an export instead of out of somebody's memory.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ViewType {
    #[default]
    FloorPlan,
    Section,
    Elevation,
    SitePlan,
    Other,
}

/// What may be done with an image, and therefore with a model trained on it.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UsageRights {
    /// Produced here, or supplied with an explicit grant covering training,
    /// derived artifacts and the resulting weights.
    Owned {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        grant: String,
    },
    /// Licensed under terms that permit commercial use.
    Licensed {
        license: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
    },
    /// Licensed for research only — CC BY-NC and everything shaped like it.
    ResearchOnly {
        license: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
    },
    /// Nobody has checked. Usable for an experiment, excluded from anything
    /// that claims a policy.
    Unknown,
}

impl UsageRights {
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Self::Owned { .. } => "owned".to_owned(),
            Self::Licensed { license, .. } => license.clone(),
            Self::ResearchOnly { license, .. } => format!("{license} (research only)"),
            Self::Unknown => "unknown".to_owned(),
        }
    }

    /// Whether an export declaring `policy` may include this image.
    #[must_use]
    pub const fn allows(&self, policy: RightsPolicy) -> bool {
        match (self, policy) {
            (_, RightsPolicy::Any) => true,
            (Self::Owned { .. } | Self::Licensed { .. }, _) => true,
            (Self::ResearchOnly { .. }, RightsPolicy::Research) => true,
            (Self::ResearchOnly { .. } | Self::Unknown, RightsPolicy::Commercial) => false,
            (Self::Unknown, RightsPolicy::Research) => false,
        }
    }
}

/// What an export claims about itself.
///
/// `Commercial` is the default, because the failure this guards against is
/// silent and the correction is one field. An export that wants CubiCasa5K in
/// it has to say `research` and will say so in its manifest forever.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RightsPolicy {
    #[default]
    Commercial,
    Research,
    /// Everything, including images nobody has checked. For an experiment whose
    /// weights are thrown away.
    Any,
}

impl RightsPolicy {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Commercial => "commercial",
            Self::Research => "research",
            Self::Any => "any",
        }
    }
}

/// One registered image.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ImageRecord {
    /// SHA-256 of the bytes, lowercase hex. Computed by the server on upload
    /// and never taken from the client.
    pub image_id: String,
    /// Where the bytes are. `blob:<image_id>` for something uploaded here.
    pub uri: String,
    pub width: u32,
    pub height: u32,
    /// The split key. Every rendering of one building shares it.
    pub group_id: String,
    /// Where it came from — a supplier, a public corpus, a scan batch.
    pub source: String,
    pub rights: UsageRights,
    #[serde(default)]
    pub view: ViewType,
    /// `ground_floor`, `attic`, `basement`. Free text, because the vocabulary
    /// differs per supplier and guessing it wrong is worse than keeping theirs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    /// Everything known about this plan that did not come from the pixels: the
    /// room names and areas from the catalogue page, the plan's own dimensions,
    /// the listing URL.
    ///
    /// This is the field that makes OCR a cross-check rather than a source.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schema(value_type = Object)]
    pub metadata: BTreeMap<String, Value>,
    #[serde(with = "time::serde::rfc3339")]
    pub registered_at: OffsetDateTime,
}

/// What is being asked for when an image is registered.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RegisterImageRequest {
    pub project: String,
    pub image_id: String,
    pub uri: String,
    pub width: u32,
    pub height: u32,
    pub group_id: String,
    #[serde(default)]
    pub source: String,
    pub rights: UsageRights,
    #[serde(default)]
    pub view: ViewType,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

/// Where a revision sits between drawn and trusted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    /// Somebody is still drawing, or a model proposed it and nobody has looked.
    #[default]
    Draft,
    InReview,
    /// The accepted revision is the truth for this image.
    Accepted,
    /// Looked at and refused. Kept, because "we decided this plan is unusable"
    /// is worth more than a gap.
    Rejected,
}

impl ReviewState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::InReview => "in_review",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
        }
    }
}

/// One immutable set of shapes for one image.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct AnnotationRevision {
    pub project: String,
    pub image_id: String,
    /// The schema these shapes were checked against. An export refuses a
    /// revision drawn under a different vocabulary rather than guessing.
    pub schema_version: String,
    /// SHA-256 of the shapes, the schema version and the image. Author, notes
    /// and time are deliberately outside it: two people drawing the same thing
    /// is one revision.
    pub revision: String,
    pub annotations: Vec<Annotation>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// What a caller sends to save a drawing.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveRevisionRequest {
    pub project: String,
    pub image_id: String,
    pub annotations: Vec<Annotation>,
    #[serde(default)]
    pub notes: String,
    /// Promote this revision in the same call. Requires that it validates
    /// clean, which is the only place review and validation meet.
    #[serde(default)]
    pub accept: bool,
}

/// A revision as it appears in a list, without its shapes.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct RevisionSummary {
    pub revision: String,
    pub schema_version: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
    pub shape_count: usize,
    /// Instances per class, which is what tells a reviewer at a glance that a
    /// plan has nine rooms and no doors.
    pub per_class: BTreeMap<String, usize>,
    /// Instances per origin. A revision that is entirely `model` has not been
    /// looked at, whatever its review state claims.
    pub per_origin: BTreeMap<String, usize>,
}

impl RevisionSummary {
    #[must_use]
    pub fn of(revision: &AnnotationRevision) -> Self {
        let mut per_class: BTreeMap<String, usize> = BTreeMap::new();
        let mut per_origin: BTreeMap<String, usize> = BTreeMap::new();
        for annotation in &revision.annotations {
            *per_class.entry(annotation.class.clone()).or_default() += 1;
            *per_origin
                .entry(annotation.origin.as_str().to_owned())
                .or_default() += 1;
        }
        Self {
            revision: revision.revision.clone(),
            schema_version: revision.schema_version.clone(),
            created_at: revision.created_at,
            author: revision.author.clone(),
            shape_count: revision.annotations.len(),
            per_class,
            per_origin,
        }
    }

    /// Whether a human contributed anything to this revision.
    #[must_use]
    pub fn touched_by_a_human(&self) -> bool {
        self.per_origin
            .get(Origin::Human.as_str())
            .is_some_and(|count| *count > 0)
    }
}

/// One image's mutable head: what it is, what has been drawn on it, and which
/// of those drawings is the truth.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ImageHead {
    pub project: String,
    pub image: ImageRecord,
    #[serde(default)]
    pub review: ReviewState,
    /// The revision an export reads. Set only by acceptance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewed_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub reviewed_at: Option<OffsetDateTime>,
    /// Newest first.
    #[serde(default)]
    pub revisions: Vec<RevisionSummary>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

impl ImageHead {
    #[must_use]
    pub fn latest(&self) -> Option<&RevisionSummary> {
        self.revisions.first()
    }

    #[must_use]
    pub fn accepted_summary(&self) -> Option<&RevisionSummary> {
        let accepted = self.accepted.as_ref()?;
        self.revisions
            .iter()
            .find(|summary| &summary.revision == accepted)
    }
}

/// A change to an image's review state.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewRequest {
    pub project: String,
    pub image_id: String,
    pub review: ReviewState,
    /// Which revision is being accepted. Required for
    /// [`ReviewState::Accepted`], refused otherwise — accepting "whatever is
    /// newest" is the same mistake as launching without pinning a version.
    #[serde(default)]
    pub revision: Option<String>,
    #[serde(default)]
    pub note: String,
}

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

#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct ImagePage {
    pub images: Vec<ImageHead>,
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
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

/// Where one image's revisions and its head are read together.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ImageDetail {
    #[serde(flatten)]
    pub head: ImageHead,
    /// The shapes of one revision — the accepted one, or the one asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<AnnotationRevision>,
    /// Which side of the split this image's family is on, under the project's
    /// current policy. Shown so a labeller knows whether they are drawing a
    /// test case, which changes how carefully they draw it.
    pub split: Split,
}
