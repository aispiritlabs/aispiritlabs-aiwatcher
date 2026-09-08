//! Where a schedule is written down.
//!
//! Its own prefix — `schedules/` — beside the five registries and the
//! artifacts', rather than a file inside the pipeline's tree. Two reasons, and
//! the second is the one that decided it:
//!
//! * A schedule is keyed by [`DefinitionKind`] and a name, so it will hold a
//!   workflow's as readily as a curation pipeline's. Nesting it under
//!   `pipelines/` would make the second kind a second layout.
//! * `aiwatcher-datasets` owns `pipelines/` and this crate depends on it, not
//!   the other way round. Two crates writing one prefix is the arrangement the
//!   private-key-layout rule exists to prevent.
//!
//! ## Mutable, and outside the content address
//!
//! Everything else these registries keep is addressed by its content. A
//! schedule is not: changing nine to ten must not mint a pipeline revision,
//! because that would be a revision history about scheduling — the same
//! mistake as making `produced_by` part of a dataset version's identity. It is
//! a head with no versions behind it, which is what a prompt's labels are.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

use aiwatcher_core::prompts::ObjectStore;

use super::rule::Schedule;
use crate::error::StoreError;
use crate::plan::DefinitionKind;

/// The key prefix every schedule lives under.
pub const PREFIX: &str = "schedules";

/// A definition's schedule, as stored.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct ScheduledDefinition {
    pub definition_kind: DefinitionKind,
    pub definition_name: String,
    pub schedule: Schedule,
    /// Who set it, from the session. Recorded because a run nobody remembers
    /// asking for at three in the morning is a question with an answer.
    #[serde(default)]
    pub set_by: String,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    /// What the tick last did with this schedule.
    ///
    /// `None` until it has fired once, which is also what a schedule set a
    /// minute ago looks like.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<LastFiring>,
}

/// What the tick did the last time this schedule came round.
///
/// **The scheduler's own decision, and never the run's outcome.** Whether the
/// run then succeeded is on the event log, which the Workflows view folds, and
/// a second copy here would be a second answer free to disagree with it
/// (ADR_0026). What is recorded is the thing nothing else knows: that a slot
/// came round, and what this loop did about it.
///
/// It exists because the alternatives were a log line and silence. A schedule
/// that has been refused every morning for a week looks, from the panel,
/// exactly like one that has been working.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct LastFiring {
    /// The slot, not the moment the tick noticed it. A run started late after
    /// an outage belongs to the nine o'clock it was for.
    #[serde(with = "time::serde::rfc3339")]
    pub slot: OffsetDateTime,
    pub outcome: FiringOutcome,
    /// The run it started. Absent when it started none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    /// Why, when it did not start one. The refusal in the words the caller
    /// would have read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FiringOutcome {
    /// A run was started. Whether it *succeeded* is the log's answer, by the
    /// id beside this.
    Started,
    /// The last run had not finished and the policy is `OverlapPolicy::Skip`.
    Skipped,
    /// Nothing could be started — the definition stopped compiling, or the
    /// store was unreachable. The one outcome that is a problem.
    Refused,
}

impl ScheduledDefinition {
    /// The id the run for one slot is named by.
    ///
    /// Derived, never generated, and derived from the *definition and the
    /// slot* rather than from a compiled plan: two workers reading the head a
    /// moment apart could compile different `plan_id`s, and an id built on one
    /// of those would let both runs through. From this, two callers that both
    /// decide 09:00 is due produce one execution and one conflict —
    /// `ExpectedVersion::NoStream` doing what it already does for two API
    /// replicas racing a start.
    ///
    /// It lives here rather than in either caller because there are two: the
    /// tick in the work role, and the route that starts a schedule immediately
    /// when somebody asks it to.
    #[must_use]
    pub fn execution_id_for(&self, slot: OffsetDateTime) -> crate::ExecutionId {
        crate::ExecutionId::new(crate::derive_uuid(&format!(
            "aiwatcher/execution/schedule/{}/{}/{}",
            self.definition_kind.as_str(),
            self.definition_name,
            slot.unix_timestamp(),
        )))
    }
}

/// Schedules, over the object store the registries already use.
#[derive(Clone, Debug)]
pub struct ScheduleStore {
    store: Arc<dyn ObjectStore>,
    prefix: String,
}

