//! Closed periods of what variants were observed doing, written down.
//!
//! The read model is bounded by memory and by the log's retention, so what a
//! variant was observed doing last month is gone from it. The projector's
//! period fold ([`crate::period_fold`]) writes each closed period here, at its
//! own width and rolled up into hours and days: one object per variant, and
//! then a marker naming them — the marker is the commit, so a period is read
//! only once every record it names is stored (`aiwatcher_jobs::ORDERING`'s
//! rule). Each object is created once and never rewritten, so a replay, or a
//! second process folding the same log, lands on the first record. A period
//! nothing ended in is not written at all.
//!
//! The fold's own state is kept here too, under `fold/`, one object per
//! position it was saved at: created, never overwritten, the last few kept, and
//! the one furthest along that reads is the one loaded — so two processes
//! sharing a processor ID cannot set each other back, whichever saves last. A
//! marker also says where the fold was when the period closed, so a fold whose
//! every saved state is gone starts again from the last period it wrote rather
//! than from nothing.
//!
//! What the fold could not read is written down under `gaps/` — positions the
//! log no longer held and the span of time they may have lain in, so a window
//! over it says what it may be short of — once `journal/`, the pages a journal
//! of the log kept ([`crate::journal`]), has refilled what it could.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use aiwatcher_core::ports::PortError;
use aiwatcher_core::storage::ObjectStore;

use crate::observations::ObservedPeriod;

/// The prefix this module owns in the deployment's object store.
pub const PREFIX: &str = "variant-observations/";

/// How many saved states of one fold are kept.
const GENERATIONS_KEPT: usize = 3;

/// Where the fold was when it closed a period: enough to start again from
/// there when no saved state is left.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoldAt {
    /// The position of the last event folded when the period closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub through: Option<u64>,
    /// The first period the fold covered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub began: Option<i64>,
    /// The width of its periods from each moment on.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub widths: BTreeMap<i64, i64>,
}

/// Positions a fold expected and the log no longer held: the events between
/// the last one it read and the next one the log gave it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogGap {
    pub first_position: u64,
    pub last_position: u64,
    /// The span of time, in Unix seconds, those events may have lain in: from
    /// the log's clock before them to the clock of the first event after.
    pub from: i64,
    pub until: i64,
}

impl LogGap {
    /// How many events the log no longer held.
    #[must_use]
    pub const fn events(&self) -> u64 {
        self.last_position - self.first_position + 1
    }

    /// Named by all it is, so a gap written twice lands on one key and one
    /// listing answers which reach a window without reading a single object.
    fn key(&self) -> String {
        format!(
            "{PREFIX}gaps/{:012}-{:012}-{:020}-{:020}.json",
            self.until, self.from, self.first_position, self.last_position
        )
    }

    fn from_key(key: &str) -> Option<Self> {
        let mut parts = key
            .strip_prefix(&format!("{PREFIX}gaps/"))?
            .strip_suffix(".json")?
            .splitn(4, '-');
        let until = parts.next()?.parse().ok()?;
        let from = parts.next()?.parse().ok()?;
        let first_position = parts.next()?.parse().ok()?;
        let last_position = parts.next()?.parse().ok()?;
        Some(Self {
            first_position,
            last_position,
            from,
            until,
        })
    }
}

/// Positions a journal page is filed under together: pages are listed a
/// bucket at a time, so a gap is looked up without listing the whole journal.
const JOURNAL_BUCKET: u64 = 1_000_000;

/// Positions of the log a journal read, one after the last, and the events
/// among them the fold reads — each holding only what the fold reads of it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JournalPage {
    /// The first and the last position read; every one between was read too.
    pub first: u64,
    pub last: u64,
    /// The log's clock at the first position, and the latest it reached by
    /// the last, in Unix seconds.
    pub from: i64,
    pub clock: i64,
    pub events: Vec<aiwatcher_core::RecordedEvent>,
}

impl JournalPage {
    fn key(&self) -> String {
        format!(
            "{PREFIX}journal/{:014}/{:020}-{:020}-{:012}.json",
            self.first / JOURNAL_BUCKET,
            self.first,
            self.last,
            self.clock
        )
    }

