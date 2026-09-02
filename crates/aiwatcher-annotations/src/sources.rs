//! Where training data comes from, and what a human recorded about its licence.
//!
//! A table somebody wrote and dated, not a client. Public mirrors — Hugging
//! Face, Kaggle, Roboflow Universe — routinely restate a corpus's licence
//! wrongly, and a CC BY-NC dataset re-uploaded as MIT is common enough that
//! fetching a licence live would be worse than useless: it would arrive
//! looking authoritative. See [`crate::integrations::hubs`], which searches
//! those mirrors and is never allowed to believe them.
//!
//! Every row says what it says *as of* a date, links the original, and errs
//! towards [`SourceUsage::Unclear`]. It is a signpost. The only thing that is
//! a permission is the licence text at the other end of the link.
//!
//! # This build ships no rows
//!
//! The table is domain content, not code: which corpora exist and what their
//! licences permit is a question about one field of vision, and a list shipped
//! here would be one project's homework imposed on everybody else's. So the
//! default catalogue is **empty**, and an instance loads its own from a JSON
//! file named by `AIWATCHER_DATASET_SOURCES`.
//!
//! Empty is a safe default rather than a degraded one. With no rows nothing
//! matches, every hub result stays `unclear`, and an import of one registers
//! its images with unknown rights — which a commercial export excludes by
//! name, in a manifest, forever. The cost of an empty table is a smaller
//! export and a line saying why; the cost of a wrong row is a licence claim.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::license::SourceUsage;

/// How the bytes are obtained.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceAccess {
    /// Downloadable.
    #[default]
    Open,
    /// A form, an agreement, or an email.
    Request,
    /// Restricted to academic or public research institutions.
    Academic,
}

/// One corpus.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
#[serde(default)]
pub struct DatasetSource {
    /// Stable slug. What [`crate::integrations::hubs`] matches a mirror
    /// against, and what an import records as the row it was checked against.
    pub id: String,
    pub name: String,
    /// What kind of thing it contains, in the caller's own words —
    /// `floor_plan`, `satellite`, `radiograph`. Free text for the same reason
    /// [`crate::images::ImageRecord::view`] is: an enum here would be one
    /// domain's list.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// Best published figure. `None` where the authors give a range.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<u64>,
    /// What one item is: `images`, `scans`, `studies`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub item_label: String,
    /// What it labels, in whatever vocabulary the corpus uses.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// `raster`, `vector`, `video`, `point_cloud`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub media: Vec<String>,
    pub license: String,
    pub usage: SourceUsage,
    pub access: SourceAccess,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paper: Option<String>,
    /// When somebody last read the licence at `url`.
    ///
    /// The field that makes a row worth more than a guess, and the one that
    /// goes stale. A row with no date is a row nobody checked.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub verified_on: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

/// A place to go looking, rather than a corpus.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct SourceDirectory {
    pub name: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct SourcePage {
    pub sources: Vec<DatasetSource>,
    pub directories: Vec<SourceDirectory>,
    /// Rows before the filter, so a caller can tell an empty filter from an
    /// empty table.
    pub total: usize,
}

/// A loaded catalogue: the corpora somebody checked, and where to look for more.
///
/// Cloneable and cheap to hold. An instance loads one at start-up and every
/// reader shares it — the alternative, re-reading a file per request, would
/// make the answer depend on when it was asked.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct SourceCatalog {
    #[serde(default)]
    pub sources: Vec<DatasetSource>,
    #[serde(default)]
    pub directories: Vec<SourceDirectory>,
}

impl SourceCatalog {
    /// Parse a catalogue from the JSON an instance was pointed at.
    ///
    /// # Errors
    /// When the document is not a catalogue, or a row has no `id` — the field
    /// a hub match is made on, and the one whose absence would make a row
    /// unreachable rather than wrong.
    pub fn parse(body: &[u8]) -> crate::Result<Self> {
        let catalog: Self = serde_json::from_slice(body).map_err(|error| {
            crate::Error::Invalid(format!(
                "the dataset source catalogue is not valid: {error}"
            ))
        })?;
        for source in &catalog.sources {
            if source.id.trim().is_empty() {
                return Err(crate::Error::Invalid(
                    "every dataset source needs an id; it is what a hub result is matched against"
                        .to_owned(),
                ));
            }
        }
        Ok(catalog)
    }

    /// The rows matching a filter, with the total before it.
    #[must_use]
    pub fn search(
        &self,
        query: Option<&str>,
        usage: Option<SourceUsage>,
        label: Option<&str>,
    ) -> SourcePage {
        let needle = query.map(str::to_lowercase);
        let label = label.map(str::to_lowercase);
        let sources = self
            .sources
            .iter()
            .filter(|source| usage.is_none_or(|wanted| source.usage == wanted))
            .filter(|source| {
                label
                    .as_ref()
                    .is_none_or(|wanted| source.labels.iter().any(|held| held == wanted))
            })
            .filter(|source| {
                needle.as_ref().is_none_or(|needle| {
                    source.id.to_lowercase().contains(needle)
                        || source.name.to_lowercase().contains(needle)
                        || source.summary.to_lowercase().contains(needle)
                        || source.notes.to_lowercase().contains(needle)
                        || source.license.to_lowercase().contains(needle)
                        || source.labels.iter().any(|value| value.contains(needle))
                        || source.media.iter().any(|value| value.contains(needle))
                })
            })
            .cloned()
            .collect();
        SourcePage {
            sources,
            directories: self.directories.clone(),
            total: self.sources.len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_instance_that_loaded_nothing_answers_with_an_empty_table() {
        // Not an error state. Nothing matches, every hub row stays unclear,
        // and an import records unknown rights — which a commercial export
        // excludes by name. The safe direction, reached by doing nothing.
        let page = SourceCatalog::default().search(None, None, None);
        assert!(page.sources.is_empty());
        assert_eq!(page.total, 0);
    }

    #[test]
    fn a_row_with_no_id_is_refused_because_nothing_could_ever_match_it() {
        let refused = SourceCatalog::parse(br#"{"sources":[{"name":"a corpus"}]}"#);
        assert!(refused.is_err());
    }

    #[test]
    fn a_catalogue_round_trips_and_is_searchable_by_what_it_labels() {
        let catalog = SourceCatalog::parse(
            br#"{"sources":[
                {"id":"corpus-a","name":"Corpus A","labels":["cells"],
                 "license":"CC BY-NC 4.0","usage":"non_commercial","access":"open",
                 "url":"https://example.test/a","verified_on":"2026-09-02"},
                {"id":"corpus-b","name":"Corpus B","labels":["roads"],
                 "license":"CC0","usage":"commercial","access":"open",
                 "url":"https://example.test/b","verified_on":"2026-09-02"}
            ]}"#,
        )
        .expect("a catalogue");

        assert_eq!(catalog.search(None, None, Some("cells")).sources.len(), 1);
        assert_eq!(
            catalog
                .search(None, Some(SourceUsage::Commercial), None)
                .sources
                .len(),
            1
        );
        // The total is before the filter, so an empty result reads as "nothing
        // matched" rather than as "nothing is loaded".
        assert_eq!(catalog.search(Some("nothing"), None, None).total, 2);
    }
}