impl ScheduleStore {
    #[must_use]
    pub fn new(store: Arc<dyn ObjectStore>) -> Self {
        Self {
            store,
            prefix: PREFIX.to_owned(),
        }
    }

    /// Set or replace one definition's schedule.
    ///
    /// # Errors
    ///
    /// [`StoreError::Backend`] when the object store could not be written.
    pub async fn set(&self, scheduled: &ScheduledDefinition) -> crate::Result<()> {
        let body = serde_json::to_vec(scheduled).map_err(StoreError::Encoding)?;
        self.store
            .put(
                &self.key(scheduled.definition_kind, &scheduled.definition_name),
                body,
            )
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?;
        Ok(())
    }

    /// One definition's schedule, or `None` when it has none.
    ///
    /// # Errors
    ///
    /// [`StoreError::Backend`] when the object store could not be read.
    pub async fn get(
        &self,
        kind: DefinitionKind,
        name: &str,
    ) -> crate::Result<Option<ScheduledDefinition>> {
        let Some(body) = self
            .store
            .get(&self.key(kind, name))
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?
        else {
            return Ok(None);
        };
        serde_json::from_slice(&body)
            .map(Some)
            .map_err(|error| StoreError::Backend(error.to_string()))
    }

    /// Forget one definition's schedule.
    ///
    /// Deleting is the right verb here and `enabled: false` is a different
    /// thing: one says "not any more", the other says "not for now, and these
    /// are still the settings". A person who wanted the second and got the
    /// first re-types an hour from memory next month.
    ///
    /// # Errors
    ///
    /// [`StoreError::Backend`] when the object store could not be written.
    pub async fn clear(&self, kind: DefinitionKind, name: &str) -> crate::Result<()> {
        self.store
            .delete(&self.key(kind, name))
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?;
        Ok(())
    }

    /// Every schedule, for the tick to read.
    ///
    /// A whole listing on every tick, and that is affordable by construction:
    /// there is at most one object per *definition somebody scheduled*, which
    /// is a number a person types rather than one a run produces. The lists
    /// that grow with retention are the ones that page.
    ///
    /// A row this build cannot decode is skipped rather than fatal — one
    /// schedule written by a newer version must not stop every other one
    /// firing.
    ///
    /// # Errors
    ///
    /// [`StoreError::Backend`] when the object store could not be listed.
    pub async fn all(&self) -> crate::Result<Vec<ScheduledDefinition>> {
        let entries = self
            .store
            .list(&format!("{}/", self.prefix))
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?;

        let mut scheduled = Vec::new();
        for entry in entries {
            let Some(body) = self
                .store
                .get(&entry.key)
                .await
                .map_err(|error| StoreError::Backend(error.to_string()))?
            else {
                continue;
            };
            match serde_json::from_slice::<ScheduledDefinition>(&body) {
                Ok(row) => scheduled.push(row),
                Err(error) => {
                    tracing::warn!(key = %entry.key, %error, "a schedule this build cannot read");
                }
            }
        }
        // By name, so a tick that starts several runs starts them in an order
        // somebody can predict from the list they are looking at.
        scheduled.sort_by(|a, b| {
            (a.definition_kind.as_str(), &a.definition_name)
                .cmp(&(b.definition_kind.as_str(), &b.definition_name))
        });
        Ok(scheduled)
    }

