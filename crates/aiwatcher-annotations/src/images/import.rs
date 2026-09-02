//! Registering many images at once, from rows a Flow PHP pipeline produced.
//!
//! The bulk half of the [`images`](crate::images) slice. Everything the single
//! image path checks, checked the same way: this runs the *same* validation
//! and the *same* write as a one-at-a-time registration, so a dry run and a
//! write cannot disagree about what is valid.
//!
//! The other end of [`hubs`](crate::integrations::hubs). A search says a corpus exists; this
//! is how the images in it become rows in a project somebody can draw on. In
//! between sits a Flow query, for a reason worth stating rather than assuming:
//! **every hub lays its files out differently**, and a mapping written in
//! Rust would be a `match` on hub names that grows by one arm per corpus and
//! is wrong for the next one.
//!
//! A Flow pipeline is where that mapping belongs. It is a saved recipe (the
//! curation registry already versions those), it is readable by whoever has to
//! fix it, and it is the same surface the panel already uses to build a
//! dataset. What arrives here is a list of rows that already have this
//! module's column names.
//!
//! ## What this refuses
//!
//! Three things, and each is a mistake that is invisible afterwards.
//!
//! **A claimed licence.** [`ImportRequest::rights`] is what the *caller*
//! asserts, and the caller is a person. A hub row cannot supply it: the
//! discovery surface carries [`SourceUsage::Unclear`] and the panel offers
//! [`UsageRights::Unknown`], which a commercial export then excludes by name.
//! Somebody who has read the licence at the original can say otherwise, and
//! that assertion is recorded with their name on it.
//!
//! **A licence better than the curated table's.** When the row matched a
//! curated corpus and that corpus is research-only, an import claiming
//! commercial terms is refused outright. This is the one case where aiwatcher
//! knows more than the person clicking: a human read that licence at the
//! source, on a date, and wrote it down.
//!
//! **A family key that is really an image key.** [`ImportRow::group_id`] is
//! the *building*. A pipeline that mapped it from the file name gives every
//! image its own family, which silently turns the family split back into a
//! per-image split — and nothing in the numbers afterwards says so. So a
//! request whose rows are all singleton families is reported as such, loudly,
//! on a response that still succeeded.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::images::RegisterImageRequest;
use crate::license::{SourceUsage, UsageRights, check_rights};
use crate::project::AnnotationProject;
use crate::store::Backend;
use crate::{Error, Result};

/// The most images one import may carry.
///
/// A bounded request, like every other write in this crate. Five thousand
/// images is more than any catalogue import and far less than a corpus dump,
/// which is the size at which somebody should be running this from a script
/// against the single-image route rather than through a browser.
pub const MAX_IMPORT_ROWS: usize = 5_000;

/// One image a pipeline produced.
///
/// Deliberately the same field names as [`RegisterImageRequest`], minus what
/// the request supplies once for the whole batch. A Flow query that renames
/// its columns to these is the entire mapping layer.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportRow {
    /// SHA-256 of the bytes, when the pipeline has already fetched and stored
    /// them. Absent for a row that only points at a remote file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_id: Option<String>,
    pub uri: String,
    pub width: u32,
    pub height: u32,
    /// The **building**, not the drawing. See the module docstring.
    pub group_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub view: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schema(value_type = Object)]
    pub metadata: BTreeMap<String, Value>,
}

/// Where a batch came from, recorded on every image it produces.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportSource {
    /// `kaggle` or `huggingface`, or anything else for a pipeline that did not
    /// start at a hub.
    #[serde(default)]
    pub hub: String,
    #[serde(default)]
    pub dataset_id: String,
    #[serde(default)]
    pub url: String,
    /// What the hub said the licence was. Kept verbatim and never believed —
    /// it is evidence about the mirror, not about the data.
    #[serde(default)]
    pub claimed_license: String,
    /// The curated row this matched, if any. The presence of this is what lets
    /// [`check_rights`] refuse an over-claim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curated_source: Option<String>,
    /// The curated verdict, when there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curated_usage: Option<SourceUsage>,
    /// The Flow PHP that produced these rows. Provenance that survives the
    /// import: "where did these six hundred images come from" has an answer
    /// that is a script rather than a memory.
    #[serde(default)]
    pub pipeline: String,
}

/// A bulk registration.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
    pub project: String,
    /// What the caller asserts may be done with every image in this batch.
    ///
    /// One value for the batch rather than per row, because a licence is a
    /// property of the corpus and a per-row field invites a pipeline to derive
    /// it from a column — which is exactly the mirror's word, laundered
    /// through a `withEntry`.
    #[serde(default = "unknown_rights")]
    pub rights: UsageRights,
    #[serde(default)]
    pub source: ImportSource,
    pub rows: Vec<ImportRow>,
    /// Check everything and register nothing.
    ///
    /// The default is false, but the panel always asks for a dry run first:
    /// six hundred rows with a group key mapped from the filename is not
    /// something anybody wants to discover after it is in the project.
    #[serde(default)]
    pub dry_run: bool,
}

