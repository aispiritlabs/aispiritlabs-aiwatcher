//! Searching Kaggle and Hugging Face, without letting either of them decide
//! anything.
//!
//! [`sources`](crate::sources) is a table a human wrote and dated, and its
//! module docstring says why it is not a client: those two hubs, and Roboflow
//! Universe with them, restate corpus licences wrongly often enough that a
//! live answer would be *worse* than none, because it would arrive looking
//! authoritative. A CC BY-NC dataset re-uploaded as MIT is not a rare event.
//!
//! That rule is intact, and this module is what it looks like when you also
//! want the search. The split is between two different questions:
//!
//! **"What exists?"** is a discovery question, it has no wrong answer that
//! costs anything, and a hub answers it far better than a table of eight rows
//! ever will. That is what this module asks.
//!
//! **"What may we train on?"** is a permission question with an expensive
//! wrong answer, and no hub is allowed to answer it. Every result comes back
//! [`SourceUsage::Unclear`] with the hub's own words preserved verbatim in
//! [`HubDataset::claimed_license`] — *claimed*, in the field name, because
//! that is all it is. The single exception is a row that matches the curated
//! table by URL, which then carries the curated verdict and says which row it
//! came from.
//!
//! So the worst case is that somebody imports a corpus as
//! [`UsageRights::Unknown`](crate::license::UsageRights::Unknown), which a
//! commercial export excludes by name, in a manifest, forever. The failure
//! mode is a smaller export and a line saying why — not a model that was
//! quietly trained on somebody else's non-commercial data.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::license::SourceUsage;
use crate::sources::DatasetSource;

/// How long a hub gets to answer before the search gives up on it.
///
/// Short. A search is an interactive control, and one slow hub must not make
/// the other one feel broken — [`Hubs::search`] reports the timeout as that
/// hub's status and returns the results it did get.
const SEARCH_TIMEOUT: Duration = Duration::from_secs(8);

/// The most rows one hub may contribute.
///
/// A cap rather than a page. This is a discovery surface: the answer to "there
/// are four hundred matches" is a narrower query, not a scroll bar, and paging
/// a list nobody is going to reach the end of would be a cursor to maintain
/// for no reader.
const MAX_RESULTS: usize = 50;

/// Which mirror a row came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HubKind {
    Kaggle,
    /// Renamed explicitly. The derived snake_case is `hugging_face`, which is
    /// not what anybody writes, and this name is on the wire in three places
    /// that have to agree: the `hub=` query parameter, the Flow dataset's
    /// closed value set, and [`HubKind::as_str`]. Letting the derive pick it
    /// would have made two of the three wrong.
    #[serde(rename = "huggingface")]
    HuggingFace,
}

impl HubKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Kaggle => "kaggle",
            Self::HuggingFace => "huggingface",
        }
    }

    /// The variable that turns this hub on, named in every disabled status.
    ///
    /// The same shape as `RegistryDisabled`: "no results" and "this hub was
    /// never configured" are different problems with different fixes, and a
    /// surface that renders them identically sends somebody to look for a
    /// corpus that was never searched for.
    #[must_use]
    pub const fn variable(self) -> &'static str {
        match self {
            Self::Kaggle => "AIWATCHER_KAGGLE_USERNAME / AIWATCHER_KAGGLE_KEY",
            Self::HuggingFace => "AIWATCHER_HUGGINGFACE_ENABLED",
        }
    }
}

/// One file inside a hub dataset, as far as the hub will say.
///
/// Only enough to tell a floor-plan corpus from a CSV of house prices before
/// downloading either. Sizes are the hub's own and are not verified.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct HubFile {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

