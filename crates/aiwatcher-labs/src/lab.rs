//! What a lab *is*: a brief somebody reads, and the measurement their work is
//! held to.
//!
//! Identity is content, as it is for a prompt version, an annotation revision
//! and a scorecard: the version id is a digest over the whole document, so
//! publishing the same lab twice lands on the version that is already there
//! and two instructors who wrote the same exercise reach one object.

use std::collections::BTreeMap;

use aiwatcher_evaluation::VersionReference;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{LabError, Result};

/// The largest brief accepted. A lesson longer than this is a handbook that
/// got pasted into the wrong field.
pub const MAX_BRIEF_BYTES: usize = 256 * 1024;
/// The label a participant reads. A publish with no label is the instructor
/// drafting next week's lab while the class is on this one.
pub const PUBLISHED_LABEL: &str = "published";
/// Version summaries kept in the head. A cap on the *index*, never on the
/// store: a version past it is still readable by id.
pub const MAX_VERSIONS_INDEXED: usize = 100;

/// A lab's name, which is also how it is addressed in a URL and a key.
///
/// Slug rules rather than free text, for the reason every other registry here
/// has them: the name is a path segment, and a name that can contain a slash
/// is a name that can name somebody else's object.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct LabName(String);

impl LabName {
    /// # Errors
    ///
    /// [`LabError::Invalid`] for anything that is not 1–64 bytes of lowercase
    /// letters, digits, `.`, `_` and `-`, beginning with a letter or a digit.
    pub fn parse(value: &str) -> Result<Self> {
        let ok = (1..=64).contains(&value.len())
            && value.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
            && value.chars().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-')
            });
        if ok {
            Ok(Self(value.to_owned()))
        } else {
            Err(LabError::Invalid {
                field: "name".into(),
                reason: "is 1–64 characters of a-z, 0-9, dot, underscore and hyphen, starting \
                         with a letter or a digit"
                    .into(),
            })
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for LabName {
    type Error = LabError;
    fn try_from(value: String) -> Result<Self> {
        Self::parse(&value)
    }
}

impl From<LabName> for String {
    fn from(value: LabName) -> Self {
        value.0
    }
}

impl std::fmt::Display for LabName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl utoipa::PartialSchema for LabName {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        utoipa::openapi::ObjectBuilder::new()
            .schema_type(utoipa::openapi::Type::String)
            .pattern(Some("^[a-z0-9][a-z0-9._-]{0,63}$"))
            .into()
    }
}
impl utoipa::ToSchema for LabName {}

/// The measurement a lab's work is held to.
///
/// Both halves are pinned at a version, and for the same reason a scoring run
/// pins its card: a head would let a rewrite change what an already-issued lab
/// measures between one participant's submission and the next.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = LabTests)]
#[serde(deny_unknown_fields)]
pub struct LabTests {
    /// The scorecard, as `name` and `version` — `GET
    /// /api/v1/evaluation-scorecards/{name}/versions` lists them.
    pub scorecard: VersionReference,
    /// The cohort, by the digest its three derived files are kept under:
    /// what `POST /api/v1/evaluation-cohorts` answered and `GET
    /// /api/v1/evaluation-cohorts/{cases}` reads back. It carries the dataset,
    /// the split and the case count, so a lab restating any of them would be
    /// free to disagree with the cases it points at.
    pub cases: String,
}

impl LabTests {
    pub(crate) fn validate(&self) -> Result<()> {
        crate::text(&self.scorecard.name, "tests.scorecard.name")?;
        crate::text(&self.scorecard.version, "tests.scorecard.version")?;
        crate::text(&self.cases, "tests.cases")?;
        crate::require(
            self.cases.len() == 64 && self.cases.chars().all(|c| c.is_ascii_hexdigit()),
            "tests.cases",
            "is the cohort's digest: 64 hex characters, as `POST /api/v1/evaluation-cohorts` \
             answered it",
        )
    }
}