const fn unknown_rights() -> UsageRights {
    UsageRights::Unknown
}

/// What happened to one row.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum RowOutcome {
    /// Registered, or would have been under `dry_run`.
    Accepted { image_id: String },
    /// Refused, with the reason. One row's problem never fails the batch: an
    /// import of six hundred where four have a bad URI should register five
    /// hundred and ninety-six and say which four.
    Rejected { uri: String, reason: String },
}

/// What an import did.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct ImportReport {
    pub project: String,
    pub dry_run: bool,
    pub accepted: usize,
    pub rejected: usize,
    /// Distinct [`ImportRow::group_id`] values across the batch.
    pub families: usize,
    /// Rows whose bytes were downloaded from a hub and stored here.
    ///
    /// Set by the caller that did the downloading, not by the import: this
    /// module writes an object store and reaches nothing. Zero for a batch
    /// whose pipeline had already stored its own bytes, which is every batch
    /// that carries an `image_id`.
    #[serde(default)]
    pub fetched: usize,
    pub outcomes: Vec<RowOutcome>,
    /// Things that are not errors and that somebody has to read anyway.
    ///
    /// Every one of these describes a state where the import succeeds and the
    /// resulting corpus is quietly worth less than it looks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Everything worth saying about a batch that is not a refusal.
#[must_use]
pub fn warnings(request: &ImportRequest, families: usize) -> Vec<String> {
    let mut warnings = Vec::new();
    let rows = request.rows.len();

    if rows > 1 && families == rows {
        warnings.push(format!(
            "every one of the {rows} images is its own family. The split key is the building, so \
             a mirrored plan and its original have to share a group_id — otherwise the test score \
             measures memorisation and nothing in the numbers says so. Check what the pipeline \
             mapped group_id from."
        ));
    }
    if matches!(request.rights, UsageRights::Unknown) {
        warnings.push(
            "registered with unknown rights, so a commercial export will exclude every one of \
             these images and say so in its manifest. That is the safe default, not a failure — \
             set the rights once somebody has read the licence at its original."
                .to_owned(),
        );
    }
    if !request.source.claimed_license.is_empty()
        && matches!(request.rights, UsageRights::Unknown)
        && request.source.curated_source.is_none()
    {
        warnings.push(format!(
            "the mirror claims '{}'. Nobody has checked that at the source, which is why it did \
             not become the rights on these images.",
            request.source.claimed_license
        ));
    }
    warnings
}

/// The distinct families in a batch.
#[must_use]
pub fn families(rows: &[ImportRow]) -> usize {
    rows.iter()
        .map(|row| row.group_id.as_str())
        .collect::<BTreeSet<_>>()
        .len()
}

/// One row, as the single-image route would have received it.
///
/// `source` is composed here rather than taken from the row so that every
/// image in a batch carries the same provenance string, which is what makes
/// "show me everything that came from that Kaggle import" a search rather than
/// an archaeology.
#[must_use]
pub fn to_request(
    row: ImportRow,
    project: &str,
    rights: &UsageRights,
    source: &ImportSource,
) -> RegisterImageRequest {
    let mut metadata = row.metadata;
    metadata.insert("import.hub".to_owned(), Value::from(source.hub.clone()));
    metadata.insert(
        "import.dataset".to_owned(),
        Value::from(source.dataset_id.clone()),
    );
    if !source.url.is_empty() {
        metadata.insert("import.url".to_owned(), Value::from(source.url.clone()));
    }
    if !source.claimed_license.is_empty() {
        // Kept beside the rights rather than instead of them. When somebody
        // later asks why a corpus is `unknown`, the mirror's claim is the
        // first thing worth seeing and the last thing worth believing.
        metadata.insert(
            "import.claimed_license".to_owned(),
            Value::from(source.claimed_license.clone()),
        );
    }
    if let Some(curated) = &source.curated_source {
        metadata.insert(
            "import.curated_source".to_owned(),
            Value::from(curated.clone()),
        );
    }
    if !source.pipeline.is_empty() {
        metadata.insert(
            "import.pipeline".to_owned(),
            Value::from(source.pipeline.clone()),
        );
    }

    RegisterImageRequest {
        project: project.to_owned(),
        // Empty means the caller had no bytes to hash. The registry refuses
        // that rather than inventing one: an image id is a content address,
        // and a made-up one is two different pictures sharing a key.
        image_id: row.image_id.unwrap_or_default(),
        uri: row.uri,
        width: row.width,
        height: row.height,
        group_id: row.group_id,
        source: if source.dataset_id.is_empty() {
            source.hub.clone()
        } else {
            format!("{}:{}", source.hub, source.dataset_id)
        },
        rights: rights.clone(),
        view: row.view,
        level: row.level,
        metadata,
    }
}

// ── The operation ────────────────────────────────────────────────────────────

