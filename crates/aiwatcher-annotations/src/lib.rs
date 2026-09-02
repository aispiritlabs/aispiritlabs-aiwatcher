//! Vector image annotations, the label schemas they are drawn against, and the
//! immutable exports a training run names.
//!
//! This is the third authored artifact in this workspace, after prompts
//! (ADR_0011) and datasets (ADR_0014), and it is authored for the same reason:
//! everything folded from the event log is bounded by retention, and a training
//! label has to outlive every run that used it.
//!
//! What is specific to this one is that **the vector shape is the source and
//! every raster is derived** — see ADR_0017. A segmentation mask cannot say
//! which wall an opening sits on, which way a door swings, or which two rooms
//! it connects, and those are exactly the fields the product's output JSON has
//! to carry. Draw pixels and they are lost at the moment of drawing.
//!
//! ```text
//! project ─ label schema (versioned by content)
//!    │
//!    ├─ image ── head: rights, family, review state, accepted revision
//!    │            └─ revision (immutable, content-addressed) ── shapes
//!    │
//!    └─ export (immutable, content-addressed)
//!          ├─ samples: image → split → accepted revision
//!          ├─ exclusions: every image left out, and why
//!          └─ COCO, generated on request
//! ```
//!
//! # Layout
//!
//! Sliced by noun rather than by layer, so a change to "what an image is"
//! touches one directory:
//!
//! ```text
//! registry     the facade. Resolves a project, then delegates. The only
//!              public door: every operation below is crate-internal.
//! project      a project: its vocabulary, its split policy, its overrides
//! images/      SLICE — one picture: its head, revisions, review, bytes
//!   store        what the registry does to one
//!   import       many at once, from rows a Flow pipeline produced
//! export       freezing a project into an immutable manifest, and COCO
//! license      what may be done with the data. One question, one module.
//! schema       the label vocabulary a drawing is checked against
//! shapes       the geometry itself, and what makes a drawing finished
//! sources      the dated table of public corpora somebody read the licence of
//! integrations/  what this crate reaches *out* to
//!   hubs         Kaggle and Hugging Face — asked what exists, never what is
//!                permitted
//! store        (private) the key layout every slice reads and writes through
//! ```

use aiwatcher_core::ports::{PortError, PortResult};
use aiwatcher_core::prompts::ObjectStore;
use sha2::{Digest, Sha256};

pub mod export;
pub mod images;
pub mod integrations;
pub mod license;
pub mod project;
pub mod registry;
pub mod schema;
pub mod shapes;
pub mod sources;
mod store;

pub use export::{
    BuiltExport, ExclusionReason, ExportCounts, ExportExclusion, ExportManifest, ExportPage,
    ExportRequest, ExportSample, ExportSummary, split_for, to_coco,
};
pub use images::import::{
    ImportReport, ImportRequest, ImportRow, ImportSource, MAX_IMPORT_ROWS, RowOutcome,
};
pub use images::{
    AnnotationRevision, BLOB_SCHEME, ImageDetail, ImageFilter, ImageHead, ImagePage, ImageRecord,
    RegisterImageRequest, ReviewRequest, ReviewState, RevisionSummary, SaveRevisionRequest,
    SavedRevision, StoredBlob,
};
pub use integrations::hubs::{
    HubCellQuery, HubColumn, HubConfig, HubDataset, HubFile, HubKind, HubQuery, HubRow,
    HubRowsPage, HubRowsQuery, HubSearchPage, HubStatus, Hubs,
};
pub use license::{RightsPolicy, SourceUsage, UsageRights, check_rights};
pub use project::{
    AnnotationProject, ProjectPage, ProjectSummary, SaveProjectRequest, Split, SplitRatios,
};
pub use registry::Registry;
pub use schema::{AttributeDef, AttributeKind, GeometryKind, LabelClass, LabelSchema, LinkDef};
pub use shapes::{Annotation, Geometry, Keypoint, Origin, Point};
pub use sources::{DatasetSource, SourceAccess, SourceCatalog, SourceDirectory, SourcePage};

