//! The store: one object per version, one head per name.
//!
//! Key layout, which is the prompt registry's with one word changed:
//!
//! ```text
//! labs/<name>/head.json
//! labs/<name>/versions/<version_id>.json
//! labs/scopes/<organization>/<project>/registry/<name>/…
//! ```
//!
//! The **version object is written before the head that indexes it**
//! (`aiwatcher_jobs::ORDERING`). A crash the right way round leaves an object
//! nothing lists, which a republish of the same document resolves because
//! publishing is idempotent; the wrong way round leaves a head whose rows 404.

use std::sync::Arc;

use aiwatcher_core::ObjectStore;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::{
    LABS_PAGE_MAX, Lab, LabError, LabHead, LabName, LabVersion, LabVersionSummary, Result,
    lab::PUBLISHED_LABEL,
};

/// What the registry is allowed to hold.
#[derive(Clone, Debug)]
pub struct RegistryConfig {
    /// Key prefix inside the bucket, so one store holds this beside the five
    /// registries already in it.
    pub prefix: String,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self {
            prefix: "labs".to_owned(),
        }
    }
}

/// Publish a version of a lab, and optionally move a label onto it.
#[derive(Clone, Debug, Deserialize, utoipa::ToSchema)]
#[schema(as = LabPublishRequest)]
#[serde(deny_unknown_fields)]
pub struct PublishLab {
    #[serde(flatten)]
    pub lab: Lab,
    /// Why this version exists.
    #[serde(default)]
    pub notes: Option<String>,
    /// Move this label onto the new version — `published` to make it the one
    /// participants read. Absent publishes a draft that everything can read
    /// and nothing is using, which is separate from publishing for the same
    /// reason it is in the prompt registry: writing a lesson and setting it are
    /// different decisions.
    #[serde(default)]
    pub label: Option<String>,
}

/// What a publish did.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
#[schema(as = LabPublished)]
pub struct LabPublished {
    pub version: LabVersion,
    /// `false` when this exact document was already stored. Publishing is
    /// content-addressed, so re-sending an unchanged lab is not a new version.
    pub created: bool,
    pub head: LabHead,
}

/// One lab as a list shows it: its name and what it is at now.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
#[schema(as = LabSummary)]
pub struct LabSummary {
    pub name: LabName,
    /// What `published` points at, or the newest version when nothing is
    /// labelled. Absent only for a head with no versions at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<LabVersionSummary>,
    /// Whether a version carries the `published` label — the difference
    /// between a lab a participant is meant to read and a draft.
    pub is_published: bool,
    pub versions: usize,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Default)]
pub struct LabFilter {
    /// Page by name: a lab published between two requests must not shift the
    /// page under a reader.
    pub after: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
#[schema(as = LabPage)]
pub struct LabPage {
    /// By position, then by name — the order a workshop's slots are read in.
    pub labs: Vec<LabSummary>,
    pub total: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Registry {
    pub(crate) store: Arc<dyn ObjectStore>,
    pub(crate) config: RegistryConfig,
    pub(crate) scope: Option<aiwatcher_iam::ProjectScope>,
}

impl Registry {
    #[must_use]
    pub fn new(store: Arc<dyn ObjectStore>, config: RegistryConfig) -> Self {
        Self {
            store,
            config,
            scope: None,
        }
    }

    #[must_use]
    pub fn config(&self) -> &RegistryConfig {
        &self.config
    }

    fn head_key(&self, name: &LabName) -> String {
        format!("{}/{name}/head.json", self.config.prefix)
    }

    fn version_key(&self, name: &LabName, version_id: &str) -> String {
        format!("{}/{name}/versions/{version_id}.json", self.config.prefix)
    }

    /// One lab's head, or `None` when nothing has been published under the
    /// name.
    ///
    /// # Errors
    ///
    /// [`LabError::Store`] when the store is unreachable, [`LabError::Corrupt`]
    /// when the head is not readable JSON.
    pub async fn head(&self, name: &LabName) -> Result<Option<LabHead>> {
        let key = self.head_key(name);
        self.read_json(&key).await
    }

    /// One version, with its brief.
    ///
    /// # Errors
    ///
    /// As [`Registry::head`].
    pub async fn version(&self, name: &LabName, version_id: &str) -> Result<Option<LabVersion>> {
        let key = self.version_key(name, &validate_version_id(version_id)?);
        self.read_json(&key).await
    }

    /// The version a label points at, or the current one when no label is
    /// named. One request, because reading a lab in order to show it should
    /// not be two.
    ///
    /// # Errors
    ///
    /// [`LabError::UnknownLab`] for a name nothing was published under or a
    /// label pointing nowhere, [`LabError::UnknownVersion`] when the label
    /// points at a version the store no longer has.
    pub async fn resolve(&self, name: &LabName, label: Option<&str>) -> Result<LabVersion> {
        let head = self
            .head(name)
            .await?
            .ok_or_else(|| LabError::UnknownLab(name.clone()))?;
        let version_id = match label {
            Some(label) => head.labels.get(label).map(String::as_str),
            None => head.current(),
        }
        .ok_or_else(|| LabError::UnknownLab(name.clone()))?
        .to_owned();
        self.version(name, &version_id)
            .await?
            .ok_or(LabError::UnknownVersion {
                name: name.clone(),
                version: version_id,
            })
    }