/// Register many images from rows a pipeline produced.
///
/// One row's problem never fails the batch. An import of six hundred where
/// four have an unreachable URI should register five hundred and ninety-six
/// and name the four — the alternative is a caller who has to bisect their
/// own pipeline to find out which row the 400 was about.
///
/// What *does* fail the whole batch is the rights check, before any write:
/// a claim that contradicts what a human recorded about the corpus is a
/// decision to reverse, not a row to skip.
///
/// # Errors
/// When the project does not exist, the batch is over
/// [`MAX_IMPORT_ROWS`](crate::import::MAX_IMPORT_ROWS), or the asserted
/// rights over-claim against the curated table.
pub(crate) async fn import(
    backend: &Backend,
    project: &AnnotationProject,
    request: ImportRequest,
) -> Result<ImportReport> {
    if request.rows.len() > MAX_IMPORT_ROWS {
        return Err(Error::TooLarge {
            what: "an import",
            size: request.rows.len(),
            limit: MAX_IMPORT_ROWS,
        });
    }
    check_rights(
        &request.rights,
        request.source.curated_usage,
        request.source.curated_source.as_deref(),
    )
    .map_err(Error::Invalid)?;

    let families = families(&request.rows);
    let mut report = ImportReport {
        project: project.name.clone(),
        dry_run: request.dry_run,
        families,
        warnings: warnings(&request, families),
        ..ImportReport::default()
    };

    for row in request.rows {
        let uri = row.uri.clone();
        let single = to_request(row, &project.name, &request.rights, &request.source);
        let image_id = single.image_id.clone();

        // A dry run validates against the same code the write uses rather
        // than a copy of its rules. A preview that checks something else is
        // a preview that passes and then fails.
        let outcome = if request.dry_run {
            super::check(&single).map(|()| image_id)
        } else {
            super::register(backend, project, single)
                .await
                .map(|head| head.image.image_id)
        };

        match outcome {
            Ok(image_id) => {
                report.accepted += 1;
                report.outcomes.push(RowOutcome::Accepted { image_id });
            }
            Err(error) => {
                report.rejected += 1;
                report.outcomes.push(RowOutcome::Rejected {
                    uri,
                    reason: error.to_string(),
                });
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(curated: Option<SourceUsage>) -> ImportSource {
        ImportSource {
            hub: "huggingface".to_owned(),
            dataset_id: "someone/plans".to_owned(),
            claimed_license: "mit".to_owned(),
            curated_source: curated.map(|_| "cubicasa5k".to_owned()),
            curated_usage: curated,
            ..ImportSource::default()
        }
    }

    fn row(group: &str) -> ImportRow {
        ImportRow {
            image_id: Some("ab".repeat(32)),
            uri: "https://example.test/plan.png".to_owned(),
            width: 1064,
            height: 1021,
            group_id: group.to_owned(),
            level: None,
            view: "photo".to_owned(),
            metadata: BTreeMap::new(),
        }
    }

    #[test]
    fn a_batch_where_every_image_is_its_own_family_says_so() {
        let request = ImportRequest {
            project: "floor-plans/import".to_owned(),
            rights: UsageRights::ResearchOnly {
                license: "CC BY-NC 4.0".to_owned(),
                url: None,
            },
            source: source(None),
            rows: vec![row("plan-1"), row("plan-2"), row("plan-3")],
            dry_run: false,
        };
        let found = warnings(&request, families(&request.rows));

        assert!(
            found.iter().any(|line| line.contains("its own family")),
            "a per-image split has to be loud: {found:?}"
        );
    }

    #[test]
    fn families_that_share_a_building_are_not_warned_about() {
        let rows = vec![row("komancza-dws"), row("komancza-dws"), row("bergamo")];
        let request = ImportRequest {
            project: "floor-plans/import".to_owned(),
            rights: UsageRights::Owned {
                grant: "supplied under contract".to_owned(),
            },
            source: source(None),
            rows,
            dry_run: false,
        };
        assert_eq!(families(&request.rows), 2);
        assert!(
            !warnings(&request, families(&request.rows))
                .iter()
                .any(|line| line.contains("its own family"))
        );
    }

    #[test]
    fn unknown_rights_are_the_default_and_the_response_says_what_that_costs() {
        let request: ImportRequest = serde_json::from_value(serde_json::json!({
            "project": "floor-plans/import",
            "rows": [],
        }))
        .expect("rights may be omitted");

        assert!(matches!(request.rights, UsageRights::Unknown));
        assert!(
            warnings(&request, 0)
                .iter()
                .any(|line| line.contains("commercial export will exclude"))
        );
    }

    #[test]
    fn every_imported_image_carries_where_it_came_from() {
        let request = to_request(
            row("komancza-dws"),
            "floor-plans/import",
            &UsageRights::Unknown,
            &source(Some(SourceUsage::NonCommercial)),
        );

        assert_eq!(request.source, "huggingface:someone/plans");
        assert_eq!(
            request.metadata["import.claimed_license"],
            Value::from("mit")
        );
        assert_eq!(
            request.metadata["import.curated_source"],
            Value::from("cubicasa5k")
        );
    }
}