    /// `(first, last, clock)`, named by the key alone.
    fn named(key: &str) -> Option<(u64, u64, i64)> {
        let (_, name) = key
            .strip_prefix(&format!("{PREFIX}journal/"))?
            .split_once('/')?;
        let mut parts = name.strip_suffix(".json")?.splitn(3, '-');
        Some((
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
        ))
    }
}

/// What says a period was written, and which variants it holds records for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Marker {
    from: i64,
    to: i64,
    /// The global side's variants, spelt as they always were — so a marker
    /// written before projects existed reads back unchanged.
    variants: Vec<String>,
    /// Each project's variants, by that project's key. Absent from every
    /// marker written before this, which reads as no project having written
    /// into the period — which is true.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    scoped: BTreeMap<String, Vec<String>>,
    complete: bool,
    written_at: i64,
    #[serde(default)]
    fold: FoldAt,
}

/// Where closed periods are kept.
#[derive(Clone, Debug)]
pub struct PeriodStore(Arc<dyn ObjectStore>);

fn hex(text: &str) -> String {
    text.bytes().map(|byte| format!("{byte:02x}")).collect()
}

fn folder(level: i64, from: i64) -> String {
    format!("{PREFIX}periods/{level:06}/{from:012}/")
}

/// A variant ID is a producer's text, so its key is its bytes in hex.
///
/// A project's records sit under a `scopes/` segment of their own, which is
/// ADR_0033 pt. 4's rule — the scope is outside the digest and inside the key,
/// so the same declaration in two projects is one version ID and two objects.
/// A global record's key is **byte for byte what it always was**, so nothing
/// written moves and a create-only write still lands on the first record.
fn record_key(
    level: i64,
    from: i64,
    project: Option<aiwatcher_core::ProjectScope>,
    variant_id: &str,
) -> String {
    match project {
        None => format!("{}{}.json", folder(level, from), hex(variant_id)),
        Some(scope) => format!(
            "{}scopes/{}/{}.json",
            folder(level, from),
            scope.key(),
            hex(variant_id)
        ),
    }
}

fn marker_key(level: i64, from: i64) -> String {
    format!("{}period.json", folder(level, from))
}

/// Where a projector's fold keeps its saved states, by its processor ID.
fn fold_folder(processor_id: &str) -> String {
    format!("{PREFIX}fold/{}/", hex(processor_id))
}

fn encoded<T: Serialize>(value: &T) -> Result<Vec<u8>, PortError> {
    serde_json::to_vec(value).map_err(|error| PortError::Rejected {
        target: "variant-observations",
        message: error.to_string(),
    })
}

fn decoded<T: for<'de> Deserialize<'de>>(key: &str, bytes: &[u8]) -> Result<T, PortError> {
    serde_json::from_slice(bytes).map_err(|error| PortError::Rejected {
        target: "variant-observations",
        message: format!("{key}: {error}"),
    })
}

impl PeriodStore {
    #[must_use]
    pub fn new(store: Arc<dyn ObjectStore>) -> Self {
        Self(store)
    }

    /// The state a projector's fold saved furthest along, if it saved one.
    ///
    /// # Errors
    ///
    /// The store's own failure. A state that no longer reads is passed over
    /// for the one saved before it.
    pub async fn load_fold(
        &self,
        processor_id: &str,
    ) -> Result<Option<crate::period_fold::PeriodFold>, PortError> {
        let mut keys: Vec<String> = self
            .0
            .list(&fold_folder(processor_id))
            .await?
            .into_iter()
            .map(|entry| entry.key)
            .filter(|key| key.ends_with(".json"))
            .collect();
        keys.sort_unstable_by(|one, other| other.cmp(one));
        for key in keys {
            let Some(bytes) = self.0.get(&key).await? else {
                continue;
            };
            match decoded(&key, &bytes) {
                Ok(fold) => return Ok(Some(fold)),
                Err(error) => {
                    tracing::warn!(%error, "a saved observation fold does not read; trying the one before it");
                }
            }
        }
        Ok(None)
    }