/// An authored lab, as somebody wrote it.
///
/// Everything here is part of the version: a lab that measures different work
/// under one name is a different lab, and so is one whose brief was corrected.
/// The *tests* are named rather than copied, which is what lets the text be
/// fixed without every result already published being measured under something
/// else.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = Lab)]
#[serde(deny_unknown_fields)]
pub struct Lab {
    pub name: LabName,
    /// What it is called where somebody reads it.
    pub title: String,
    /// The instructions, as text. Authored, so it outlives the log — a
    /// participant reading last term's lab is the ordinary case. Nothing here
    /// or in the panel renders it as anything else: interpreting half of a
    /// syntax is worse than interpreting none, and choosing a renderer is a
    /// decision to take on purpose rather than as a side effect of drawing a
    /// lesson.
    pub brief: String,
    /// Where it sits in the workshop. Absent for a lab nobody has placed yet;
    /// nothing here refuses two labs in one position, because a registry that
    /// answered one lab at a time cannot see the other without reading them
    /// all, and a reader sorting by position and name gets a stable order
    /// either way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(minimum = 1, maximum = 999)]
    pub position: Option<u16>,
    /// The measurement, when one is pinned. Absent is the ordinary state of an
    /// instructor halfway through writing a lab, and it is reported as absent
    /// rather than filled in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests: Option<LabTests>,
}

impl Lab {
    /// # Errors
    ///
    /// [`LabError::Invalid`] naming the field, for a title or brief outside
    /// its bounds, a position outside 1–999, or tests that name neither a card
    /// version nor a cohort digest.
    pub fn validate(&self) -> Result<()> {
        crate::text(&self.title, "title")?;
        crate::require(
            !self.brief.is_empty() && self.brief.len() <= MAX_BRIEF_BYTES,
            "brief",
            "is between one byte and 256 KiB of text",
        )?;
        crate::require(
            !self.brief.contains('\0'),
            "brief",
            "must not contain a null byte",
        )?;
        if let Some(position) = self.position {
            crate::require(
                (1..=999).contains(&position),
                "position",
                "is between 1 and 999",
            )?;
        }
        if let Some(tests) = &self.tests {
            tests.validate()?;
        }
        Ok(())
    }

    /// The content address of this lab. Publishing the same document twice
    /// lands on the version that is already there.
    ///
    /// # Errors
    ///
    /// [`LabError::Encoding`] when the document cannot be canonicalised.
    pub fn version_id(&self) -> Result<String> {
        let bytes = crate::canonical(self)?;
        Ok(hex::encode(Sha256::digest(&bytes)))
    }
}

/// One published version of a lab.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = LabVersion)]
pub struct LabVersion {
    pub version_id: String,
    #[serde(flatten)]
    pub lab: Lab,
    /// Why this version exists: the commit message of a lab.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub published_at: i64,
}

/// A version as the head indexes it — enough to list, never the brief.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = LabVersionSummary)]
pub struct LabVersionSummary {
    pub version_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<u16>,
    /// Whether that version pinned a measurement. The tests themselves are in
    /// the version, because a summary carrying them would be a second copy
    /// that a reader could find disagreeing with the first.
    pub has_tests: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub published_at: i64,
}

impl From<&LabVersion> for LabVersionSummary {
    fn from(version: &LabVersion) -> Self {
        Self {
            version_id: version.version_id.clone(),
            title: version.lab.title.clone(),
            position: version.lab.position,
            has_tests: version.lab.tests.is_some(),
            notes: version.notes.clone(),
            author: version.author.clone(),
            published_at: version.published_at,
        }
    }
}

/// What a name points at: the versions published under it, newest first, and
/// where each label sits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = LabHead)]
pub struct LabHead {
    pub name: LabName,
    /// Newest first, capped at [`MAX_VERSIONS_INDEXED`].
    pub versions: Vec<LabVersionSummary>,
    /// Where each label points. `published` is the one a participant reads.
    pub labels: BTreeMap<String, String>,
    pub updated_at: i64,
}

impl LabHead {
    pub(crate) fn new(name: LabName, now: i64) -> Self {
        Self {
            name,
            versions: Vec::new(),
            labels: BTreeMap::new(),
            updated_at: now,
        }
    }

    /// The version this lab is *at* — what `published` points at, or the
    /// newest when nothing is labelled.
    #[must_use]
    pub fn current(&self) -> Option<&str> {
        self.labels
            .get(PUBLISHED_LABEL)
            .map(String::as_str)
            .or_else(|| self.versions.first().map(|v| v.version_id.as_str()))
    }

    /// The summary of [`LabHead::current`], for a list that must not open
    /// every version to say what each lab is called.
    #[must_use]
    pub fn current_summary(&self) -> Option<&LabVersionSummary> {
        let current = self.current()?;
        self.versions
            .iter()
            .find(|summary| summary.version_id == current)
    }

    pub(crate) fn index(&mut self, version: &LabVersion, now: i64) {
        self.versions
            .retain(|summary| summary.version_id != version.version_id);
        self.versions.insert(0, LabVersionSummary::from(version));
        self.versions.truncate(MAX_VERSIONS_INDEXED);
        self.updated_at = now;
    }
}
