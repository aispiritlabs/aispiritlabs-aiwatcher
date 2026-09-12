//! A cohort taken from a dataset version, rather than written out by hand.
//!
//! A cohort pins three files by digest — the cases, the shape of what each
//! asked and the shape of what each expected — and until now somebody wrote
//! them, or copied them from a result already published. But for a dataset
//! aiwatcher owns, the source adapter already derives exactly those cases from
//! the owner every time it admits a pair; the files were a second copy of a
//! question it answers itself.
//!
//! So the owner's adapter derives them: the first `limit` cases of a version's
//! split, in the owner's own order, as canonical bytes. Nothing is staged. The
//! same adapter derives them again when the pair is admitted and whenever
//! evidence is read, and bytes that no longer hash to the pins are the same
//! refusal a changed bundle is.
//!
//! A limit selects the owner's first cases, not a sample. Its cases are a
//! different cohort from the whole split's — a different manifest, a different
//! context, and a result that compares with nothing measured on the whole —
//! which is what makes a ten-case smoke run honest about being one.

use aiwatcher_core::{ArtifactKind, ArtifactRef};
use serde::{Deserialize, Serialize};

use crate::{Cohort, DatasetKind, DatasetReference, Result, require, text};

/// What each derived file is called, in a cohort and in a bundle alike.
pub const COHORT_CASES: &str = "cases.json";
pub const COHORT_INPUT_SCHEMA: &str = "input-schema.json";
pub const COHORT_EXPECTATIONS_SCHEMA: &str = "expectations-schema.json";

/// Which cases of which dataset version a cohort should select.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CohortRequest {
    /// A curation dataset version, an annotation export or a conversation
    /// corpus. What the variant measured must name the same one.
    pub dataset: DatasetReference,
    /// For an annotation export, the split it deals (`train`, `validation`,
    /// `test`); a conversation corpus is measured on `test`. A curation
    /// version has no splits of its own, so there it is the name the cohort
    /// is given and selects nothing.
    pub split: String,
    /// The owner's first this-many cases, when fewer than all of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(minimum = 1)]
    pub limit: Option<u64>,
}

impl CohortRequest {
    pub fn validate(&self) -> Result<()> {
        self.dataset.validate("cohort.dataset")?;
        text(&self.split, "cohort.split")?;
        require(
            matches!(
                self.dataset.kind,
                DatasetKind::Curation | DatasetKind::Annotations | DatasetKind::Conversations
            ),
            "cohort.dataset.kind",
            "only a dataset this deployment owns has cases it can derive; an external cohort's \
             cases are its producer's",
        )?;
        require(
            self.limit.is_none_or(|limit| limit > 0),
            "cohort.limit",
            "selects at least one case",
        )
    }
}

/// The three files an owner's adapter derived, and how many cases they hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CohortFiles {
    pub cases: Vec<u8>,
    pub input_schema: Vec<u8>,
    pub expectations_schema: Vec<u8>,
    /// How many cases the files select.
    pub count: u64,
    /// How many the split holds, of which `count` are the first.
    pub available: u64,
}

impl CohortFiles {
    /// The pins a declaration names these files by.
    ///
    /// The address is a name, never a location: every reader derives the
    /// bytes again from the owner, and nothing ever fetches this URI.
    #[must_use]
    pub fn cohort(&self, split: &str) -> Cohort {
        let cases = crate::store::hash(&self.cases);
        let pin = |name: &str, bytes: &[u8]| {
            let mut artifact = ArtifactRef::new(
                name,
                format!("aiwatcher://evaluation-cohorts/{cases}/{name}"),
                crate::store::hash(bytes),
            )
            .of_kind(ArtifactKind::Blob);
            artifact.size_bytes = Some(bytes.len() as u64);
            artifact.content_type = "application/json".into();
            artifact
        };
        Cohort {
            case_manifest: pin(COHORT_CASES, &self.cases),
            case_count: self.count,
            split: split.to_owned(),
            input_schema: pin(COHORT_INPUT_SCHEMA, &self.input_schema),
            expectations_schema: pin(COHORT_EXPECTATIONS_SCHEMA, &self.expectations_schema),
        }
    }

    /// The bytes behind one pin, when it is one of these files.
    #[must_use]
    pub fn pinned(&self, artifact: &ArtifactRef) -> Option<&[u8]> {
        [&self.cases, &self.input_schema, &self.expectations_schema]
            .into_iter()
            .find(|bytes| {
                crate::store::hash(bytes) == artifact.digest
                    && artifact
                        .size_bytes
                        .is_none_or(|size| size == bytes.len() as u64)
            })
            .map(Vec::as_slice)
    }
}

/// A derived cohort, as it was first derived.
///
/// Kept under the digest of its cases, so a declaration naming those cases can
/// say where they came from — which is what lets an operator admitting it be
/// told that its three files are not theirs to bring. It records a derivation
/// and never authorises one: the adapter derives the bytes again every time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DerivedCohort {
    pub request: CohortRequest,
    pub cohort: Cohort,
    /// How many cases the split holds, of which the cohort selects the first
    /// `cohort.case_count`.
    pub available: u64,
    pub derived_by: String,
    pub derived_at: i64,
}

impl DerivedCohort {
    /// Whether this derivation is the cohort a context pins.
    #[must_use]
    pub fn describes(&self, dataset: &DatasetReference, cohort: &Cohort) -> bool {
        &self.request.dataset == dataset && &self.cohort == cohort
    }
}
