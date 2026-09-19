//! The store: one object per rule version, one head per name, one object per
//! delivery — and that last one is the whole dedup mechanism.
//!
//! ```text
//! alerts/rules/<name>/head.json
//! alerts/rules/<name>/versions/<version_id>.json
//! alerts/deliveries/<dedup_key>.json
//! ```
//!
//! A rule publishes the **version object before the head that indexes it**
//! ([`aiwatcher_jobs::ORDERING`]): a crash the right way round leaves an
//! object nothing lists, which a republish resolves because publishing is
//! content-addressed; the wrong way round leaves a head whose rows 404.
//!
//! A delivery is created with [`ObjectStore::create`], which is atomic
//! create-if-absent. Two watchers that saw the same failure race, one wins,
//! and the loser is told `created: false` — so a redelivered occurrence is not
//! a second notification, with no lock and nothing to expire. It is the one
//! place here that must not be a read followed by a write.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use aiwatcher_core::ObjectStore;
use aiwatcher_jobs::JobState;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::delivery::{AlertPayload, Delivery};
use crate::rule::{AlertRule, RuleHead, RuleName, RuleVersion, RuleVersionSummary};
use crate::signal::{AlertSignal, dedup_key};
use crate::{ALERTS_PAGE_MAX, AlertError, Result};

/// What the registry is allowed to hold.
#[derive(Clone, Debug)]
pub struct RegistryConfig {
    /// Key prefix inside the bucket, so one store holds this beside the
    /// registries already in it.
    pub prefix: String,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self {
            prefix: "alerts".to_owned(),
        }
    }
}

/// Publish a version of a rule.
#[derive(Clone, Debug, Deserialize, utoipa::ToSchema)]
#[schema(as = AlertRulePublishRequest)]
#[serde(deny_unknown_fields)]
pub struct PublishRule {
    pub rule: AlertRule,
    /// Why this version exists.
    #[serde(default)]
    pub notes: Option<String>,
}

/// What a publish did.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
#[schema(as = AlertRulePublished)]
pub struct RulePublished {
    pub version: RuleVersion,
    /// `false` when this exact rule was already stored. Publishing is
    /// content-addressed, so re-sending an unchanged rule is not a new version
    /// — and, because the dedup key is built from the version, it is also not
    /// a reason to notify anybody again about what already fired.
    pub created: bool,
    pub head: RuleHead,
}

/// One rule as a list shows it.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
#[schema(as = AlertRuleSummary)]
pub struct RuleSummary {
    pub name: RuleName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<RuleVersionSummary>,
    pub enabled: bool,
    pub versions: usize,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Default)]
