//! An image, the drawings on it, and what the registry does with both.
//!
//! The slice. Everything about *one picture* is here — its head, its
//! revisions, its review state, the bytes behind it and the bulk import that
//! creates many at once — and [`Registry`](crate::Registry) is the facade that
//! resolves a project and hands the work over.
//!
//! The split from [`project`](crate::project) is where the two nouns actually
//! part company. A project owns a vocabulary and a split *policy*; an image
//! owns geometry, provenance and a review state. They used to share a file,
//! and the file was the largest one in the crate.
//!
//! What this module deliberately does **not** do is look a project up. Every
//! operation takes an already-resolved
//! [`AnnotationProject`](crate::project::AnnotationProject), because that
//! keeps one question — "does this project exist" — answered in one place, and
//! because a slice that fetched its own context would fetch it once per row on
//! an import of six hundred.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::license::UsageRights;
use crate::project::Split;
use crate::shapes::{Annotation, Origin};

pub mod import;
mod store;

// The operations are crate-internal: [`Registry`](crate::Registry) is the
// public door, and it is the only thing that resolves a project. A caller
// reaching these directly would be a caller that skipped that.
pub(crate) use import::import;
pub(crate) use store::{
    blob, check, detail, heads, list, put_blob, register, review, revision, save_revision,
};

/// How a registered image points at bytes this registry holds.
///
/// A scheme of its own rather than a path, because a URI in `ImageRecord` may
/// equally be an external URL and a reader has to be able to tell. Not `blob:`:
/// the browser owns that one, and an `<img src="blob:…">` would resolve
/// against the document instead of reaching this server.
pub const BLOB_SCHEME: &str = "aiwatcher://blob/";

/// A stored image blob and what it is.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct StoredBlob {
    /// SHA-256 of the bytes, computed here. This is the `image_id`.
    pub image_id: String,
    pub content_type: String,
    pub bytes: usize,
    /// What to put in `ImageRecord::uri`. See [`BLOB_SCHEME`].
    pub uri: String,
    /// False when these exact bytes were already stored.
    pub created: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct BlobMeta {
    content_type: String,
    bytes: usize,
}

/// The result of saving a drawing.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct SavedRevision {
    pub revision: AnnotationRevision,
    pub head: ImageHead,
    /// False when this exact set of shapes was already stored.
    pub created: bool,
}

/// How an image list is narrowed.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct ImageFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<ReviewState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split: Option<Split>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    /// Matches the source, the group, the level and the image id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
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
    /// What kind of picture this is, in the caller's own words.
    ///
    /// Free text for the same reason `level` is: the vocabulary differs per
    /// corpus, and an enum shipped here would be one project's list imposed on
    /// every other. An export selects the views it wants by name, so a corpus
    /// that mixes photographs with diagrams, or plans with elevations, keeps
    /// the ones a model reads out of the ones it does not — by saying so
    /// rather than by somebody remembering.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub view: String,
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
    /// What kind of picture this is, in the caller's own words.
    ///
    /// Free text for the same reason `level` is: the vocabulary differs per
    /// corpus, and an enum shipped here would be one project's list imposed on
    /// every other. An export selects the views it wants by name, so a corpus
    /// that mixes photographs with diagrams, or plans with elevations, keeps
    /// the ones a model reads out of the ones it does not — by saying so
    /// rather than by somebody remembering.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub view: String,
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

#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct ImagePage {
    pub images: Vec<ImageHead>,
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
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
