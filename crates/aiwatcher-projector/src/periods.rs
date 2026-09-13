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
//! position it was saved at: created, never overwritten, and the one furthest
//! along is the one loaded — so two processes sharing a processor ID cannot
//! set each other back, whichever saves last.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use aiwatcher_core::ports::PortError;
use aiwatcher_core::storage::ObjectStore;

use crate::observations::ObservedPeriod;

/// The prefix this module owns in the deployment's object store.
pub const PREFIX: &str = "variant-observations/";

/// What says a period was written, and which variants it holds records for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Marker {
    from: i64,
    to: i64,
    variants: Vec<String>,
    complete: bool,
    written_at: i64,
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
fn record_key(level: i64, from: i64, variant_id: &str) -> String {
    format!("{}{}.json", folder(level, from), hex(variant_id))
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
    /// through, then remove the states saved before it. A state already saved
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
        for entry in self.0.list(&folder).await? {
            if entry.key < key {
                self.0.delete(&entry.key).await?;
            }
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
    ) -> Result<bool, PortError> {
        for record in records {
            self.0
                .create(
                    &record_key(level, from, &record.variant_id),
                    encoded(record)?,
                )
                .await?;
        }
        let marker = Marker {
            from,
            to: from + level,
            variants: records
                .iter()
                .map(|record| record.variant_id.clone())
                .collect(),
            complete: records.iter().all(|record| record.complete),
            written_at: now,
        };
        self.0
            .create(&marker_key(level, from), encoded(&marker)?)
            .await
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
        variant_ids: &[&str],
    ) -> Result<Option<Vec<ObservedPeriod>>, PortError> {
        let key = marker_key(level, from);
        let Some(bytes) = self.0.get(&key).await? else {
            return Ok(None);
        };
        let marker: Marker = decoded(&key, &bytes)?;
        let mut records = Vec::new();
        for variant_id in variant_ids {
            if !marker.variants.iter().any(|held| held == variant_id) {
                continue;
            }
            let key = record_key(level, from, variant_id);
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

        assert!(store.period(3_600, hour, &["v1"]).await.unwrap().is_none());
        assert!(
            store
                .write(3_600, hour, &[record("v1", hour, 4)], 1)
                .await
                .unwrap()
        );
        assert!(
            !store
                .write(3_600, hour, &[record("v1", hour, 9)], 2)
                .await
                .unwrap(),
            "the first writer's period is the one kept"
        );

        let read = store
            .period(3_600, hour, &["v1", "v/2"])
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
            store.period(300, hour, &["v1"]).await.unwrap().is_none(),
            "another level is another period"
        );
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
    }
}