    /// Every lab, paged.
    ///
    /// # Errors
    ///
    /// [`LabError::Store`] when the store is unreachable. A head that does not
    /// parse is skipped with a warning rather than failing the whole list: one
    /// unreadable object should not take a workshop's page down with it.
    pub async fn list(&self, filter: &LabFilter) -> Result<LabPage> {
        let prefix = format!("{}/", self.config.prefix);
        let mut names: Vec<LabName> = self
            .store
            .list(&prefix)
            .await?
            .into_iter()
            .filter_map(|entry| {
                let rest = entry.key.strip_prefix(&prefix)?;
                LabName::parse(rest.strip_suffix("/head.json")?).ok()
            })
            .collect();
        names.sort();
        names.dedup();
        let total = names.len();

        let mut summaries = Vec::with_capacity(names.len());
        for name in names {
            match self.head(&name).await {
                Ok(Some(head)) => summaries.push(LabSummary {
                    current: head.current_summary().cloned(),
                    is_published: head.labels.contains_key(PUBLISHED_LABEL),
                    versions: head.versions.len(),
                    updated_at: head.updated_at,
                    name: head.name,
                }),
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(%name, %error, "skipping an unreadable lab head");
                }
            }
        }
        // By position first, because that is the order a workshop is worked
        // through; a lab nobody has placed sorts after the placed ones rather
        // than at the front, where an unset field would put it.
        summaries.sort_by(|left, right| {
            let key = |summary: &LabSummary| {
                (
                    summary
                        .current
                        .as_ref()
                        .and_then(|version| version.position)
                        .unwrap_or(u16::MAX),
                    summary.name.clone(),
                )
            };
            key(left).cmp(&key(right))
        });

        if let Some(after) = &filter.after {
            let position = summaries
                .iter()
                .position(|summary| summary.name.as_str() == after.as_str());
            match position {
                Some(index) => summaries.drain(..=index).for_each(drop),
                None => summaries.clear(),
            }
        }
        let limit = filter
            .limit
            .unwrap_or(LABS_PAGE_MAX)
            .clamp(1, LABS_PAGE_MAX);
        let next_cursor = (summaries.len() > limit)
            .then(|| summaries.get(limit - 1).map(|s| s.name.to_string()))
            .flatten();
        summaries.truncate(limit);
        Ok(LabPage {
            labs: summaries,
            total,
            next_cursor,
        })
    }

    /// Publish a version, writing the object before the head that indexes it.
    ///
    /// # Errors
    ///
    /// [`LabError::Invalid`] for a document that does not validate,
    /// [`LabError::Store`] when the store is unreachable.
    pub async fn publish(
        &self,
        request: PublishLab,
        author: Option<String>,
    ) -> Result<LabPublished> {
        request.lab.validate()?;
        let name = request.lab.name.clone();
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let version_id = request.lab.version_id()?;
        let key = self.version_key(&name, &version_id);

        let existing: Option<LabVersion> = self.read_json(&key).await?;
        let (version, created) = match existing {
            Some(version) => (version, false),
            None => {
                let version = LabVersion {
                    version_id,
                    lab: request.lab,
                    notes: request.notes,
                    author,
                    published_at: now,
                };
                self.write_json(&key, &version).await?;
                (version, true)
            }
        };

        let mut head = self
            .head(&name)
            .await?
            .unwrap_or_else(|| LabHead::new(name.clone(), now));
        if let Some(label) = request.label {
            crate::text(&label, "label")?;
            crate::require(
                label.chars().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-')
                }),
                "label",
                "is lowercase letters, digits, dot, underscore and hyphen",
            )?;
            head.labels.insert(label, version.version_id.clone());
        }
        head.index(&version, now);
        self.write_json(&self.head_key(&name), &head).await?;

        Ok(LabPublished {
            version,
            created,
            head,
        })
    }

    /// Move a label onto a version that is already stored.
    ///
    /// # Errors
    ///
    /// [`LabError::UnknownLab`] for a name nothing was published under,
    /// [`LabError::UnknownVersion`] for a version this registry does not hold —
    /// a label is a pointer, and one pointing at nothing is a list whose rows
    /// 404.
    pub async fn set_label(
        &self,
        name: &LabName,
        label: &str,
        version_id: &str,
    ) -> Result<LabHead> {
        crate::text(label, "label")?;
        let version_id = validate_version_id(version_id)?;
        let mut head = self
            .head(name)
            .await?
            .ok_or_else(|| LabError::UnknownLab(name.clone()))?;
        if self.version(name, &version_id).await?.is_none() {
            return Err(LabError::UnknownVersion {
                name: name.clone(),
                version: version_id,
            });
        }
        head.labels.insert(label.to_owned(), version_id);
        head.updated_at = OffsetDateTime::now_utc().unix_timestamp();
        self.write_json(&self.head_key(name), &head).await?;
        Ok(head)
    }

    pub(crate) async fn read_json<T: serde::de::DeserializeOwned>(
        &self,
        key: &str,
    ) -> Result<Option<T>> {
        self.check_key(key)?;
        let Some(bytes) = self.store.get(key).await? else {
            return Ok(None);
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|source| LabError::Corrupt {
                key: key.to_owned(),
                source,
            })
    }

    pub(crate) async fn write_json<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        self.check_key(key)?;
        let body = serde_json::to_vec_pretty(value)?;
        self.store.put(key, body).await?;
        Ok(())
    }
}

/// A version id is a digest this registry wrote, never text a caller composes:
/// anything else is a path segment on its way into a key.
fn validate_version_id(value: &str) -> Result<String> {
    crate::require(
        value.len() == 64 && value.chars().all(|c| c.is_ascii_hexdigit()),
        "version_id",
        "is a lab version's digest: 64 hex characters",
    )?;
    Ok(value.to_owned())
}
