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

use crate::integrations::pixels;
use crate::license::SourceUsage;
use crate::sources::DatasetSource;

/// How long a hub gets to answer before the search gives up on it.
///
/// Short. A search is an interactive control, and one slow hub must not make
/// the other one feel broken — [`Hubs::search`] reports the timeout as that
/// hub's status and returns the results it did get.
const SEARCH_TIMEOUT: Duration = Duration::from_secs(8);

/// How long a hub gets to hand over one image.
///
/// Longer than [`SEARCH_TIMEOUT`], and for the opposite reason: a search is an
/// interactive control that must feel fast, while this runs inside an import
/// somebody has already decided to wait for. A megabyte over a slow link is
/// normal here and is not a hub that is down.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// The most images one call may list.
///
/// Hugging Face's own rows endpoint caps a page at 100.
const MAX_IMAGES: usize = 100;

/// The most one carried column may weigh, serialised.
///
/// A row is handed over whole so a script can reach any of it, and one column
/// is occasionally a document: `pixparse/idl-wds` publishes a `json` column
/// holding every word box on the page. Carrying that for a hundred rows would
/// make a preview heavier than the pictures it is previewing, so an oversized
/// column is named instead of sent.
const MAX_COLUMN_BYTES: usize = 4096;

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

/// What to list inside one hub dataset.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct HubRowsQuery {
    /// `owner/name`, exactly as a search result addresses it.
    pub dataset: String,
    /// Which hub holds it. Only Hugging Face serves rows; see [`Hubs::images`].
    #[serde(default)]
    pub hub: Option<HubKind>,
    /// The dataset's configuration and split. Both are discovered when absent,
    /// which is the common case — a corpus published as one `train` split has
    /// names nobody should have to look up in order to see it.
    #[serde(default)]
    pub config: Option<String>,
    #[serde(default)]
    pub split: Option<String>,
    #[serde(default)]
    pub offset: Option<u64>,
    #[serde(default)]
    pub limit: Option<usize>,
    /// Columns to hand over as an address rather than as a value,
    /// comma-separated.
    ///
    /// This is where "which column is bytes" is decided, and the caller
    /// decides it. A hub declares a column `binary` and that is the *hub's*
    /// word for a byte string; whether those bytes are a picture, a PDF or an
    /// OCR dump is a question about the corpus, and answering it here would be
    /// answering it for every corpus. A script that names nothing gets every
    /// column as it came.
    ///
    /// What the substitution is for is size, not meaning: a column of base64
    /// pictures is megabytes per page, and an address is resolved by
    /// [`Hubs::cell`] only when somebody actually wants the bytes.
    #[serde(default)]
    pub address: Option<String>,
}

impl HubRowsQuery {
    fn limit(&self) -> usize {
        self.limit.unwrap_or(25).clamp(1, MAX_IMAGES)
    }

    /// The columns the caller asked to have as addresses.
    fn addressed(&self) -> BTreeSet<&str> {
        self.address
            .as_deref()
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .collect()
    }
}

/// One cell of one row, as an address rather than as bytes.
///
/// What a row carries when the hub has no URL for its picture — a `binary`
/// column is bytes in the response and nothing else. Small enough to sit in a
/// query result, resolvable by [`Hubs::cell`], and stable in a way a signed
/// asset URL is not.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct HubCellQuery {
    pub dataset: String,
    #[serde(default)]
    pub hub: Option<HubKind>,
    pub config: String,
    pub split: String,
    /// Which row of the split, counting from zero.
    pub row: u64,
    /// Which column of it.
    pub column: String,
}

/// One column a hub dataset declares, in the hub's own words.
///
/// Carried verbatim and not interpreted. aiwatcher does not know which column
/// of somebody else's corpus is the picture, what the caption is called, or
/// whether `indices` is an id or a label — and a route that decided would be
/// deciding it for every corpus. This is what a script is written against.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct HubColumn {
    pub name: String,
    /// The hub's type tag: `Image`, `Value`, `Sequence`, and so on.
    pub kind: String,
    /// The dtype a `Value` carries: `string`, `binary`, `int32`, …
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dtype: String,
}

/// One row of a hub dataset, as the hub sent it.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct HubRow {
    /// Where it sat in the split. The only name a row has that the corpus did
    /// not choose, and the one a family key is usually built from.
    pub row_index: u64,
    /// Every column, under the corpus's own name for it.
    ///
    /// Two substitutions, and both are about size rather than meaning: a cell
    /// the hub sent as bytes becomes a path back into this process that
    /// resolves them (see [`Hubs::cell`]), because a hundred rows of base64 is
    /// a result heavier than the pictures it describes; and a cell too large
    /// to carry is left out and named in [`omitted`](Self::omitted).
    #[schema(value_type = Object)]
    pub row: BTreeMap<String, Value>,
    /// Columns left out, and why they would have been.
    ///
    /// Named, because a column that is simply absent reads as a column the
    /// corpus does not have.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub omitted: Vec<String>,
}

