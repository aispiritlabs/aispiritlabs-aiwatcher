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
    /// When the *current cadence* began to apply.
    ///
    /// Review R7: without this every schedule was handed the whole interval a
    /// checkpoint had accumulated, so one written while the worker was down for
    /// three days ran three days of slots the moment it came back — and
    /// changing an hour during an outage re-ran the past under the new rule.
    ///
    /// Not `updated_at`, and that is the review's own warning: an edit that
    /// does not change *when* it fires would then quietly drop a slot that was
    /// already due. It moves only when [`Schedule::fires_the_same_as`] says the
    /// rule changed — which makes re-enabling an activation too, so a schedule
    /// switched back on starts from now rather than running the days it was
    /// off.
    ///
    /// Optional so a schedule stored before this field existed still reads;
    /// [`Self::effective_from`] answers for it, and the fallback is
    /// `updated_at` because that is when such a schedule was last written.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub effective_from: Option<OffsetDateTime>,
}

/// A schedule's firings live in the workflow store, not here.
///
/// They were a field on the struct above until review R3. The tick read every
/// schedule, did its work and wrote the whole object back, so an edit or a
/// DELETE that landed in between was overwritten by a snapshot taken before
/// it — a deleted schedule came back enabled, and a `get`-before-`put` does
/// not close that, because the object store offers no compare-and-set.
///
/// Configuration and slot outcomes have different writers and different
/// lifetimes, so they now live in different places: this object is written
/// only by whoever sets a schedule, and [`crate::SlotRecord`] is written only
/// by whoever takes a slot. See [`crate::schedule::slot`].
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
    /// When this schedule's current rule started applying.
    #[must_use]
    pub fn effective_from(&self) -> OffsetDateTime {
        self.effective_from.unwrap_or(self.updated_at)
    }

    /// The slots this definition is due for in an interval.
    ///
    /// [`Schedule::slots_between`] with the interval clipped to the activation
    /// moment, which is the one thing the pure rule cannot know: a cadence
    /// describes every instant it would ever fire at, and *since when* is a
    /// fact about this stored schedule rather than about "daily at nine".
    #[must_use]
    pub fn slots_due(&self, previous: OffsetDateTime, now: OffsetDateTime) -> Vec<OffsetDateTime> {
        self.schedule
            .slots_between(previous.max(self.effective_from()), now)
    }

    /// The activation moment a new version of this schedule should carry.
    ///
    /// Called with what is already stored, if anything. A rule that fires at
    /// the same instants keeps the moment it has had all along, so an edit to
    /// `overlap` — or to nothing at all — cannot drop a slot that is already
    /// due; a rule that fires differently starts now.
    #[must_use]
    pub fn activation_after(&self, stored: Option<&Self>, now: OffsetDateTime) -> OffsetDateTime {
        match stored {
            Some(stored) if stored.schedule.fires_the_same_as(&self.schedule) => {
                stored.effective_from()
            }
            _ => now,
        }
    }

    /// The id a run somebody asked for by hand is named by.
    ///
    /// Derived from a **request identity** rather than from the clock, which is
    /// the whole difference. `execution_id_for(now)` names a slot's second, so
    /// a double-clicked button inside one second was one run and a retry a
    /// second later was two — a network retry, a proxy repeating a PUT, or
    /// somebody clicking again because the first response was slow.
    ///
    /// The caller supplies the identity because only the caller knows whether
    /// this is the same intention as last time. Absent, there is nothing
    /// better than the clock and the second is what it falls back to.
    #[must_use]
    pub fn execution_id_for_request(&self, request_id: &str) -> crate::ExecutionId {
        crate::ExecutionId::new(crate::derive_uuid(&format!(
            "aiwatcher/execution/schedule/{}/{}/request/{}",
            self.definition_kind.as_str(),
            self.definition_name,
            request_id,
        )))
    }

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

/// What the tick may do with schedules: read them.
///
/// **Review R3, enforced by the signature rather than by remembering.** The
/// loop used to hold a whole [`ScheduleStore`], and it wrote back to it — the
/// snapshot it had taken before doing its work, which overwrote any edit or
/// DELETE that landed in between. A deleted schedule came back enabled, and no
/// amount of re-reading before the write closes that, because an object store
/// offers no compare-and-set.
///
/// So the tick is handed this instead. A future change that wants the tick to
/// write configuration has to widen this trait first, which is the moment to
/// re-read R3 rather than a line in a review nobody runs.
#[async_trait::async_trait]
pub trait ScheduleReader: Send + Sync + std::fmt::Debug {
    /// Every schedule, for the tick to read.
    ///
    /// # Errors
    ///
    /// Whatever the store could not do.
    async fn all(&self) -> crate::Result<Vec<ScheduledDefinition>>;
}