    /// The key one schedule lives at.
    ///
    /// The name is hashed rather than sanitised, for the reason the staging
    /// directory is: a definition name holds separators, and a key built by
    /// replacing them is a key two names can collide on.
    fn key(&self, kind: DefinitionKind, name: &str) -> String {
        format!(
            "{}/{}/{}.json",
            self.prefix,
            kind.as_str(),
            crate::digest(name.as_bytes())
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::rule::{Cadence, OverlapPolicy};

    fn scheduled(name: &str, hour: u8) -> ScheduledDefinition {
        ScheduledDefinition {
            definition_kind: DefinitionKind::CurationPipeline,
            definition_name: name.to_owned(),
            schedule: Schedule {
                cadence: Cadence::Daily { hour, minute: 0 },
                timezone: "Europe/Warsaw".to_owned(),
                enabled: true,
                overlap: OverlapPolicy::Skip,
            },
            set_by: "somebody".to_owned(),
            updated_at: OffsetDateTime::UNIX_EPOCH,
            last: None,
        }
    }

    fn store() -> ScheduleStore {
        ScheduleStore::new(Arc::new(
            aiwatcher_prompts::adapters::memory::MemoryObjectStore::new(),
        ))
    }

    #[test]
    fn two_callers_deciding_the_same_slot_is_due_derive_one_execution() {
        // The whole reason there is no lease. Both workers reach this, both
        // start, and the second is a conflict rather than a second run.
        let slot = OffsetDateTime::from_unix_timestamp(1_788_000_000).expect("a slot");
        assert_eq!(
            scheduled("curation/pii", 9).execution_id_for(slot),
            scheduled("curation/pii", 10).execution_id_for(slot),
            "the hour somebody typed must not change which run a slot is",
        );
        assert_ne!(
            scheduled("curation/pii", 9).execution_id_for(slot),
            scheduled("curation/other", 9).execution_id_for(slot),
        );
        assert_ne!(
            scheduled("curation/pii", 9).execution_id_for(slot),
            scheduled("curation/pii", 9).execution_id_for(slot + time::Duration::days(1)),
        );
    }

    #[tokio::test]
    async fn a_schedule_is_read_back_as_it_was_set() {
        let store = store();
        store.set(&scheduled("curation/pii", 9)).await.expect("set");

        let found = store
            .get(DefinitionKind::CurationPipeline, "curation/pii")
            .await
            .expect("get");
        assert_eq!(found, Some(scheduled("curation/pii", 9)));
    }

    #[tokio::test]
    async fn setting_it_again_replaces_rather_than_accumulates() {
        // A head, not a version history: there is one answer to "when does this
        // run", and keeping the old hours would be keeping a record of
        // somebody's typing.
        let store = store();
        store.set(&scheduled("curation/pii", 9)).await.expect("set");
        store
            .set(&scheduled("curation/pii", 10))
            .await
            .expect("set");

        assert_eq!(store.all().await.expect("all").len(), 1);
        assert_eq!(
            store
                .get(DefinitionKind::CurationPipeline, "curation/pii")
                .await
                .expect("get")
                .map(|row| row.schedule.cadence),
            Some(Cadence::Daily {
                hour: 10,
                minute: 0
            })
        );
    }

    #[tokio::test]
    async fn two_names_that_would_sanitise_alike_keep_two_schedules() {
        // `curation/pii` and `curation-pii` differ by a separator, and a key
        // built by replacing separators would put them in one place — which is
        // one definition running on the other's hour.
        let store = store();
        store.set(&scheduled("curation/pii", 9)).await.expect("set");
        store
            .set(&scheduled("curation-pii", 10))
            .await
            .expect("set");

        assert_eq!(store.all().await.expect("all").len(), 2);
    }

    #[tokio::test]
    async fn what_the_tick_did_is_read_back_with_the_schedule() {
        // The gap this closes: a schedule refused every morning for a week
        // looked, from the panel, exactly like one that had been working.
        let store = store();
        let mut row = scheduled("curation/pii", 9);
        row.last = Some(LastFiring {
            slot: OffsetDateTime::from_unix_timestamp(1_788_000_000).expect("a slot"),
            outcome: FiringOutcome::Refused,
            execution_id: None,
            detail: Some("curation/pii does not compile".to_owned()),
        });
        store.set(&row).await.expect("set");

        let found = store
            .get(DefinitionKind::CurationPipeline, "curation/pii")
            .await
            .expect("get")
            .expect("a schedule");
        assert_eq!(
            found.last.as_ref().map(|last| last.outcome),
            Some(FiringOutcome::Refused)
        );
        assert_eq!(
            found.last.and_then(|last| last.detail).as_deref(),
            Some("curation/pii does not compile")
        );
    }

    #[tokio::test]
    async fn a_cleared_schedule_stops_being_listed() {
        let store = store();
        store.set(&scheduled("curation/pii", 9)).await.expect("set");
        store
            .clear(DefinitionKind::CurationPipeline, "curation/pii")
            .await
            .expect("clear");

        assert!(store.all().await.expect("all").is_empty());
        assert_eq!(
            store
                .get(DefinitionKind::CurationPipeline, "curation/pii")
                .await
                .expect("get"),
            None
        );
    }

    #[tokio::test]
    async fn a_definition_nobody_scheduled_has_no_schedule_rather_than_an_error() {
        assert_eq!(
            store()
                .get(DefinitionKind::CurationPipeline, "curation/never")
                .await
                .expect("get"),
            None
        );
    }
}