    /// Save a projector's fold state under the position it was folded
    /// through, then remove all but the few saved last. A state already saved
    /// at that position is the same fold of the same log, and is kept.
    ///
    /// # Errors
    ///
    /// The store's own failure.
    pub async fn save_fold(
        &self,
        processor_id: &str,
        through: u64,
        fold: &crate::period_fold::PeriodFold,
    ) -> Result<(), PortError> {
        let folder = fold_folder(processor_id);
        let key = format!("{folder}{through:020}.json");
        self.0.create(&key, encoded(fold)?).await?;
        let mut saved: Vec<String> = self
            .0
            .list(&folder)
            .await?
            .into_iter()
            .map(|entry| entry.key)
            .filter(|saved| *saved <= key)
            .collect();
        saved.sort_unstable_by(|one, other| other.cmp(one));
        for old in saved.iter().skip(GENERATIONS_KEPT) {
            self.0.delete(old).await?;
        }
        Ok(())
    }

    /// Write one period's records, then its marker. `false` when another
    /// writer's marker was already there.
    ///
    /// # Errors
    ///
    /// The store's own failure; a partly written period is finished by the
    /// next pass, whose objects land on the same keys.
    pub async fn write(
        &self,
        level: i64,
        from: i64,
        records: &[ObservedPeriod],
        now: i64,
        fold: &FoldAt,
    ) -> Result<bool, PortError> {
        for record in records {
            self.0
                .create(
                    &record_key(level, from, record.project, &record.variant_id),
                    encoded(record)?,
                )
                .await?;
        }
        let mut scoped: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for record in records.iter().filter(|record| record.project.is_some()) {
            scoped
                .entry(aiwatcher_core::ProjectScope::key_of(record.project))
                .or_default()
                .push(record.variant_id.clone());
        }
        let marker = Marker {
            from,
            to: from + level,
            variants: records
                .iter()
                .filter(|record| record.project.is_none())
                .map(|record| record.variant_id.clone())
                .collect(),
            scoped,
            complete: records.iter().all(|record| record.complete),
            written_at: now,
            fold: fold.clone(),
        };
        self.0
            .create(&marker_key(level, from), encoded(&marker)?)
            .await
    }

    /// The written period that ends last, and where the fold was when it
    /// closed — what a fold with no saved state starts again from. Days and
    /// hours are listed first, since there are few; their markers name the
    /// widths the fold's own periods had, whose keys are listed next. A fold
    /// that has written neither yet has written few periods, all listed.
    ///
    /// # Errors
    ///
    /// The store's own failure, or a marker that no longer reads.
    pub async fn last_written(&self) -> Result<Option<(i64, i64, FoldAt)>, PortError> {
        let root = format!("{PREFIX}periods/");
        let mut last: Option<(i64, i64, i64)> = None;
        let listed = |keys: Vec<String>, last: &mut Option<(i64, i64, i64)>| {
            for key in keys {
                let Some((level, from)) = key
                    .strip_prefix(&root)
                    .and_then(|rest| rest.strip_suffix("/period.json"))
                    .and_then(|rest| rest.split_once('/'))
                    .and_then(|(level, from)| level.parse::<i64>().ok().zip(from.parse().ok()))
                else {
                    continue;
                };
                let end = from + level;
                if last.is_none_or(|(known, known_level, _)| {
                    end > known || (end == known && level < known_level)
                }) {
                    *last = Some((end, level, from));
                }
            }
        };
        for level in [86_400, 3_600] {
            let keys = self.keys(&format!("{root}{level:06}/")).await?;
            listed(keys, &mut last);
        }
        match last {
            Some((_, level, from)) => {
                let widths = self
                    .marker(level, from)
                    .await?
                    .map(|marker| marker.fold.widths);
                let mut seen = std::collections::BTreeSet::new();
                for width in widths.into_iter().flat_map(|widths| widths.into_values()) {
                    if width < 3_600 && seen.insert(width) {
                        let keys = self.keys(&format!("{root}{width:06}/")).await?;
                        listed(keys, &mut last);
                    }
                }
            }
            None => {
                let keys = self.keys(&root).await?;
                listed(keys, &mut last);
            }
        }
        let Some((_, level, from)) = last else {
            return Ok(None);
        };
        Ok(self
            .marker(level, from)
            .await?
            .map(|marker| (level, from, marker.fold)))
    }