pub struct RuleFilter {
    pub after: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
#[schema(as = AlertRulePage)]
pub struct RulePage {
    pub rules: Vec<RuleSummary>,
    pub total: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// A rule that is on, with the version occurrences are matched against.
///
/// Read once per pass rather than per occurrence: a watcher that looked the
/// rules up for every failed execution would turn a burst into a list of the
/// bucket per row.
#[derive(Clone, Debug)]
pub struct ActiveRule {
    pub name: RuleName,
    pub version_id: String,
    pub rule: AlertRule,
}

/// What raising one signal against one rule did.
#[derive(Clone, Debug)]
pub struct Raised {
    pub rule: RuleName,
    pub dedup_key: String,
    /// `false` when this occurrence had already been raised under this rule
    /// version — which is not an error, and is the normal case for anything
    /// that reads its source more than once.
    pub created: bool,
}

#[derive(Clone, Debug, Default)]
pub struct DeliveryFilter {
    /// Only this rule's deliveries.
    pub rule: Option<RuleName>,
    /// Only deliveries in this state.
    pub state: Option<JobState>,
    /// The last key on the previous page.
    pub after: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
#[schema(as = AlertDeliveryPage)]
pub struct DeliveryPage {
    /// Newest first — a history is read from the top.
    pub deliveries: Vec<Delivery>,
    pub total: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Registry {
    store: Arc<dyn ObjectStore>,
    config: RegistryConfig,
    /// Keys this process has already read and found terminal.
    ///
    /// A finished delivery's object is never written again, so reading it on
    /// every pass buys nothing. A restart forgets the set and reads each once
    /// more, which is why this is an optimisation rather than state: being
    /// wrong about it costs a `get`, never a missed notification.
    finished: Arc<Mutex<HashSet<String>>>,
}

impl Registry {
    #[must_use]
    pub fn new(store: Arc<dyn ObjectStore>, config: RegistryConfig) -> Self {
        Self {
            store,
            config,
            finished: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    #[must_use]
    pub fn config(&self) -> &RegistryConfig {
        &self.config
    }

    fn head_key(&self, name: &RuleName) -> String {
        format!("{}/rules/{name}/head.json", self.config.prefix)
    }

    fn version_key(&self, name: &RuleName, version_id: &str) -> String {
        format!(
            "{}/rules/{name}/versions/{version_id}.json",
            self.config.prefix
        )
    }

    fn deliveries_prefix(&self) -> String {
        format!("{}/deliveries/", self.config.prefix)
    }

    fn delivery_key(&self, dedup_key: &str) -> String {
        format!("{}{dedup_key}.json", self.deliveries_prefix())
    }

    fn cursor_key(&self, watcher: &str) -> String {
        format!("{}/watch/{watcher}.json", self.config.prefix)
    }

    // ── Rules ───────────────────────────────────────────────────────────────

    /// One rule's head, or `None` when nothing was published under the name.
    ///
    /// # Errors
    ///
    /// [`AlertError::Store`] when the store is unreachable,
    /// [`AlertError::Corrupt`] when the head is not readable JSON.
    pub async fn head(&self, name: &RuleName) -> Result<Option<RuleHead>> {
        self.read_json(&self.head_key(name)).await
    }

    /// One version of a rule.
    ///
    /// # Errors
    ///
    /// As [`Registry::head`], and [`AlertError::Invalid`] for a version id
    /// that is not a digest this registry wrote.
    pub async fn version(&self, name: &RuleName, version_id: &str) -> Result<Option<RuleVersion>> {
        let key = self.version_key(name, &validate_version_id(version_id)?);
        self.read_json(&key).await
    }

    /// Every rule, paged by name.
    ///
    /// # Errors
    ///
    /// [`AlertError::Store`] when the store is unreachable. A head that does
    /// not parse is skipped with a warning rather than failing the list: one
    /// unreadable object must not take the page down with it.
    pub async fn list(&self, filter: &RuleFilter) -> Result<RulePage> {
        let prefix = format!("{}/rules/", self.config.prefix);
        let mut names: Vec<RuleName> = self
            .store
            .list(&prefix)
            .await?
            .into_iter()
            .filter_map(|entry| {
                let rest = entry.key.strip_prefix(&prefix)?;
                RuleName::parse(rest.strip_suffix("/head.json")?).ok()
            })
            .collect();
        names.sort();
        names.dedup();
        let total = names.len();

        let mut summaries = Vec::with_capacity(names.len());
        for name in names {
            match self.head(&name).await {
                Ok(Some(head)) => summaries.push(RuleSummary {
                    current: head.current_summary().cloned(),
                    enabled: head.enabled,
                    versions: head.versions.len(),
                    updated_at: head.updated_at,
                    name: head.name,
                }),
                Ok(None) => {}
                Err(error) => tracing::warn!(%name, %error, "skipping an unreadable alert rule"),
            }
        }

        if let Some(after) = &filter.after {
            match summaries
                .iter()
                .position(|summary| summary.name.as_str() == after.as_str())
            {
                Some(index) => summaries.drain(..=index).for_each(drop),
                None => summaries.clear(),
            }
        }
        let limit = filter
            .limit
            .unwrap_or(ALERTS_PAGE_MAX)
            .clamp(1, ALERTS_PAGE_MAX);
        let next_cursor = (summaries.len() > limit)
            .then(|| summaries.get(limit - 1).map(|s| s.name.to_string()))
            .flatten();
        summaries.truncate(limit);
        Ok(RulePage {
            rules: summaries,
            total,
            next_cursor,
        })
    }

    /// Publish a version, writing the object before the head that indexes it.
    ///
    /// # Errors
    ///
    /// [`AlertError::Invalid`] for a rule that does not validate,
    /// [`AlertError::Store`] when the store is unreachable.
    pub async fn publish(
        &self,
        request: PublishRule,
        author: Option<String>,
        now: i64,
    ) -> Result<RulePublished> {
        request.rule.validate()?;
        if let Some(notes) = &request.notes {
            crate::text(notes, "notes")?;
        }
        let name = request.rule.name.clone();
        let version_id = request.rule.version_id()?;
        let key = self.version_key(&name, &version_id);

        let existing: Option<RuleVersion> = self.read_json(&key).await?;
        let (version, created) = match existing {
            Some(version) => (version, false),
            None => {
                let version = RuleVersion {
                    version_id,
                    rule: request.rule,
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
            .unwrap_or_else(|| RuleHead::new(name.clone(), now));
        head.index(&version, now);
        self.write_json(&self.head_key(&name), &head).await?;
        Ok(RulePublished {
            version,
            created,
            head,
        })
    }

    /// Switch a rule on or off, without publishing a version.
    ///
    /// # Errors
    ///
    /// [`AlertError::UnknownRule`] for a name nothing was published under.
    pub async fn set_enabled(&self, name: &RuleName, enabled: bool, now: i64) -> Result<RuleHead> {
        let mut head = self
            .head(name)
            .await?
            .ok_or_else(|| AlertError::UnknownRule(name.clone()))?;
        head.enabled = enabled;
        head.updated_at = now;
        self.write_json(&self.head_key(name), &head).await?;
        Ok(head)
    }

    /// Every rule that is on, with the version it is at.
    ///
    /// # Errors
    ///
    /// [`AlertError::Store`] when the store is unreachable. A head pointing at
    /// a version the store no longer holds is skipped with a warning: it can
    /// match nothing, and failing the pass would stop every other rule too.
    pub async fn active(&self) -> Result<Vec<ActiveRule>> {
        let page = self
            .list(&RuleFilter {
                after: None,
                limit: Some(ALERTS_PAGE_MAX),
            })
            .await?;
        let mut active = Vec::new();
        for summary in page.rules.into_iter().filter(|summary| summary.enabled) {
            let Some(current) = summary.current else {
                continue;
            };
            match self.version(&summary.name, &current.version_id).await {
                Ok(Some(version)) => active.push(ActiveRule {
                    name: summary.name,
                    version_id: version.version_id,
                    rule: version.rule,
                }),
                Ok(None) => tracing::warn!(
                    name = %summary.name,
                    version = current.version_id,
                    "an alert rule points at a version this store does not hold"
                ),
                Err(error) => {
                    tracing::warn!(name = %summary.name, %error, "skipping an unreadable rule");
                }
            }
        }
        Ok(active)
    }

    // ── Occurrences ─────────────────────────────────────────────────────────

    /// Create a delivery for every rule this signal matches, once each.
    ///
    /// # Errors
    ///
    /// [`AlertError::Store`] when the store is unreachable. A rule whose
    /// delivery could not be created leaves the others alone: the caller gets
    /// the error and its cursor stays where it was, so the pass is redone and
    /// the deliveries that did land are recognised rather than repeated.
    pub async fn raise(
        &self,
        rules: &[ActiveRule],
        signal: &AlertSignal,
        now: i64,
    ) -> Result<Vec<Raised>> {
        let mut raised = Vec::new();
        for rule in rules {
            if !rule.rule.trigger.matches(signal) {
                continue;
            }
            let key = dedup_key(&rule.version_id, signal);
            let payload = AlertPayload {
                dedup_key: key.clone(),
                rule: rule.name.clone(),
                rule_version: rule.version_id.clone(),
                trigger: signal.trigger,
                subject: signal.subject.clone(),
                title: signal.title.clone(),
                description: rule.rule.description.clone(),
                occurred_at: signal.occurred_at,
                facts: signal.facts.clone(),
                links: signal.links.clone(),
            };
            let delivery = Delivery::new(payload, now);
            let body = serde_json::to_vec_pretty(&delivery)?;
            let created = self.store.create(&self.delivery_key(&key), body).await?;
            raised.push(Raised {
                rule: rule.name.clone(),
                dedup_key: key,
                created,
            });
        }
        Ok(raised)
    }

    /// Where a watcher stopped reading its source.
    ///
    /// Kept here rather than in the workflow store because it belongs to the
    /// same store as the deliveries it produces, and because losing it is
    /// safe: a watcher that reads its source again raises the same
    /// occurrences, and every one of them lands on the delivery that already
    /// exists. The cursor saves work, never correctness.
    ///
    /// # Errors
    ///
    /// [`AlertError::Store`] when the store is unreachable.
    pub async fn cursor(&self, watcher: &str) -> Result<Option<String>> {
        let stored: Option<WatchCursor> = self.read_json(&self.cursor_key(watcher)).await?;
        Ok(stored.map(|cursor| cursor.at))
    }

    /// Move a watcher's cursor — after everything it passed was raised, never
    /// before ([`aiwatcher_jobs::ORDERING`]).
    ///
    /// # Errors
    ///
    /// [`AlertError::Store`] when the store is unreachable.
    pub async fn set_cursor(&self, watcher: &str, at: &str) -> Result<()> {
        self.write_json(
            &self.cursor_key(watcher),
            &WatchCursor { at: at.to_owned() },
        )
        .await
    }

    // ── Deliveries ──────────────────────────────────────────────────────────

    /// The deliveries the drain should try now, oldest first.
    ///
    /// # Errors
    ///
    /// [`AlertError::Store`] when the store is unreachable.
    pub async fn due(&self, limit: usize, now: i64) -> Result<Vec<Delivery>> {
        let mut due: Vec<Delivery> = self
            .load_unfinished()
            .await?
            .into_iter()
            .filter(|delivery| delivery.is_due(now))
            .collect();
        due.sort_by_key(|delivery| (delivery.created_at, delivery.dedup_key.clone()));
        due.truncate(limit);
        Ok(due)
    }

    /// Write back what became of one delivery.
    ///
    /// # Errors
    ///
    /// [`AlertError::Store`] when the store is unreachable.
    pub async fn record(&self, delivery: &Delivery) -> Result<()> {
        self.write_json(&self.delivery_key(&delivery.dedup_key), delivery)
            .await?;
        if delivery.state.is_finished()
            && let Ok(mut finished) = self.finished.lock()
        {
            finished.insert(delivery.dedup_key.clone());
        }
        Ok(())
    }

    /// One delivery, by its key.
    ///
    /// # Errors
    ///
    /// As [`Registry::head`].
    pub async fn delivery(&self, dedup_key: &str) -> Result<Option<Delivery>> {
        let key = self.delivery_key(&validate_version_id(dedup_key)?);
        self.read_json(&key).await
    }

    /// Ask for a failed delivery again.
    ///
    /// # Errors
    ///
    /// [`AlertError::UnknownDelivery`] for a key nothing is stored under, and
    /// [`AlertError::NotRetryable`] for one that did not fail — a queued
    /// delivery is already going to be tried, and re-sending a delivered one
    /// would be this system creating the duplicate it exists to avoid.
    pub async fn retry(&self, dedup_key: &str, now: i64) -> Result<Delivery> {
        let mut delivery = self
            .delivery(dedup_key)
            .await?
            .ok_or_else(|| AlertError::UnknownDelivery(dedup_key.to_owned()))?;
        if delivery.state != JobState::Failed {
            return Err(AlertError::NotRetryable {
                key: dedup_key.to_owned(),
                state: delivery.state.as_str(),
            });
        }
        delivery.requeue(now);
        if let Ok(mut finished) = self.finished.lock() {
            finished.remove(&delivery.dedup_key);
        }
        self.record(&delivery).await?;
        Ok(delivery)
    }

    /// The history, newest first.
    ///
    /// # Errors
    ///
    /// [`AlertError::Store`] when the store is unreachable.
    pub async fn deliveries(&self, filter: &DeliveryFilter) -> Result<DeliveryPage> {
        let mut rows = self.load_all().await?;
        rows.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| left.dedup_key.cmp(&right.dedup_key))
        });
        if let Some(rule) = &filter.rule {
            rows.retain(|delivery| &delivery.rule == rule);
        }
        if let Some(state) = filter.state {
            rows.retain(|delivery| delivery.state == state);
        }
        let total = rows.len();
        if let Some(after) = &filter.after {
            match rows.iter().position(|row| &row.dedup_key == after) {
                Some(index) => rows.drain(..=index).for_each(drop),
                None => rows.clear(),
            }
        }
        let limit = filter
            .limit
            .unwrap_or(ALERTS_PAGE_MAX)
            .clamp(1, ALERTS_PAGE_MAX);
        let next_cursor = (rows.len() > limit)
            .then(|| rows.get(limit - 1).map(|row| row.dedup_key.clone()))
            .flatten();
        rows.truncate(limit);
        Ok(DeliveryPage {
            deliveries: rows,
            total,
            next_cursor,
        })
    }

    /// Forget finished deliveries created before a moment, up to `limit`.
    ///
    /// Only finished ones: a queued delivery older than the window is one the
    /// channel has not taken yet, and deleting it would be this system losing
    /// the notification quietly rather than failing it loudly.
    ///
    /// # Errors
    ///
    /// [`AlertError::Store`] when the store is unreachable.
    pub async fn prune(&self, before: i64, limit: usize) -> Result<usize> {
        let mut pruned = 0;
        for delivery in self.load_all().await? {
            if pruned >= limit {
                break;
            }
            if !delivery.state.is_finished() || delivery.created_at >= before {
                continue;
            }
            self.store
                .delete(&self.delivery_key(&delivery.dedup_key))
                .await?;
            if let Ok(mut finished) = self.finished.lock() {
                finished.remove(&delivery.dedup_key);
            }
            pruned += 1;
        }
        Ok(pruned)
    }

    /// Every stored delivery. The history is bounded by the sweep, so this is
    /// bounded by what a deployment chose to keep.
    async fn load_all(&self) -> Result<Vec<Delivery>> {
        let prefix = self.deliveries_prefix();
        let mut rows = Vec::new();
        for entry in self.store.list(&prefix).await? {
            if !entry.key.ends_with(".json") {
                continue;
            }
            match self.read_json::<Delivery>(&entry.key).await {
                Ok(Some(delivery)) => rows.push(delivery),
                Ok(None) => {}
                Err(error) => tracing::warn!(key = entry.key, %error, "unreadable alert delivery"),
            }
        }
        Ok(rows)
    }

    /// Every delivery this process has not already seen finish.
    async fn load_unfinished(&self) -> Result<Vec<Delivery>> {
        let prefix = self.deliveries_prefix();
        let entries = self.store.list(&prefix).await?;
        let mut rows = Vec::new();
        for entry in entries {
            let Some(key) = entry
                .key
                .strip_prefix(&prefix)
                .and_then(|rest| rest.strip_suffix(".json"))
            else {
                continue;
            };
            if self
                .finished
                .lock()
                .is_ok_and(|finished| finished.contains(key))
            {
                continue;
            }
            match self.read_json::<Delivery>(&entry.key).await {
                Ok(Some(delivery)) => {
                    if delivery.state.is_finished() {
                        if let Ok(mut finished) = self.finished.lock() {
                            finished.insert(delivery.dedup_key.clone());
                        }
                    } else {
                        rows.push(delivery);
                    }
                }
                Ok(None) => {}
                Err(error) => tracing::warn!(key = entry.key, %error, "unreadable alert delivery"),
            }
        }
        Ok(rows)
    }

    async fn read_json<T: serde::de::DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let Some(bytes) = self.store.get(key).await? else {
            return Ok(None);
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|source| AlertError::Corrupt {
                key: key.to_owned(),
                source,
            })
    }

    async fn write_json<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let body = serde_json::to_vec_pretty(value)?;
        self.store.put(key, body).await?;
        Ok(())
    }
}

/// Where one watcher stopped. A struct rather than a bare string so a later
/// watcher can add what it needs without rewriting what is stored.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct WatchCursor {
    at: String,
}

/// What a delivery and a rule version are both named by: a digest this
/// registry wrote. Anything else is a path segment on its way into a key.
fn validate_version_id(value: &str) -> Result<String> {
    crate::require(
        value.len() == 64 && value.chars().all(|c| c.is_ascii_hexdigit()),
        "version_id",
        "is a digest: 64 hex characters",
    )?;
    Ok(value.to_owned())
}

/// The moment a caller with no clock of its own means.
#[must_use]
pub fn now() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}