/// Names, ids and slugs.
const MAX_NAME_BYTES: usize = 160;
/// Control revision size
const MAX_REVISION_BYTES: usize = 4 * 1024 * 1024;
/// Size for uploaded files
const MAX_BLOB_BYTES: usize = 16 * 1024 * 1024;

const MAX_ANNOTATIONS: usize = 5_000;
/// The most images one list request returns, matching the dataset viewer.
pub const MAX_IMAGE_PAGE: usize = 200;

/// A rejected or unavailable registry operation.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),

    /// A drawing that did not validate, with every problem found rather than
    /// the first. A labeller fixing one error per round trip stops using the
    /// tool.
    #[error("the annotation was refused: {}", .0.join("; "))]
    Rejected(Vec<String>),

    #[error("{0} was not found")]
    NotFound(String),

    #[error("{what} is {size}; the limit is {limit}")]
    TooLarge {
        what: &'static str,
        size: usize,
        limit: usize,
    },

    #[error("the annotation registry could not use its object store: {0}")]
    Store(#[from] PortError),

    #[error("stored object {key} is not an annotation registry document: {message}")]
    Corrupt { key: String, message: String },
}

pub type Result<T> = std::result::Result<T, Error>;

/// A slash-separated name: a project, a group, a level.
pub fn validate_name(name: &str, what: &str) -> Result<()> {
    if name.is_empty() || name.len() > MAX_NAME_BYTES {
        return Err(Error::Invalid(format!(
            "{what} name must be between 1 and {MAX_NAME_BYTES} bytes"
        )));
    }
    for segment in name.split('/') {
        let mut characters = segment.chars();
        let starts_well = characters
            .next()
            .is_some_and(|value| value.is_ascii_alphanumeric());
        let continues_well = characters
            .all(|value| value.is_ascii_alphanumeric() || matches!(value, '-' | '_' | '.'));
        if !starts_well || !continues_well || matches!(segment, "." | "..") {
            return Err(Error::Invalid(format!(
                "{what} name segments must start with a letter or number and hold only letters, numbers, '.', '_' or '-'"
            )));
        }
    }
    Ok(())
}

/// A single machine name with no slashes: a class, an attribute, a keypoint, an
/// annotation id.
pub fn validate_slug(value: &str, what: &str) -> Result<()> {
    if value.is_empty() || value.len() > MAX_NAME_BYTES {
        return Err(Error::Invalid(format!(
            "{what} must be between 1 and {MAX_NAME_BYTES} bytes"
        )));
    }
    let mut characters = value.chars();
    let starts_well = characters
        .next()
        .is_some_and(|value| value.is_ascii_alphanumeric());
    let continues_well = characters
        .all(|value| value.is_ascii_alphanumeric() || matches!(value, '-' | '_' | '.' | ':'));
    if !starts_well || !continues_well {
        return Err(Error::Invalid(format!(
            "{what} must start with a letter or number and hold only letters, numbers, '.', ':', '_' or '-'"
        )));
    }
    Ok(())
}

/// A lowercase SHA-256 in hex: an image id, a revision, an export.
///
/// Checked before it is interpolated into an object key, for the same reason
/// every part of an `EngineRef` is checked before it reaches an orchestrator's
/// URL — a `..` in an identifier is a path traversal into somebody else's data.
pub fn validate_digest(value: &str, what: &str) -> Result<()> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        // Uppercase hex would key the same content twice.
        if value.bytes().any(|byte| byte.is_ascii_uppercase()) {
            return Err(Error::Invalid(format!("{what} must be lowercase hex")));
        }
        return Ok(());
    }
    Err(Error::Invalid(format!(
        "{what} must be a 64-character lowercase SHA-256"
    )))
}

fn digest(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

/// Keep the object-store failure vocabulary consistent with the other
/// registries.
impl From<Error> for PortError {
    fn from(error: Error) -> Self {
        match error {
            Error::Store(error) => error,
            other => Self::Rejected {
                target: "annotation-registry",
                message: other.to_string(),
            },
        }
    }
}

/// A small probe useful to wiring and health checks.
pub async fn probe(store: &dyn ObjectStore, prefix: &str) -> PortResult<()> {
    store.list(prefix).await.map(|_| ())
}