    /// Write down positions the fold found missing.
    ///
    /// # Errors
    ///
    /// The store's own failure.
    pub async fn write_gap(&self, gap: &LogGap) -> Result<(), PortError> {
        self.0.create(&gap.key(), encoded(gap)?).await.map(|_| ())
    }

    /// Every gap written whose span reaches `since` or later, oldest first.
    ///
    /// # Errors
    ///
    /// The store's own failure.
    pub async fn gaps(&self, since: i64) -> Result<Vec<LogGap>, PortError> {
        let mut gaps: Vec<LogGap> = self
            .keys(&format!("{PREFIX}gaps/"))
            .await?
            .iter()
            .filter_map(|key| LogGap::from_key(key))
            .filter(|gap| gap.until >= since)
            .collect();
        gaps.sort_by_key(|gap| (gap.from, gap.first_position));
        Ok(gaps)
    }

    /// Keep a journal page. A page already kept under its key is the same
    /// positions read the same way, and stays.
    ///
    /// # Errors
    ///
    /// The store's own failure.
    pub async fn write_page(&self, page: &JournalPage) -> Result<(), PortError> {
        self.0.create(&page.key(), encoded(page)?).await.map(|_| ())
    }

    /// The journal pages reaching any position from `first` to `last`, in
    /// order of their first — two journals may have paged one stretch twice,
    /// and a reader skips what it already holds.
    ///
    /// # Errors
    ///
    /// The store's own failure, or a page that no longer reads.
    pub async fn pages(&self, first: u64, last: u64) -> Result<Vec<JournalPage>, PortError> {
        let mut named = Vec::new();
        // A page filed a bucket earlier may run on into this one.
        for bucket in (first / JOURNAL_BUCKET).saturating_sub(1)..=last / JOURNAL_BUCKET {
            for key in self.keys(&format!("{PREFIX}journal/{bucket:014}/")).await? {
                if let Some((from, to, _)) = JournalPage::named(&key)
                    && from <= last
                    && to >= first
                {
                    named.push((from, key));
                }
            }
        }
        named.sort();
        let mut pages = Vec::with_capacity(named.len());
        for (_, key) in named {
            if let Some(bytes) = self.0.get(&key).await? {
                pages.push(decoded(&key, &bytes)?);
            }
        }
        Ok(pages)
    }