#[async_trait::async_trait]
impl ScheduleReader for ScheduleStore {
    async fn all(&self) -> crate::Result<Vec<ScheduledDefinition>> {
        Self::all(self).await
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
    use time::macros::datetime;

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
            effective_from: None,
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
        // Two kinds may share a name — the store is keyed by both, and a
        // registered workflow is schedulable beside a pipeline. An id that
        // dropped the kind would make the two nine o'clocks one run, and the
        // second would be refused as a redelivery of the first with nothing
        // anywhere to say a schedule had not fired.
        let mut workflow = scheduled("curation/pii", 9);
        workflow.definition_kind = DefinitionKind::Workflow;
        assert_ne!(
            scheduled("curation/pii", 9).execution_id_for(slot),
            workflow.execution_id_for(slot),
        );
        assert_ne!(
            scheduled("curation/pii", 9).execution_id_for_request("one-click"),
            workflow.execution_id_for_request("one-click"),
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

    #[test]
    fn a_schedule_written_during_an_outage_does_not_run_the_days_before_it_existed() {
        // The tick hands every schedule the whole interval its
        // checkpoint accumulated, so a worker down from Monday to Thursday
        // ticks once with `previous` on Monday — and a schedule somebody wrote
        // on Wednesday would have run Monday's and Tuesday's nine o'clock too,
        // for days it did not exist.
        let mut written = scheduled("nightly", 9);
        written.updated_at = datetime!(2026-09-09 12:00 UTC);
        written.effective_from = Some(datetime!(2026-09-09 12:00 UTC));

        let due = written.slots_due(
            datetime!(2026-09-07 00:00 UTC),
            datetime!(2026-09-10 12:00 UTC),
        );

        assert_eq!(
            due,
            vec![datetime!(2026-09-10 07:00 UTC)],
            "only the nine o'clock after it was written — 07:00Z is 09:00 in Warsaw"
        );
    }

    #[test]
    fn an_edit_that_does_not_change_when_it_fires_keeps_a_slot_that_was_already_due() {
        // The review's own warning about the naive fix: taking `updated_at` as
        // the boundary would make every edit drop a pending slot. Somebody
        // switching `skip` to `allow` at 08:59 must still get nine o'clock.
        let stored = {
            let mut stored = scheduled("nightly", 9);
            stored.updated_at = datetime!(2026-09-01 00:00 UTC);
            stored.effective_from = Some(datetime!(2026-09-01 00:00 UTC));
            stored
        };

        let edited_at = datetime!(2026-09-10 06:59 UTC);
        let mut edit = scheduled("nightly", 9);
        edit.schedule.overlap = OverlapPolicy::Allow;
        edit.updated_at = edited_at;
        edit.effective_from = Some(edit.activation_after(Some(&stored), edited_at));

        assert_eq!(
            edit.effective_from, stored.effective_from,
            "changing the overlap policy is not a change to when it fires"
        );
        assert_eq!(
            edit.slots_due(
                datetime!(2026-09-10 06:00 UTC),
                datetime!(2026-09-10 08:00 UTC)
            ),
            vec![datetime!(2026-09-10 07:00 UTC)],
            "the slot that was already due survived the edit"
        );
    }

    #[test]
    fn changing_the_hour_starts_the_new_rule_from_the_edit_rather_than_the_past() {
        let stored = {
            let mut stored = scheduled("nightly", 9);
            stored.updated_at = datetime!(2026-09-01 00:00 UTC);
            stored.effective_from = Some(datetime!(2026-09-01 00:00 UTC));
            stored
        };

        let edited_at = datetime!(2026-09-10 12:00 UTC);
        let mut edit = scheduled("nightly", 3);
        edit.updated_at = edited_at;
        edit.effective_from = Some(edit.activation_after(Some(&stored), edited_at));

        assert_eq!(edit.effective_from, Some(edited_at));
        assert!(
            edit.slots_due(datetime!(2026-09-08 00:00 UTC), edited_at)
                .is_empty(),
            "a cadence changed today does not re-run the past two days under the new rule"
        );
    }

    #[test]
    fn re_enabling_a_schedule_does_not_run_the_days_it_was_off() {
        // Enabling is a change to when it fires, so it is an activation. The
        // alternative — carrying the moment across — would make switching a
        // schedule back on start every slot it had missed while it was off,
        // which is not what anybody means by the toggle.
        let off = {
            let mut off = scheduled("nightly", 9);
            off.schedule.enabled = false;
            off.updated_at = datetime!(2026-09-01 00:00 UTC);
            off.effective_from = Some(datetime!(2026-09-01 00:00 UTC));
            off
        };

        let back_on_at = datetime!(2026-09-10 12:00 UTC);
        let mut back_on = scheduled("nightly", 9);
        back_on.updated_at = back_on_at;
        back_on.effective_from = Some(back_on.activation_after(Some(&off), back_on_at));

        assert_eq!(back_on.effective_from, Some(back_on_at));
        assert!(
            back_on
                .slots_due(datetime!(2026-09-01 00:00 UTC), back_on_at)
                .is_empty()
        );
    }

    #[test]
    fn a_schedule_stored_before_this_field_existed_is_effective_from_when_it_was_written() {
        // The compatibility half: `effective_from` is absent in every schedule
        // written by the release before it, and reading such a row must not
        // mean "effective since the epoch", which is the answer that runs every
        // slot since 1970.
        let mut old = scheduled("nightly", 9);
        old.updated_at = datetime!(2026-09-09 12:00 UTC);
        old.effective_from = None;

        assert_eq!(old.effective_from(), datetime!(2026-09-09 12:00 UTC));
        assert!(
            old.slots_due(
                datetime!(2026-09-07 00:00 UTC),
                datetime!(2026-09-09 13:00 UTC)
            )
            .is_empty()
        );
    }
}