/// One page of a hub dataset's rows.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct HubRowsPage {
    pub hub: HubKind,
    pub dataset: String,
    /// Resolved, never echoed: what was actually read, which is what makes the
    /// same call repeatable after a dataset gains a second split.
    pub config: String,
    pub split: String,
    /// What the corpus declares it holds. A script is written from this.
    pub columns: Vec<HubColumn>,
    pub rows: Vec<HubRow>,
    /// The split's own count, when the hub reports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_rows: Option<u64>,
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

    /// The rows inside one hub dataset, as the hub sent them.
    ///
    /// A search answers "which corpora exist"; this answers "what is in this
    /// one". Nothing here decides which column is a picture, what a caption is
    /// called, or what a family key should be built from — those are questions
    /// about somebody else's corpus, and a route that answered them would be
    /// answering them for every corpus. It reports the columns the dataset
    /// declares and hands over the rows; the script does the rest.
    ///
    /// Two substitutions, both about size and neither about meaning. A cell
    /// the hub sent as bytes becomes an address [`Self::cell`] resolves, so a
    /// hundred rows of base64 do not travel through the query service and the
    /// browser to be looked at once. A cell too large to carry, or one the hub
    /// itself shortened, is left out and named.
    ///
    /// Hugging Face only. Kaggle publishes archives rather than rows: seeing
    /// inside one means downloading and unpacking it, which is a different
    /// operation with a different cost, and pretending otherwise here would
    /// mean a route that silently takes minutes on one hub and milliseconds on
    /// the other.
    ///
    /// # Errors
    /// When the hub is not configured, does not serve rows, or did not answer.
    pub async fn rows(&self, query: &HubRowsQuery) -> Result<HubRowsPage, String> {
        if query.hub == Some(HubKind::Kaggle) {
            return Err(
                "Kaggle serves archives rather than rows; open the dataset and import \
                        the files, or search Hugging Face for a mirror"
                    .to_owned(),
            );
        }
        if !self.config.huggingface {
            return Err(format!(
                "Hugging Face is not configured; set {}",
                HubKind::HuggingFace.variable()
            ));
        }
        let dataset = query.dataset.trim();
        if dataset.is_empty() {
            return Err("name the dataset to read, as `owner/name`".to_owned());
        }

        let (config, split) = match (query.config.clone(), query.split.clone()) {
            (Some(config), Some(split)) => (config, split),
            _ => self.huggingface_split(dataset, query).await?,
        };

        let body = send(
            self.request("https://datasets-server.huggingface.co/rows")
                .query(&[("dataset", dataset), ("config", &config), ("split", &split)])
                .query(&[
                    ("offset", query.offset.unwrap_or(0).to_string()),
                    ("length", query.limit().to_string()),
                ]),
            HubKind::HuggingFace,
        )
        .await?;

        let columns: Vec<HubColumn> = body
            .get("features")
            .and_then(Value::as_array)
            .map(|features| {
                features
                    .iter()
                    .filter_map(|feature| {
                        let name = feature.get("name").and_then(Value::as_str)?;
                        let kind = feature.get("type");
                        Some(HubColumn {
                            name: name.to_owned(),
                            kind: kind
                                .and_then(|kind| kind.get("_type"))
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            dtype: kind
                                .and_then(|kind| kind.get("dtype"))
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut page = HubRowsPage {
            hub: HubKind::HuggingFace,
            dataset: dataset.to_owned(),
            config,
            split,
            columns,
            total_rows: number(&body, "num_rows_total"),
            rows: Vec::new(),
        };

        let addressed = query.addressed();

        for entry in body
            .get("rows")
            .and_then(Value::as_array)
            .unwrap_or(&vec![])
        {
            let row_index = number(entry, "row_idx").unwrap_or_default();
            let Some(cells) = entry.get("row").and_then(Value::as_object) else {
                continue;
            };
            // A cell the hub shortened is not the value. Carrying the fragment
            // would be handing a script half a picture under a whole name.
            let shortened: Vec<&str> = entry
                .get("truncated_cells")
                .and_then(Value::as_array)
                .map(|names| names.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();

            let mut carried = BTreeMap::new();
            let mut omitted = Vec::new();
            for (name, value) in cells {
                if shortened.contains(&name.as_str()) {
                    omitted.push(name.clone());
                    continue;
                }
                // A column the caller named comes back as an address. Which
                // ones those are is not decided here — see
                // [`HubRowsQuery::address`].
                if addressed.contains(name.as_str()) {
                    carried.insert(
                        name.clone(),
                        Value::from(cell_address(&HubCellQuery {
                            dataset: dataset.to_owned(),
                            hub: Some(HubKind::HuggingFace),
                            config: page.config.clone(),
                            split: page.split.clone(),
                            row: row_index,
                            column: name.clone(),
                        })),
                    );
                    continue;
                }
                if value.to_string().len() > MAX_COLUMN_BYTES {
                    omitted.push(name.clone());
                    continue;
                }
                carried.insert(name.clone(), value.clone());
            }

            page.rows.push(HubRow {
                row_index,
                row: carried,
                omitted,
            });
        }
        Ok(page)
    }

    /// The bytes of one cell, whichever shape it is in.
    ///
    /// The other half of [`HubImage::uri`]: a row that named a cell rather
    /// than a URL is resolved here, by re-reading that single row. One row
    /// rather than the page it came from, deliberately — the hub shortens a
    /// response that grows too large and names the casualties, and a request
    /// for one cell is the one least likely to be shortened.
    ///
    /// # Errors
    /// When the hub is not configured, the cell is not there, the hub
    /// shortened it, or it is not a picture.
    pub async fn cell(&self, query: &HubCellQuery) -> Result<(Vec<u8>, String), String> {
        if !self.config.huggingface {
            return Err(format!(
                "Hugging Face is not configured; set {}",
                HubKind::HuggingFace.variable()
            ));
        }
        let body = send(
            self.request("https://datasets-server.huggingface.co/rows")
                .query(&[
                    ("dataset", query.dataset.as_str()),
                    ("config", query.config.as_str()),
                    ("split", query.split.as_str()),
                ])
                .query(&[
                    ("offset", query.row.to_string()),
                    ("length", "1".to_owned()),
                ]),
            HubKind::HuggingFace,
        )
        .await?;

        let entry = body
            .get("rows")
            .and_then(Value::as_array)
            .and_then(|rows| rows.first())
            .ok_or_else(|| format!("row {} is past the end of the split", query.row))?;

        if entry
            .get("truncated_cells")
            .and_then(Value::as_array)
            .is_some_and(|cells| {
                cells
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|name| name == query.column)
            })
        {
            return Err(format!(
                "the hub shortened {} on row {}; the whole picture is not available through the \
                 rows endpoint",
                query.column, query.row
            ));
        }

        let cell = entry
            .get("row")
            .and_then(|row| row.get(&query.column))
            .ok_or_else(|| format!("row {} has no column {}", query.row, query.column))?;

        // A typed image cell is a URL to follow; the allowlist in `fetch` is
        // what keeps that from being an address a caller chose.
        let src = string(cell, "src");
        if !src.is_empty() {
            let (bytes, header) = self.fetch(&src).await?;
            // The bytes outrank the header. Hugging Face serves its cached
            // assets as `binary/octet-stream`, and a browser handed that for
            // something it is about to draw is being told less than the first
            // four bytes already say.
            let content_type =
                pixels::describe(&bytes).map_or(header, |found| found.content_type.to_owned());
            return Ok((bytes, content_type));
        }

        let bytes = inline_bytes(cell)
            .ok_or_else(|| format!("{} on row {} is not bytes", query.column, query.row))?;
        let found = pixels::describe(&bytes).ok_or_else(|| {
            format!(
                "{} on row {} is not a picture this can read",
                query.column, query.row
            )
        })?;
        Ok((bytes, found.content_type.to_owned()))
    }

    /// The dataset's first configuration and split.
    ///
    /// Asked rather than assumed. `default`/`train` is the common shape and is
    /// not the only one, and a wrong guess reaches the caller as somebody
    /// else's 404 about a split they never named.
    async fn huggingface_split(
        &self,
        dataset: &str,
        query: &HubRowsQuery,
    ) -> Result<(String, String), String> {
        let body = send(
            self.request("https://datasets-server.huggingface.co/splits")
                .query(&[("dataset", dataset)]),
            HubKind::HuggingFace,
        )
        .await?;

        body.get("splits")
            .and_then(Value::as_array)
            .and_then(|splits| {
                splits
                    .iter()
                    .find(|split| {
                        query
                            .split
                            .as_deref()
                            .is_none_or(|wanted| string(split, "split") == wanted)
                            && query
                                .config
                                .as_deref()
                                .is_none_or(|wanted| string(split, "config") == wanted)
                    })
                    .map(|split| (string(split, "config"), string(split, "split")))
            })
            .ok_or_else(|| format!("{dataset} has no split matching that request"))
    }

    /// The bytes behind one hub image.
    ///
    /// Restricted to the hosts this module hands out, and that restriction is
    /// the point rather than a detail. The alternative — fetching whatever URI
    /// a caller put in an import row — is a request-forgery primitive: this
    /// process runs inside a cluster, so "download this address for me" is a
    /// request to reach the cluster's own network on the caller's behalf. Same
    /// rule as the rerun target, which may only come from configuration.
    ///
    /// # Errors
    /// When the host is not a hub's, or the download failed.
    pub async fn fetch(&self, uri: &str) -> Result<(Vec<u8>, String), String> {
        if !is_hub_asset(uri) {
            return Err(format!(
                "{uri} is not a Hugging Face address; only a hub's own asset host may be fetched"
            ));
        }
        let response = self
            .request(uri)
            .timeout(FETCH_TIMEOUT)
            .send()
            .await
            .map_err(|error| format!("the image did not download: {error}"))?;

        let status = response.status();
        if !status.is_success() {
            // An expired signature is the failure worth naming: these URLs
            // last hours, so a batch previewed yesterday and imported today
            // fails here rather than anywhere that would explain itself.
            return Err(format!(
                "the image answered {status}; a hub asset URL expires within hours of being listed"
            ));
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_owned();
        let bytes = response
            .bytes()
            .await
            .map_err(|error| format!("the image did not download: {error}"))?;
        Ok((bytes.to_vec(), content_type))
    }

    fn request(&self, url: &str) -> reqwest::RequestBuilder {
        let request = self.http.get(url);
        match self.config.huggingface_token.as_deref() {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
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

/// The bytes of a cell the hub sent inline.
///
/// Two spellings, both base64: a plain `binary` column is the string itself,
/// and an image feature the hub could not serve as an asset arrives as
/// `{"bytes": …, "path": …}`.
fn inline_bytes(cell: &Value) -> Option<Vec<u8>> {
    use base64::Engine as _;

    let encoded = cell
        .as_str()
        .or_else(|| cell.get("bytes").and_then(Value::as_str))?;
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()
}

/// A path back into this process naming one cell.
///
/// Built with a URL type rather than by formatting, because a dataset id, a
/// config and a column name are all somebody else's strings: `owner/name`
/// alone would end the query parameter and start a path segment.
fn cell_address(query: &HubCellQuery) -> String {
    let Ok(url) = reqwest::Url::parse_with_params(
        "https://aiwatcher.invalid/api/v1/dataset-hubs/image",
        [
            ("dataset", query.dataset.as_str()),
            ("config", query.config.as_str()),
            ("split", query.split.as_str()),
            ("row", &query.row.to_string()),
            ("column", query.column.as_str()),
        ],
    ) else {
        return String::new();
    };
    match url.query() {
        Some(parameters) => format!("{}?{parameters}", url.path()),
        None => url.path().to_owned(),
    }
}

/// The cell a row's `uri` names, when it names one.
///
/// The other end of [`cell_address`], and the reason an import can resolve
/// bytes without ever following an address a caller chose: what comes back is
/// a parsed query, not a URL to fetch. `None` for anything else, which is how
/// a row carrying an ordinary hub URL keeps taking the allowlisted path.
#[must_use]
pub fn parse_cell_address(uri: &str) -> Option<HubCellQuery> {
    let (path, query) = uri.split_once('?')?;
    if path != "/api/v1/dataset-hubs/image" {
        return None;
    }
    let url = reqwest::Url::parse(&format!("https://aiwatcher.invalid/?{query}")).ok()?;
    let mut found = HubCellQuery {
        hub: Some(HubKind::HuggingFace),
        ..HubCellQuery::default()
    };
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "dataset" => found.dataset = value.into_owned(),
            "config" => found.config = value.into_owned(),
            "split" => found.split = value.into_owned(),
            "column" => found.column = value.into_owned(),
            "row" => found.row = value.parse().ok()?,
            _ => return None,
        }
    }
    if found.dataset.is_empty() || found.split.is_empty() || found.column.is_empty() {
        return None;
    }
    Some(found)
}

/// Whether a URI is one this module could have produced.
///
/// A prefix match on the scheme *and* a suffix match on the host, split on the
/// first `/` after the authority — never a `contains`. `https://evil.test/?x=huggingface.co`
/// contains the host and is not it, and the same mistake in the curated-source
/// matcher is what invented a licence claim out of a coincidence of spelling.
fn is_hub_asset(uri: &str) -> bool {
    let Some(rest) = uri.strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or_default();
    // Credentials in the authority would put anything before an `@`, and the
    // host is what comes after it.
    let host = authority.rsplit('@').next().unwrap_or_default();
    let host = host.split(':').next().unwrap_or_default();
    host == "huggingface.co" || host.ends_with(".huggingface.co")
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

    #[tokio::test]
    async fn kaggle_is_refused_for_rows_rather_than_asked_for_them() {
        let error = hubs()
            .rows(&HubRowsQuery {
                dataset: "someone/floor-plans".to_owned(),
                hub: Some(HubKind::Kaggle),
                ..HubRowsQuery::default()
            })
            .await
            .expect_err("Kaggle serves archives, not rows");

        assert!(error.contains("archives"), "{error}");
    }

    #[tokio::test]
    async fn reading_rows_needs_hugging_face_configured() {
        let error = Hubs::new(HubConfig::default())
            .expect("the client builds")
            .rows(&HubRowsQuery {
                dataset: "someone/floor-plans".to_owned(),
                ..HubRowsQuery::default()
            })
            .await
            .expect_err("nothing is configured");

        assert!(error.contains("AIWATCHER_HUGGINGFACE_ENABLED"), "{error}");
    }

    /// Which columns come back as addresses is the caller's list and nothing
    /// else. A hub declaring a column `binary` says the cell is a byte string;
    /// whether those bytes are a picture, a PDF or an OCR dump is a question
    /// about the corpus, and this crate does not answer it.
    #[test]
    fn only_the_columns_a_caller_named_are_addressed() {
        let named = HubRowsQuery {
            address: Some(" tif , image_content ,, ".to_owned()),
            ..HubRowsQuery::default()
        };
        let addressed = named.addressed();
        assert!(addressed.contains("tif"));
        assert!(addressed.contains("image_content"));
        // Whitespace and empty entries are not column names.
        assert!(!addressed.contains(""));
        assert_eq!(addressed.len(), 2);

        // A `binary` column nobody named stays a value: the default is to hand
        // the corpus over as it came.
        assert!(!addressed.contains("pdf"));
        assert!(HubRowsQuery::default().addressed().is_empty());
    }

    /// A cell address has to survive the round trip through a query result and
    /// an import row, with the punctuation a dataset id and a column name are
    /// free to contain.
    #[test]
    fn a_cell_address_parses_back_into_the_cell_it_named() {
        let query = HubCellQuery {
            dataset: "pixparse/idl-wds".to_owned(),
            hub: Some(HubKind::HuggingFace),
            config: "default".to_owned(),
            split: "train".to_owned(),
            row: 41,
            column: "image_content".to_owned(),
        };
        let address = cell_address(&query);
        assert!(
            address.starts_with("/api/v1/dataset-hubs/image?"),
            "{address}"
        );
        // The slash in the dataset id would otherwise start a path segment.
        assert!(address.contains("pixparse%2Fidl-wds"), "{address}");

        let parsed = parse_cell_address(&address).expect("it parses back");
        assert_eq!(parsed.dataset, query.dataset);
        assert_eq!(parsed.row, 41);
        assert_eq!(parsed.column, "image_content");

        // Anything else is not a cell address, and must keep taking the
        // allowlisted path rather than being resolved as one.
        assert!(parse_cell_address("https://huggingface.co/a.png").is_none());
        assert!(parse_cell_address("/api/v1/dataset-hubs/image?dataset=x").is_none());
        assert!(
            parse_cell_address("/api/v1/annotation-blobs/x?dataset=a&split=b&column=c&row=1")
                .is_none()
        );
    }

    /// The whole point of the allowlist: an import row is caller-supplied, and
    /// this process sits inside a cluster.
    #[tokio::test]
    async fn only_a_hubs_own_host_may_be_downloaded() {
        let hubs = hubs();
        for uri in [
            "http://169.254.169.254/latest/meta-data/",
            "https://aiwatcher.internal/api/v1/events",
            // Contains the host and is not it.
            "https://evil.test/x?next=huggingface.co",
            "https://nothuggingface.co/a.png",
            // Credentials in the authority do not move the host.
            "https://huggingface.co@evil.test/a.png",
        ] {
            let error = hubs.fetch(uri).await.expect_err("refused: {uri}");
            assert!(
                error.contains("not a Hugging Face address"),
                "{uri}: {error}"
            );
        }

        assert!(is_hub_asset(
            "https://datasets-server.huggingface.co/cached-assets/x/image.jpg?Expires=1"
        ));
        assert!(is_hub_asset(
            "https://huggingface.co/datasets/x/resolve/main/a.png"
        ));
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