/// One dataset a hub says it has.
///
/// Every field here is the hub's, not aiwatcher's, with two exceptions:
/// [`usage`](Self::usage) and [`curated_source`](Self::curated_source), which
/// this module decides — see `Hubs::reconcile`.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct HubDataset {
    pub hub: HubKind,
    /// `owner/name` on both hubs, which is also how each addresses a download.
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub owner: String,
    pub url: String,
    /// What the *hub* says the licence is. Named for what it is.
    ///
    /// Empty is common and is not a licence: an unstated licence on a mirror
    /// means nobody filled the field in, never that the data is unencumbered.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub claimed_license: String,
    /// Always [`SourceUsage::Unclear`] unless [`curated_source`](Self::curated_source)
    /// is set. This is the guardrail, expressed as a field that cannot say
    /// "commercial" on a hub's word.
    pub usage: SourceUsage,
    /// The id of the [`DatasetSource`] row this matched, when it matched one.
    ///
    /// A match is on the *original's* URL, so it means "a human read this
    /// licence, at the source, on a date" — which is the only thing that ever
    /// justifies a usage other than unclear.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curated_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downloads: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub likes: Option<u64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<HubFile>,
}

/// Whether a hub answered, and if not, why.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct HubStatus {
    pub hub: HubKind,
    pub configured: bool,
    /// Rows this hub contributed to the page.
    pub results: usize,
    /// Present when the hub was asked and did not answer. A search with one
    /// hub down is a partial answer, and saying which half is missing beats
    /// both an error and a silently short list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// What to set when `configured` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variable: Option<String>,
}

/// What a caller asked for.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct HubQuery {
    /// Free text. Empty asks each hub for its own idea of what is popular,
    /// which is a worse question than any real one but a reasonable landing
    /// state for a search box nobody has typed in yet.
    #[serde(default)]
    pub q: Option<String>,
    /// One hub, or both when absent.
    #[serde(default)]
    pub hub: Option<HubKind>,
    #[serde(default)]
    pub limit: Option<usize>,
}

impl HubQuery {
    fn text(&self) -> &str {
        self.q.as_deref().unwrap_or("").trim()
    }

    fn limit(&self) -> usize {
        self.limit.unwrap_or(25).clamp(1, MAX_RESULTS)
    }

    fn wants(&self, hub: HubKind) -> bool {
        self.hub.is_none_or(|only| only == hub)
    }
}

/// A page of results, and what each hub had to say about answering.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct HubSearchPage {
    pub results: Vec<HubDataset>,
    pub hubs: Vec<HubStatus>,
    /// Repeated on every response rather than left to a tooltip, because it is
    /// the one thing a reader of this list has to know about it.
    pub notice: String,
}

/// The sentence that rides on every search response.
///
/// Public because the hub list route carries it too: a panel that renders the
/// hubs before anybody searches should already be saying this.
pub const NOTICE: &str = "A hub's licence field is what somebody typed when they uploaded a copy. \
Every row here is `unclear` unless it matches a corpus whose licence a human read at the \
original, and importing one registers its rights as unknown — which a commercial export \
excludes, by name, in its manifest.";

/// What is configured, and how to reach it.
#[derive(Clone, Debug, Default)]
pub struct HubConfig {
    /// Kaggle needs both halves; either alone is not a credential.
    pub kaggle_username: Option<String>,
    pub kaggle_key: Option<String>,
    /// Hugging Face's dataset search is public, so this is a switch rather
    /// than a credential: an instance that should not reach the internet at
    /// all leaves it off.
    pub huggingface: bool,
    /// Only for gated repositories. Search works without it.
    pub huggingface_token: Option<String>,
}

impl HubConfig {
    #[must_use]
    pub fn kaggle(&self) -> Option<(&str, &str)> {
        match (self.kaggle_username.as_deref(), self.kaggle_key.as_deref()) {
            (Some(user), Some(key)) if !user.is_empty() && !key.is_empty() => Some((user, key)),
            _ => None,
        }
    }

    #[must_use]
    pub const fn any(&self) -> bool {
        self.huggingface || (self.kaggle_username.is_some() && self.kaggle_key.is_some())
    }
}

/// The search surface. Holds one HTTP client and the curated table.
pub struct Hubs {
    http: reqwest::Client,
    config: HubConfig,
    curated: Vec<DatasetSource>,
}

