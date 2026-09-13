//! Closed periods of what variants were observed doing, written down.
//!
//! The read model is bounded by memory and by the log's retention, so what a
//! variant was observed doing last month is gone from it. When a period
//! closes, the serve role folds every variant's runs that ended in it into an
//! [`ObservedPeriod`] and writes it here: one object per variant, and then a
//! marker naming them — the marker is the commit, so a period is read only
//! once every record it names is stored (`aiwatcher_jobs::ORDERING`'s rule).
//! Each object is created once and never rewritten: two replicas folding one
//! log reach the same record, and the first to store it is the one kept.
//!
//! A period this fold cannot vouch for — it began folding after the period
//! started, or it let go of a run that may have ended in it — is not written.
//! A later fold that holds it writes it; a period nobody could is absent, and
//! the live fold answers for it for as long as it holds anything.

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

fn folder(from: i64, to: i64) -> String {
    format!("{PREFIX}{from:012}-{to:012}/")
}

/// A variant ID is a producer's text, so its key is its bytes in hex.
fn record_key(from: i64, to: i64, variant_id: &str) -> String {
    let named: String = variant_id
        .bytes()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{}{named}.json", folder(from, to))
}

fn marker_key(from: i64, to: i64) -> String {
    format!("{}period.json", folder(from, to))
}

fn encoded<T: Serialize>(value: &T) -> Result<Vec<u8>, PortError> {
    serde_json::to_vec(value).map_err(|error| PortError::Rejected {
        target: "variant-observations",
        message: error.to_string(),
    })
}

impl PeriodStore {
    #[must_use]
    pub fn new(store: Arc<dyn ObjectStore>) -> Self {
        Self(store)
    }

    /// Whether `[from, to)` is already written.
    ///
    /// # Errors
    ///
    /// The store's own failure.
    pub async fn written(&self, from: i64, to: i64) -> Result<bool, PortError> {
        Ok(self.0.get(&marker_key(from, to)).await?.is_some())
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
        from: i64,
        to: i64,
        records: &[ObservedPeriod],
        now: i64,
    ) -> Result<bool, PortError> {
        for record in records {
            self.0
                .create(&record_key(from, to, &record.variant_id), encoded(record)?)
                .await?;
        }
        let marker = Marker {
            from,
            to,
            variants: records
                .iter()
                .map(|record| record.variant_id.clone())
                .collect(),
            complete: records.iter().all(|record| record.complete),
            written_at: now,
        };
        self.0
            .create(&marker_key(from, to), encoded(&marker)?)
            .await
    }

    /// The written periods lying wholly inside `[since, until)`, and the
    /// records they hold for these variants. A period is returned whether or
    /// not it holds a record for any of them: a period with no run of a
    /// variant is a period in which it was not observed.
    ///
    /// # Errors
    ///
    /// The store's own failure, or a record that no longer reads.
    pub async fn read(
        &self,
        variant_ids: &[&str],
        since: i64,
        until: i64,
    ) -> Result<Vec<ObservedPeriod>, PortError> {
        let mut records = Vec::new();
        for entry in self.0.list(PREFIX).await? {
            let Some(bounds) = entry
                .key
                .strip_prefix(PREFIX)
                .and_then(|rest| rest.strip_suffix("/period.json"))
            else {
                continue;
            };
            let Some((from, to)) = bounds
                .split_once('-')
                .and_then(|(from, to)| from.parse::<i64>().ok().zip(to.parse::<i64>().ok()))
            else {
                continue;
            };
            if from < since || to > until {
                continue;
            }
            let Some(bytes) = self.0.get(&entry.key).await? else {
                continue;
            };
            let marker: Marker =
                serde_json::from_slice(&bytes).map_err(|error| PortError::Rejected {
                    target: "variant-observations",
                    message: format!("{}: {error}", entry.key),
                })?;
            for variant_id in variant_ids {
                if !marker.variants.iter().any(|held| held == variant_id) {
                    // Nothing of it ended here: an empty record, so the period
                    // still counts as covering the variant.
                    records.push(ObservedPeriod {
                        variant_id: (*variant_id).to_owned(),
                        from,
                        to,
                        complete: marker.complete,
                        ..ObservedPeriod::default()
                    });
                    continue;
                }
                let key = record_key(from, to, variant_id);
                let record = self.0.get(&key).await?.ok_or_else(|| PortError::Rejected {
                    target: "variant-observations",
                    message: format!("{key} is named by its period and is not stored"),
                })?;
                records.push(serde_json::from_slice(&record).map_err(|error| {
                    PortError::Rejected {
                        target: "variant-observations",
                        message: format!("{key}: {error}"),
                    }
                })?);
            }
        }
        records.sort_by_key(|record| (record.from, record.variant_id.clone()));
        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use aiwatcher_core::ports::PortResult;
    use aiwatcher_core::storage::ObjectEntry;
    use tokio::sync::RwLock;

    use super::*;

    /// A bucket that is a map, with the create-only write the store needs.
    #[derive(Debug, Default)]
    struct MemoryObjectStore(RwLock<BTreeMap<String, Vec<u8>>>);

    impl MemoryObjectStore {
        fn new() -> Self {
            Self::default()
        }
    }

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
        let store = PeriodStore::new(Arc::new(MemoryObjectStore::new()));
        let hour = 1_789_300_800;

        assert!(!store.written(hour, hour + 3_600).await.unwrap());
        assert!(
            store
                .write(hour, hour + 3_600, &[record("v1", hour, 4)], 1)
                .await
                .unwrap()
        );
        assert!(
            !store
                .write(hour, hour + 3_600, &[record("v1", hour, 9)], 2)
                .await
                .unwrap(),
            "the first writer's period is the one kept"
        );
        store
            .write(hour + 3_600, hour + 7_200, &[], 3)
            .await
            .unwrap();

        let read = store
            .read(&["v1", "v/2"], hour, hour + 7_200)
            .await
            .unwrap();
        let runs: Vec<(i64, &str, u64)> = read
            .iter()
            .map(|record| (record.from, record.variant_id.as_str(), record.runs))
            .collect();
        assert_eq!(
            runs,
            [
                (hour, "v/2", 0),
                (hour, "v1", 4),
                (hour + 3_600, "v/2", 0),
                (hour + 3_600, "v1", 0),
            ]
        );
        assert!(
            store
                .read(&["v1"], hour + 1, hour + 7_200)
                .await
                .unwrap()
                .iter()
                .all(|record| record.from == hour + 3_600),
            "a period only partly inside the window is not read"
        );
    }
}