    /// Remove the pages whose clock reached no later than `before`, and say
    /// how many went.
    ///
    /// # Errors
    ///
    /// The store's own failure.
    pub async fn prune_journal(&self, before: i64) -> Result<usize, PortError> {
        let mut removed = 0;
        for key in self.keys(&format!("{PREFIX}journal/")).await? {
            if JournalPage::named(&key).is_some_and(|(_, _, clock)| clock < before) {
                self.0.delete(&key).await?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    async fn keys(&self, prefix: &str) -> Result<Vec<String>, PortError> {
        Ok(self
            .0
            .list(prefix)
            .await?
            .into_iter()
            .map(|entry| entry.key)
            .collect())
    }

    async fn marker(&self, level: i64, from: i64) -> Result<Option<Marker>, PortError> {
        let key = marker_key(level, from);
        match self.0.get(&key).await? {
            Some(bytes) => decoded(&key, &bytes).map(Some),
            None => Ok(None),
        }
    }

    /// Every record one written period holds, whatever its variant; none when
    /// no such period was written.
    ///
    /// # Errors
    ///
    /// The store's own failure, or a record the marker names that is not
    /// stored or no longer reads.
    pub async fn records(&self, level: i64, from: i64) -> Result<Vec<ObservedPeriod>, PortError> {
        let Some(marker) = self.marker(level, from).await? else {
            return Ok(Vec::new());
        };
        // Every side of the period, not the global one: this is what a rollup
        // adds up, and a rollup that read only the global half would write an
        // hour missing every project's traffic in it.
        let mut sides: Vec<(Option<aiwatcher_core::ProjectScope>, Vec<&str>)> =
            vec![(None, marker.variants.iter().map(String::as_str).collect())];
        for (key, variants) in &marker.scoped {
            let Ok(scope) = aiwatcher_core::ProjectScope::parse(key) else {
                // A key this build cannot read is a period it must not claim to
                // have added up. Named rather than passed over in silence.
                tracing::warn!(
                    scope = key,
                    level,
                    from,
                    "a written period names a scope this build cannot read"
                );
                continue;
            };
            sides.push((Some(scope), variants.iter().map(String::as_str).collect()));
        }
        let mut records = Vec::new();
        for (project, named) in sides {
            records.extend(
                self.period(level, from, project, &named)
                    .await?
                    .unwrap_or_default(),
            );
        }
        Ok(records)
    }

    /// The records one written period holds for these variants, or `None`
    /// when no such period was written.
    ///
    /// # Errors
    ///
    /// The store's own failure, or a record the marker names that is not
    /// stored or no longer reads.
    pub async fn period(
        &self,
        level: i64,
        from: i64,
        project: Option<aiwatcher_core::ProjectScope>,
        variant_ids: &[&str],
    ) -> Result<Option<Vec<ObservedPeriod>>, PortError> {
        let key = marker_key(level, from);
        let Some(bytes) = self.0.get(&key).await? else {
            return Ok(None);
        };
        let marker: Marker = decoded(&key, &bytes)?;
        let held: &[String] = match project {
            None => &marker.variants,
            Some(scope) => marker
                .scoped
                .get(&scope.key())
                .map_or(&[][..], Vec::as_slice),
        };
        let mut records = Vec::new();
        for variant_id in variant_ids {
            if !held.iter().any(|held| held == variant_id) {
                continue;
            }
            let key = record_key(level, from, project, variant_id);
            let bytes = self.0.get(&key).await?.ok_or_else(|| PortError::Rejected {
                target: "variant-observations",
                message: format!("{key} is named by its period and is not stored"),
            })?;
            records.push(decoded(&key, &bytes)?);
        }
        Ok(Some(records))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::BTreeMap;

    use aiwatcher_core::ports::PortResult;
    use aiwatcher_core::storage::ObjectEntry;
    use tokio::sync::RwLock;

    use super::*;

    /// A bucket that is a map, with the create-only write the store needs.
    #[derive(Debug, Default)]
    pub(crate) struct MemoryObjectStore(pub(crate) RwLock<BTreeMap<String, Vec<u8>>>);

    #[async_trait::async_trait]
    impl ObjectStore for MemoryObjectStore {
        async fn put(&self, key: &str, body: Vec<u8>) -> PortResult<()> {
            self.0.write().await.insert(key.to_owned(), body);
            Ok(())
        }
        async fn create(&self, key: &str, body: Vec<u8>) -> PortResult<bool> {
            let mut objects = self.0.write().await;
            if objects.contains_key(key) {
                return Ok(false);
            }
            objects.insert(key.to_owned(), body);
            Ok(true)
        }
        async fn get(&self, key: &str) -> PortResult<Option<Vec<u8>>> {
            Ok(self.0.read().await.get(key).cloned())
        }
        async fn list(&self, prefix: &str) -> PortResult<Vec<ObjectEntry>> {
            Ok(self
                .0
                .read()
                .await
                .iter()
                .filter(|(key, _)| key.starts_with(prefix))
                .map(|(key, body)| ObjectEntry {
                    key: key.clone(),
                    size: body.len() as u64,
                    last_modified: None,
                })
                .collect())
        }
        async fn delete(&self, key: &str) -> PortResult<()> {
            self.0.write().await.remove(key);
            Ok(())
        }
    }

    fn record(variant_id: &str, from: i64, runs: u64) -> ObservedPeriod {
        ObservedPeriod {
            variant_id: variant_id.to_owned(),
            from,
            to: from + 3_600,
            runs,
            complete: true,
            ..ObservedPeriod::default()
        }
    }

    #[tokio::test]
    async fn a_period_is_written_once_and_read_back_for_the_variants_asked_about() {
        let store = PeriodStore::new(Arc::new(MemoryObjectStore::default()));
        let hour = 1_789_300_800;

        assert!(
            store
                .period(3_600, hour, None, &["v1"])
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .write(3_600, hour, &[record("v1", hour, 4)], 1, &FoldAt::default())
                .await
                .unwrap()
        );
        assert!(
            !store
                .write(3_600, hour, &[record("v1", hour, 9)], 2, &FoldAt::default())
                .await
                .unwrap(),
            "the first writer's period is the one kept"
        );

        let read = store
            .period(3_600, hour, None, &["v1", "v/2"])
            .await
            .unwrap()
            .expect("written");
        assert_eq!(
            read.iter()
                .map(|record| (record.variant_id.as_str(), record.runs))
                .collect::<Vec<_>>(),
            [("v1", 4)],
            "a variant nothing ended for holds no record"
        );
        assert!(
            store
                .period(300, hour, None, &["v1"])
                .await
                .unwrap()
                .is_none(),
            "another level is another period"
        );
    }

    /// One declaration made in two projects has one variant ID, so a record's
    /// key carries the scope — and the global side's key does not move.
    #[tokio::test]
    async fn two_projects_sharing_a_variant_id_write_two_records_and_the_global_key_stays_put() {
        let scope = |last: u8| {
            aiwatcher_core::ProjectScope::new(
                uuid::Uuid::parse_str("0198c0de-0000-7000-8000-00000000000a").expect("uuid"),
                uuid::Uuid::parse_str(&format!("0198c0de-0000-7000-8000-0000000000{last:02x}"))
                    .expect("uuid"),
            )
        };
        let objects = Arc::new(MemoryObjectStore::default());
        let store = PeriodStore::new(Arc::clone(&objects) as Arc<dyn ObjectStore>);
        let hour = 1_789_300_800;
        let scoped = |last: u8, runs: u64| ObservedPeriod {
            project: Some(scope(last)),
            ..record("v1", hour, runs)
        };

        assert!(
            store
                .write(
                    3_600,
                    hour,
                    &[record("v1", hour, 4), scoped(0xaa, 7), scoped(0xbb, 9)],
                    1,
                    &FoldAt::default()
                )
                .await
                .unwrap()
        );

        // Three records under one marker, each answering for its own side.
        for (project, runs) in [(None, 4), (Some(scope(0xaa)), 7), (Some(scope(0xbb)), 9)] {
            let read = store
                .period(3_600, hour, project, &["v1"])
                .await
                .unwrap()
                .expect("written");
            assert_eq!(
                read.iter()
                    .map(|record| (record.project, record.runs))
                    .collect::<Vec<_>>(),
                [(project, runs)],
                "{project:?}"
            );
        }

        // The global record is under exactly the key it was under before
        // projects existed — `hex("v1")` — so a create-only write still lands
        // on the first record and nothing stored moves.
        let keys: Vec<String> = objects.0.read().await.keys().cloned().collect();
        assert!(
            keys.contains(&format!(
                "variant-observations/periods/003600/{hour:012}/{}.json",
                hex("v1")
            )),
            "{keys:?}"
        );
        assert_eq!(
            keys.iter().filter(|key| key.contains("/scopes/")).count(),
            2,
            "and a project's sits under a segment of its own: {keys:?}"
        );

        // A rollup reads every side, not the global half.
        let every = store.records(3_600, hour).await.unwrap();
        assert_eq!(every.iter().map(|record| record.runs).sum::<u64>(), 20);
    }

    #[tokio::test]
    async fn the_period_written_last_says_where_its_fold_was() {
        let store = PeriodStore::new(Arc::new(MemoryObjectStore::default()));
        let hour = 1_789_300_800;
        assert!(store.last_written().await.unwrap().is_none());
        let at = |through: u64| FoldAt {
            through: Some(through),
            began: Some(hour),
            widths: BTreeMap::from([(i64::MIN, 300)]),
        };
        store
            .write(300, hour, &[record("v1", hour, 1)], 1, &at(10))
            .await
            .unwrap();
        assert_eq!(
            store.last_written().await.unwrap().map(|(_, _, fold)| fold),
            Some(at(10)),
            "no hour yet: the periods themselves"
        );
        store
            .write(3_600, hour, &[record("v1", hour, 4)], 2, &at(40))
            .await
            .unwrap();
        store
            .write(
                300,
                hour + 3_300,
                &[record("v1", hour + 3_300, 1)],
                3,
                &at(38),
            )
            .await
            .unwrap();
        let (level, from, fold) = store.last_written().await.unwrap().expect("written");
        assert_eq!(
            (level, from - hour, fold.through),
            (300, 3_300, Some(38)),
            "the hour names the fold's width, whose periods are listed; the last ends with it"
        );
    }

    #[tokio::test]
    async fn a_gap_is_written_once_and_listed_for_the_windows_it_reaches() {
        let store = PeriodStore::new(Arc::new(MemoryObjectStore::default()));
        let gap = LogGap {
            first_position: 41,
            last_position: 140,
            from: 1_789_300_800,
            until: 1_789_304_400,
        };
        store.write_gap(&gap).await.unwrap();
        store.write_gap(&gap).await.unwrap();

        assert_eq!(
            store.gaps(gap.from).await.unwrap(),
            std::slice::from_ref(&gap)
        );
        assert_eq!(
            store.gaps(gap.until).await.unwrap(),
            std::slice::from_ref(&gap)
        );
        assert!(store.gaps(gap.until + 1).await.unwrap().is_empty());
        assert_eq!(gap.events(), 100);
    }

    #[tokio::test]
    async fn journal_pages_are_found_by_the_positions_they_reach_and_pruned_by_their_clock() {
        let store = PeriodStore::new(Arc::new(MemoryObjectStore::default()));
        let page = |first: u64, last: u64, clock: i64| JournalPage {
            first,
            last,
            from: clock - 10,
            clock,
            events: Vec::new(),
        };
        for kept in [
            page(999_990, 1_000_020, 100),
            page(1_000_021, 1_000_500, 200),
            page(1_000_501, 1_000_900, 300),
            page(5, 9, 50),
        ] {
            store.write_page(&kept).await.unwrap();
        }
        store.write_page(&page(5, 9, 50)).await.unwrap();

        let found: Vec<(u64, u64)> = store
            .pages(1_000_010, 1_000_600)
            .await
            .unwrap()
            .iter()
            .map(|page| (page.first, page.last))
            .collect();
        assert_eq!(
            found,
            [
                (999_990, 1_000_020),
                (1_000_021, 1_000_500),
                (1_000_501, 1_000_900)
            ],
            "a page filed in the bucket before runs on into the gap"
        );
        assert_eq!(store.prune_journal(200).await.unwrap(), 2);
        assert_eq!(store.pages(0, 2_000_000).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn the_state_furthest_along_is_loaded_whichever_process_saved_last() {
        let store = PeriodStore::new(Arc::new(MemoryObjectStore::default()));
        let ahead = crate::period_fold::PeriodFold::new(300);
        let behind = crate::period_fold::PeriodFold::new(600);

        store.save_fold("projector", 1_000, &ahead).await.unwrap();
        store.save_fold("projector", 400, &behind).await.unwrap();

        let loaded = store.load_fold("projector").await.unwrap().expect("saved");
        assert_eq!(loaded, ahead, "a process behind does not set the fold back");
        assert!(store.load_fold("another").await.unwrap().is_none());

        for through in [1_100, 1_200, 1_300] {
            store.save_fold("projector", through, &ahead).await.unwrap();
        }
        let kept = store.0.list(&fold_folder("projector")).await.unwrap().len();
        assert_eq!(kept, GENERATIONS_KEPT, "the few saved last");
    }
}