impl std::fmt::Debug for Hubs {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Hubs")
            .field("kaggle", &self.config.kaggle().is_some())
            .field("huggingface", &self.config.huggingface)
            .field("curated", &self.curated.len())
            .finish()
    }
}

impl Hubs {
    /// # Errors
    /// When the HTTP client cannot be built, which is a TLS backend problem
    /// rather than a configuration one.
    pub fn new(config: HubConfig) -> Result<Self, reqwest::Error> {
        Self::with_catalog(config, Vec::new())
    }

    /// The same, reconciling against a catalogue the instance loaded.
    ///
    /// An empty one is the default and is safe: nothing matches, so every row
    /// keeps [`SourceUsage::Unclear`] and the mirror's claim stays labelled as
    /// the mirror's claim. See [`crate::sources`].
    ///
    /// # Errors
    /// When the HTTP client cannot be built, which is a TLS backend problem
    /// rather than a configuration one.
    pub fn with_catalog(
        config: HubConfig,
        curated: Vec<DatasetSource>,
    ) -> Result<Self, reqwest::Error> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(SEARCH_TIMEOUT)
                .user_agent(concat!("aiwatcher/", env!("CARGO_PKG_VERSION")))
                .build()?,
            config,
            curated,
        })
    }

    #[must_use]
    pub const fn config(&self) -> &HubConfig {
        &self.config
    }

    /// Every configured hub, whether or not it was searched.
    ///
    /// Served on its own route so the panel can render "Kaggle needs a key"
    /// before anybody types a query, rather than after a search returns half
    /// of what they expected.
    #[must_use]
    pub fn status(&self) -> Vec<HubStatus> {
        [HubKind::Kaggle, HubKind::HuggingFace]
            .into_iter()
            .map(|hub| {
                let configured = match hub {
                    HubKind::Kaggle => self.config.kaggle().is_some(),
                    HubKind::HuggingFace => self.config.huggingface,
                };
                HubStatus {
                    hub,
                    configured,
                    results: 0,
                    error: None,
                    variable: (!configured).then(|| hub.variable().to_owned()),
                }
            })
            .collect()
    }

    /// Ask every requested hub, and return what came back.
    ///
    /// Never fails as a whole. A hub that is down, rate-limited or refusing a
    /// credential becomes a [`HubStatus::error`] beside the results from the
    /// other one — the alternative is that one hub's outage empties a search
    /// that had a perfectly good answer from the other.
    pub async fn search(&self, query: &HubQuery) -> HubSearchPage {
        let mut page = HubSearchPage {
            notice: NOTICE.to_owned(),
            hubs: self.status(),
            results: Vec::new(),
        };

        for status in &mut page.hubs {
            if !status.configured || !query.wants(status.hub) {
                continue;
            }
            let found = match status.hub {
                HubKind::Kaggle => self.search_kaggle(query).await,
                HubKind::HuggingFace => self.search_huggingface(query).await,
            };
            match found {
                Ok(rows) => {
                    status.results = rows.len();
                    page.results.extend(rows);
                }
                Err(error) => status.error = Some(error),
            }
        }

        // Interleaved rather than concatenated, so one hub's hundred downloads
        // do not push the other's better match off the first screen. Neither
        // hub's popularity metric is comparable with the other's, which is
        // exactly why nothing here tries to rank across them.
        page.results.sort_by_key(|row| row.curated_source.is_none());
        page
    }

    async fn search_huggingface(&self, query: &HubQuery) -> Result<Vec<HubDataset>, String> {
        let limit = query.limit().to_string();
        let mut parameters = vec![("limit", limit.as_str()), ("full", "true")];
        let text = query.text();
        if !text.is_empty() {
            parameters.push(("search", text));
        }

        let mut request = self
            .http
            .get("https://huggingface.co/api/datasets")
            .query(&parameters);
        if let Some(token) = self.config.huggingface_token.as_deref() {
            request = request.bearer_auth(token);
        }

        let body = send(request, HubKind::HuggingFace).await?;
        let rows = body
            .as_array()
            .ok_or("Hugging Face did not return a list")?;
        Ok(rows
            .iter()
            .map(|row| self.huggingface_row(row))
            .collect::<Vec<_>>())
    }

    async fn search_kaggle(&self, query: &HubQuery) -> Result<Vec<HubDataset>, String> {
        let (user, key) = self.config.kaggle().ok_or(
            "Kaggle is not configured; set AIWATCHER_KAGGLE_USERNAME and AIWATCHER_KAGGLE_KEY",
        )?;

        let mut request = self
            .http
            .get("https://www.kaggle.com/api/v1/datasets/list")
            .basic_auth(user, Some(key));
        let text = query.text();
        if !text.is_empty() {
            request = request.query(&[("search", text)]);
        }

        let body = send(request, HubKind::Kaggle).await?;
        let rows = body.as_array().ok_or("Kaggle did not return a list")?;
        Ok(rows
            .iter()
            .take(query.limit())
            .map(|row| self.kaggle_row(row))
            .collect::<Vec<_>>())
    }

    fn huggingface_row(&self, row: &Value) -> HubDataset {
        let id = string(row, "id");
        let owner = id.split('/').next().unwrap_or_default().to_owned();
        let url = format!("https://huggingface.co/datasets/{id}");
        let tags = strings(row, "tags");
        // Hugging Face puts the licence in the tag list as `license:mit`
        // rather than in a field of its own, and `cardData.license` is only
        // present when the uploader filled in a model card.
        let claimed = tags
            .iter()
            .find_map(|tag| tag.strip_prefix("license:"))
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| {
                row.get("cardData")
                    .map(|card| string(card, "license"))
                    .unwrap_or_default()
            });

        self.reconcile(HubDataset {
            hub: HubKind::HuggingFace,
            title: id.clone(),
            summary: row
                .get("cardData")
                .map(|card| string(card, "pretty_name"))
                .unwrap_or_default(),
            id,
            owner,
            url,
            claimed_license: claimed,
            usage: SourceUsage::Unclear,
            curated_source: None,
            downloads: number(row, "downloads"),
            likes: number(row, "likes"),
            updated_at: string(row, "lastModified"),
            tags,
            files: row
                .get("siblings")
                .and_then(Value::as_array)
                .map(|entries| {
                    entries
                        .iter()
                        .take(20)
                        .map(|entry| HubFile {
                            name: string(entry, "rfilename"),
                            size_bytes: number(entry, "size"),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    fn kaggle_row(&self, row: &Value) -> HubDataset {
        let reference = string(row, "ref");
        let owner = row
            .get("ownerName")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| reference.split('/').next().unwrap_or_default().to_owned());

        self.reconcile(HubDataset {
            hub: HubKind::Kaggle,
            title: string(row, "title"),
            summary: string(row, "subtitle"),
            url: format!("https://www.kaggle.com/datasets/{reference}"),
            id: reference,
            owner,
            claimed_license: string(row, "licenseName"),
            usage: SourceUsage::Unclear,
            curated_source: None,
            downloads: number(row, "downloadCount"),
            likes: number(row, "voteCount"),
            updated_at: string(row, "lastUpdated"),
            tags: row
                .get("tags")
                .and_then(Value::as_array)
                .map(|tags| tags.iter().map(|tag| string(tag, "name")).collect())
                .unwrap_or_default(),
            files: Vec::new(),
        })
    }

    /// The one place a hub row is allowed to become anything but `unclear`.
    ///
    /// A mirror re-uploads a corpus under a name close to the original's, so
    /// the match is on that name — but *how* it is matched is the whole
    /// difference between a useful signal and a wrong one, and a plain
    /// substring test is wrong. `RPLAN` is a substring of `floorplans`, so
    /// `wall-constrained-floorplans-manual-only` would inherit RPLAN's licence
    /// verdict, which is a licence claim invented by a coincidence of
    /// spelling. That was not hypothetical: it is the first thing a live
    /// search against Hugging Face produced.
    ///
    /// So two rules, and the second is bounded by length:
    ///
    /// * a candidate matches when it is a whole **token** — the identifier
    ///   split on everything that is not a letter or a digit;
    /// * a candidate of eight characters or more may also match across
    ///   separators, so `cubicasa-5k` still finds `cubicasa5k`. Below eight it
    ///   may not, because that is the length at which a coincidence stops
    ///   being one.
    ///
    /// Both directions of failure are safe by construction: a miss leaves the
    /// row [`SourceUsage::Unclear`], which is where every row starts.
    fn reconcile(&self, mut row: HubDataset) -> HubDataset {
        let text = format!("{} {} {}", row.id, row.title, row.summary);
        let tokens: BTreeSet<String> = text
            .split(|character: char| !character.is_ascii_alphanumeric())
            .filter(|token| !token.is_empty())
            .map(str::to_lowercase)
            .collect();
        let squashed: String = text
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_lowercase();

        for source in &self.curated {
            let candidates = [squash(&source.id), squash(&source.name)];
            let matched = candidates.iter().any(|candidate| {
                candidate.len() >= 4
                    && (tokens.contains(candidate)
                        || (candidate.len() >= 8 && squashed.contains(candidate.as_str())))
            });
            if matched {
                row.usage = source.usage;
                row.curated_source = Some(source.id.clone());
                return row;
            }
        }
        row
    }
}

/// A name reduced to the letters and digits in it, lowercased.
///
/// `CubiCasa5K` and `cubicasa-5k` are the same corpus, and `CVC-FP` is
/// `cvcfp`. What this must never do is shorten a name to the point where it
/// collides with an ordinary word — which is why the length bounds are on the
/// *caller* rather than here.
fn squash(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_lowercase()
}

/// Where a hub row's usage came from, for a caller that has to decide.
///
/// Not derived from the fields at the point of use, because the two states a
/// caller must not conflate — "a human read this licence" and "a mirror said
/// so" — differ only in whether an `Option` is set.
#[must_use]
pub fn rights_provenance(row: &HubDataset) -> &'static str {
    match (&row.curated_source, row.usage) {
        (Some(_), SourceUsage::Commercial) => "curated: a human read this licence at the original",
        (Some(_), SourceUsage::NonCommercial) => {
            "curated: research only at the original, whatever the mirror says"
        }
        _ => "mirror: nobody has checked this licence at its source",
    }
}

async fn send(request: reqwest::RequestBuilder, hub: HubKind) -> Result<Value, String> {
    let response = request
        .send()
        .await
        .map_err(|error| format!("{} did not answer: {error}", hub.as_str()))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        // The body is truncated rather than dropped: a Kaggle 403 says whether
        // the credential is wrong or the account has not accepted the terms,
        // and those are different afternoons.
        let detail: String = body.chars().take(200).collect();
        return Err(format!("{} answered {status}: {detail}", hub.as_str()));
    }
    serde_json::from_str(&body)
        .map_err(|error| format!("{} sent something else: {error}", hub.as_str()))
}

fn string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn number(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::as_u64)
}

fn strings(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// One hub row, flattened into the columns a Flow query reads.
///
/// The bridge between this module and `services/flow`: the panel's import
/// pipeline is written against these names, so they are defined once, here,
/// beside the struct they come from. A column list that lived only in PHP
/// would drift from the struct the first time a field was renamed.
#[must_use]
pub fn as_row(row: &HubDataset) -> BTreeMap<String, Value> {
    BTreeMap::from([
        ("hub".to_owned(), Value::from(row.hub.as_str())),
        ("dataset_id".to_owned(), Value::from(row.id.clone())),
        ("title".to_owned(), Value::from(row.title.clone())),
        ("owner".to_owned(), Value::from(row.owner.clone())),
        ("url".to_owned(), Value::from(row.url.clone())),
        (
            "claimed_license".to_owned(),
            Value::from(row.claimed_license.clone()),
        ),
        ("usage".to_owned(), Value::from(row.usage.as_str())),
        (
            "curated_source".to_owned(),
            row.curated_source.clone().map_or(Value::Null, Value::from),
        ),
        (
            "rights_provenance".to_owned(),
            Value::from(rights_provenance(row)),
        ),
        (
            "downloads".to_owned(),
            row.downloads.map_or(Value::Null, Value::from),
        ),
        (
            "likes".to_owned(),
            row.likes.map_or(Value::Null, Value::from),
        ),
        ("updated_at".to_owned(), Value::from(row.updated_at.clone())),
        ("tags".to_owned(), Value::from(row.tags.clone())),
        ("files".to_owned(), Value::from(row.files.len() as u64)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A catalogue of one, built here rather than shipped.
    ///
    /// This crate ships no corpora — see [`crate::sources`] — so a test that
    /// wants a curated verdict has to state one, which is also how a
    /// deployment gets one. The name is long enough to exercise the
    /// cross-separator rule and distinctive enough not to collide.
    fn curated() -> Vec<DatasetSource> {
        vec![DatasetSource {
            id: "cubicasa5k".to_owned(),
            name: "CubiCasa5K".to_owned(),
            license: "CC BY-NC 4.0".to_owned(),
            usage: SourceUsage::NonCommercial,
            url: "https://example.test/cubicasa5k".to_owned(),
            verified_on: "2026-09-02".to_owned(),
            ..DatasetSource::default()
        }]
    }

    fn hubs() -> Hubs {
        Hubs::with_catalog(
            HubConfig {
                huggingface: true,
                ..HubConfig::default()
            },
            curated(),
        )
        .expect("the client builds")
    }

    /// The default: nothing curated, so nothing can outrank a mirror.
    fn bare() -> Hubs {
        Hubs::new(HubConfig {
            huggingface: true,
            ..HubConfig::default()
        })
        .expect("the client builds")
    }

    #[test]
    fn a_hub_row_is_unclear_however_confidently_the_mirror_states_its_licence() {
        let row = hubs().huggingface_row(&serde_json::json!({
            "id": "someone/plans-mirror",
            "tags": ["license:mit", "task_categories:image-segmentation"],
            "downloads": 4_000,
        }));

        assert_eq!(row.claimed_license, "mit");
        assert_eq!(row.usage, SourceUsage::Unclear);
        assert!(row.curated_source.is_none());
        assert!(rights_provenance(&row).starts_with("mirror"));
    }

    #[test]
    fn a_row_matching_the_curated_table_takes_the_verdict_a_human_recorded() {
        let row = hubs().huggingface_row(&serde_json::json!({
            "id": "someone/cubicasa5k",
            "tags": ["license:mit"],
        }));

        // The mirror says MIT. The table says CC BY-NC, because somebody read
        // it at the original, on a date. The table wins, and says so.
        assert_eq!(row.claimed_license, "mit");
        assert_eq!(row.usage, SourceUsage::NonCommercial);
        assert_eq!(row.curated_source.as_deref(), Some("cubicasa5k"));
        assert!(rights_provenance(&row).contains("research only"));
    }

    #[test]
    fn a_word_that_merely_contains_a_corpus_name_is_not_that_corpus() {
        // Found by the first live search against Hugging Face. "rplan" is a
        // substring of "floorplans", so a plain `contains` handed this row
        // RPLAN's licence verdict — a permission claim invented by a
        // coincidence of spelling.
        let row = hubs().huggingface_row(&serde_json::json!({
            "id": "zimhe/wall-constrained-floorplans-manual-only",
        }));

        assert_eq!(row.usage, SourceUsage::Unclear);
        assert!(row.curated_source.is_none(), "{:?}", row.curated_source);
    }

    #[test]
    fn a_mirror_that_respelled_the_name_across_a_separator_still_matches() {
        // Long enough that the coincidence risk is gone, so this one is
        // allowed to match across the hyphen.
        let row = hubs().huggingface_row(&serde_json::json!({
            "id": "someone/cubicasa-5k-segmentation",
        }));

        assert_eq!(row.curated_source.as_deref(), Some("cubicasa5k"));
        assert_eq!(row.usage, SourceUsage::NonCommercial);
    }

    #[test]
    fn with_no_catalogue_loaded_every_row_stays_unclear() {
        // The shipped default. An instance that loaded no table cannot promote
        // a mirror's claim to a verdict, which is the safe direction reached
        // by doing nothing.
        let row = bare().huggingface_row(&serde_json::json!({
            "id": "someone/cubicasa5k",
            "tags": ["license:mit"],
        }));

        assert_eq!(row.usage, SourceUsage::Unclear);
        assert!(row.curated_source.is_none());
        assert_eq!(row.claimed_license, "mit");
    }

    #[test]
    fn a_short_corpus_id_has_to_be_a_whole_word() {
        // `r2v` is three characters and never matches; `zind` is four and
        // matches only as a token. Both failures leave the row unclear, which
        // is the direction that costs an export rather than a lawsuit.
        let row = hubs().huggingface_row(&serde_json::json!({
            "id": "someone/cubicasa-annotations-extra",
        }));

        assert_eq!(row.usage, SourceUsage::Unclear);
        assert!(row.curated_source.is_none());
    }

    #[test]
    fn kaggle_is_not_configured_by_half_a_credential() {
        let config = HubConfig {
            kaggle_username: Some("someone".to_owned()),
            ..HubConfig::default()
        };
        assert!(config.kaggle().is_none());
        assert!(!config.any());
    }

    #[test]
    fn a_disabled_hub_names_the_variable_that_turns_it_on() {
        let status = hubs().status();
        let kaggle = status
            .iter()
            .find(|entry| entry.hub == HubKind::Kaggle)
            .expect("kaggle is listed even when off");
        assert!(!kaggle.configured);
        assert!(
            kaggle
                .variable
                .as_deref()
                .unwrap_or_default()
                .contains("KAGGLE")
        );
    }

    #[tokio::test]
    async fn a_hub_that_was_never_configured_is_never_asked() {
        let page = Hubs::new(HubConfig::default())
            .expect("the client builds")
            .search(&HubQuery::default())
            .await;

        assert!(page.results.is_empty());
        assert!(page.hubs.iter().all(|status| status.error.is_none()));
        assert!(page.notice.contains("unclear"));
    }

    #[test]
    fn the_flow_row_carries_the_provenance_and_not_only_the_verdict() {
        let row = hubs().kaggle_row(&serde_json::json!({
            "ref": "someone/floor-plans",
            "title": "Floor plans",
            "licenseName": "CC0: Public Domain",
        }));
        let flat = as_row(&row);

        assert_eq!(flat["claimed_license"], Value::from("CC0: Public Domain"));
        assert_eq!(flat["usage"], Value::from("unclear"));
        assert!(
            flat["rights_provenance"]
                .as_str()
                .unwrap()
                .starts_with("mirror")
        );
    }
}

#[cfg(test)]
mod wire {
    use super::*;

    #[test]
    fn the_hub_name_is_the_same_string_everywhere_it_appears() {
        // Three places have to agree: the `hub=` query parameter serde parses,
        // the closed value set the Flow dataset declares, and `as_str`. The
        // derived snake_case would have been `hugging_face`, silently making
        // one of the three reject what the others send.
        for hub in [HubKind::Kaggle, HubKind::HuggingFace] {
            let encoded = serde_json::to_string(&hub).expect("serialises");
            assert_eq!(encoded, format!("\"{}\"", hub.as_str()));
            let decoded: HubKind = serde_json::from_str(&encoded).expect("round trips");
            assert_eq!(decoded, hub);
        }
        assert_eq!(HubKind::HuggingFace.as_str(), "huggingface");
    }
}
