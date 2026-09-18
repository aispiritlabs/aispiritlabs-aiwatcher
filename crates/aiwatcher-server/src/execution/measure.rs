//! What this deployment is actually doing, as numbers somebody can graph.
//!
//! Two readings, together because they are the same kind of question: how far
//! behind is this, and how much is piling up.
//!
//! **How late is a scheduled run, and how many were due at once?** Lateness is
//! measured from the slot to the moment the tick *found* it, never to the
//! moment the run started — the second folds the store's latency and the
//! compiler's into a number about the clock. It rides with the backlog from the
//! same tick, because one slot four minutes behind and forty of them are the
//! same lateness and very different news.
//!
//! **How much is in the object store that nothing will delete?** Workflow
//! retention prunes executions and does not follow their artifacts. Whether an
//! object is still *reachable* is a question about streams retention has been
//! deleting, so this counts what is there and says nothing about what should
//! be; the curve over weeks is what decides whether a collector is worth
//! building.
//!
//! **And whose it is.** The artifact prefix now has a scoped area below it, so
//! one walk finds a deployment's own objects and every project's together. They
//! are counted apart: a project's bytes reported as the deployment's would be
//! this measurement saying a tenant's storage is everybody's, and — the sharper
//! half — a collector built one day on "the artifact prefix holds objects no
//! execution history names" would take the absence of *global* history as
//! permission to delete a project's. There is no such collector, and this adds
//! none: nothing here deletes, and nothing here has a scoped source of truth
//! about reachability to delete from.
//!
//! Two of the four buckets exist to stop a number being invented. An object
//! under the scoped area naming no project is *unattributed* rather than
//! global, and a key a listing returned from outside the prefix it was asked
//! about is *elsewhere* rather than a row in any total.
//!
//! Staging is **not** here. A notebook's staged rows live in
//! `services/ml_pipeline`, in that process's own scratch directory, keyed by a
//! hash of the context — this crate cannot list that disk, and a zero reported
//! from here would be worse than no number. `GET /ml-pipeline/staging` answers
//! for it.

use std::sync::Arc;

use aiwatcher_core::ports::{Attr, MetricKind, MetricSample, MetricSink, attr};
use aiwatcher_core::prompts::ObjectStore;
use aiwatcher_execution::artifact::layout::{self, KeyOwner};
use aiwatcher_iam::{OrganizationId, ProjectId, ProjectScope};
use std::collections::BTreeMap;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;

/// How late one slot was, and how many the tick found with it.
///
/// Both from the same tick, because reading either alone misleads: one slot
/// four minutes late is a tick that was busy, and forty slots four minutes late
/// is an instance that has fallen behind. The count is per tick rather than
/// cumulative — a backlog is a depth, not a total.
pub fn slot_samples(
    definition: &str,
    kind: &str,
    slot: OffsetDateTime,
    found_at: OffsetDateTime,
    due_this_tick: usize,
) -> Vec<MetricSample> {
    // Clamped at zero rather than allowed negative. A slot found before it was
    // due means the two clocks disagree, which is a fact about the host and not
    // about the scheduler — and a negative lateness would poison the histogram
    // for the question it exists to answer.
    let lateness = (found_at - slot).as_seconds_f64().max(0.0);
    let where_from = vec![
        attr("definition", definition.to_owned()),
        attr("definition_kind", kind.to_owned()),
    ];
    vec![
        MetricSample {
            name: "aiwatcher.schedule.slot.lateness".to_owned(),
            kind: MetricKind::Histogram,
            value: lateness,
            unit: Some("s".to_owned()),
            at: found_at,
            attributes: where_from.clone(),
        },
        MetricSample {
            name: "aiwatcher.schedule.slots.due".to_owned(),
            kind: MetricKind::Gauge,
            value: due_this_tick as f64,
            unit: None,
            at: found_at,
            attributes: where_from,
        },
    ]
}

/// How often the object store is walked for what retention leaves behind.
///
/// An hour. This is a `list` over a whole prefix, which is the one operation
/// here that costs money on a real bucket, and the number it answers moves over
/// days rather than minutes. A deployment that wants it finer can afford to ask
/// again; one that wants it cheaper turns the sink off.
const SWEEP: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// What one walk of the artifact prefix found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stored {
    pub objects: u64,
    pub bytes: u64,
    /// The oldest thing still there, in whole days. `None` when nothing has a
    /// modification time — the filesystem adapter has one, and a bucket may not
    /// report one for every key.
    pub oldest_days: Option<u64>,
}

impl Stored {
    /// Add one object to this bucket.
    fn add(&mut self, entry: &aiwatcher_core::prompts::ObjectEntry, now: OffsetDateTime) {
        self.objects += 1;
        self.bytes += entry.size;
        if let Some(at) = entry.last_modified {
            let days = (now - at).whole_days().max(0).unsigned_abs();
            self.oldest_days = Some(self.oldest_days.map_or(days, |held| held.max(days)));
        }
    }

    fn samples(self, at: OffsetDateTime, where_from: &[Attr]) -> Vec<MetricSample> {
        let gauge = |name: &str, value: f64| MetricSample {
            name: name.to_owned(),
            kind: MetricKind::Gauge,
            value,
            unit: None,
            at,
            attributes: where_from.to_vec(),
        };
        let mut samples = vec![
            gauge("aiwatcher.artifacts.objects", self.objects as f64),
            gauge("aiwatcher.artifacts.bytes", self.bytes as f64),
        ];
        // Absent rather than zero. "Nothing here is older than today" and "no
        // key reports a date" are different states, and a zero says the first
        // when the store meant the second.
        if let Some(days) = self.oldest_days {
            samples.push(gauge("aiwatcher.artifacts.oldest_days", days as f64));
        }
        samples
    }
}

/// One walk of the artifact prefix, split by whose objects it found.
///
/// A partition rather than a total and some of its parts: every key the walk
/// returned lands in exactly one bucket, so summing a metric across the series
/// is the whole prefix and no series is inside another.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Storage {
    /// What this deployment held before projects existed, and what an unscoped
    /// writer still writes.
    pub global: Stored,
    /// One entry per project that holds anything. Absent projects are absent
    /// rather than zero — an empty bucket here would be a claim about a project
    /// this walk found nothing of, and this walk cannot know which exist.
    ///
    /// Keyed by the two identifiers rather than by [`ProjectScope`], which is
    /// neither `Ord` nor `Hash`. [`Storage::project`] reads it back by scope.
    pub projects: BTreeMap<(OrganizationId, ProjectId), Stored>,
    /// Under the scoped area, naming no project this code would have written.
    /// Empty for a healthy store, and reported rather than folded into the
    /// deployment's own count.
    pub unattributed: Stored,
    /// Returned by a listing from outside the prefix it was asked about. A
    /// store's bug; counted so that it is visible and never added to a total.
    pub elsewhere: Stored,
}

/// Walk the artifact prefix and say what is in it, and whose.
///
/// Deliberately a count and not an opinion. Whether an object is *reachable* is
/// a question about every execution's stream, which retention has already been
/// deleting — so the honest thing this can report is what is there, and the
/// shape of the curve over weeks is what says whether a collector is worth
/// building. It is not an opinion about *ownership* either: the key says which
/// project wrote an object, and nothing here asks IAM whether that project
/// still exists.
#[must_use]
pub fn summarise(entries: &[aiwatcher_core::prompts::ObjectEntry], now: OffsetDateTime) -> Storage {
    let mut storage = Storage::default();
    for entry in entries {
        match layout::owner_of(&entry.key) {
            KeyOwner::Global => storage.global.add(entry, now),
            KeyOwner::Project(scope) => {
                storage
                    .projects
                    .entry((scope.organization, scope.project))
                    .or_default()
                    .add(entry, now);
            }
            KeyOwner::Unattributed => storage.unattributed.add(entry, now),
            KeyOwner::Elsewhere => storage.elsewhere.add(entry, now),
        }
    }
    storage
}

impl Storage {
    /// What this walk found of one project, or nothing if it found none.
    #[must_use]
    pub fn project(&self, scope: ProjectScope) -> Option<&Stored> {
        self.projects.get(&(scope.organization, scope.project))
    }

    /// Every bucket as its own series.
    ///
    /// The deployment-wide one keeps the attributes it has always had, so a
    /// graph that was reading it goes on reading it — and now reads only what
    /// it always said it was. A project's carries its two identifiers as well
    /// as its prefix; the unattributed bucket carries the scoped area and no
    /// identifiers, because inventing them is the thing it exists to avoid.
    ///
    /// `elsewhere` gets no series at all. It is a fact about the object store
    /// rather than about what is stored, and the loop says it in the log where
    /// somebody debugging an adapter will look, rather than in a gauge named
    /// for a prefix it is not under.
    #[must_use]
    pub fn samples(&self, at: OffsetDateTime) -> Vec<MetricSample> {
        let mut samples = self
            .global
            .samples(at, &[attr("prefix", layout::PREFIX.to_owned())]);
        for ((organization, project), stored) in &self.projects {
            let scope = ProjectScope {
                organization: *organization,
                project: *project,
            };
            samples.extend(stored.samples(
                at,
                &[
                    attr("prefix", layout::prefix(Some(scope))),
                    attr("organization", organization.0.to_string()),
                    attr("project", project.0.to_string()),
                ],
            ));
        }
        if self.unattributed != Stored::default() {
            samples.extend(
                self.unattributed
                    .samples(at, &[attr("prefix", format!("{}/scopes", layout::PREFIX))]),
            );
        }
        samples
    }
}

/// The prefix one walk covers: everything under the artifact key space,
/// projects included.
fn artifact_prefix() -> String {
    format!("{}/", layout::PREFIX)
}

/// One walk, or nothing when the store could not be asked.
///
/// `None` rather than an empty [`Storage`]: a prefix that could not be listed
/// and a prefix that holds nothing are different facts, and reporting the
/// second for the first would draw a cliff in every graph the moment a bucket
/// had a bad minute — and, for a project's series, would read as a project
/// whose storage had gone.
///
/// It reads. There is no sibling that deletes, and the module docs say why.
async fn walk(store: &dyn ObjectStore, now: OffsetDateTime) -> Option<Storage> {
    match store.list(&artifact_prefix()).await {
        Ok(entries) => Some(summarise(&entries, now)),
        Err(error) => {
            // A measurement that could not be taken is not a failure of
            // anything this instance is doing. Said once and dropped.
            tracing::warn!(%error, "the artifact prefix could not be walked");
            None
        }
    }
}

/// Report what the object store holds, every hour, until shutdown.
///
/// Spawned only where both an object store and a metric sink exist. Neither is
/// required to run aiwatcher, and a deployment with no sink would be paying for
/// a full `list` to throw the answer away.
///
/// It reads and it reports. It deletes nothing, and it is not the place a
/// collector goes: deciding an object is unreachable needs a source of truth
/// about what still names it, and for the scoped area there is none — no
/// project execution history exists yet to be asked. A pass that deleted what
/// no *deployment-wide* history named would delete a project's bytes for
/// having no global history, which is the one failure this split exists to
/// make impossible.
pub fn spawn_storage_sweep(
    store: Arc<dyn ObjectStore>,
    metrics: Arc<dyn MetricSink>,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tracing::info!(
        seconds = SWEEP.as_secs(),
        "measuring what the artifact prefix holds"
    );
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(SWEEP) => {}
            }
            let now = OffsetDateTime::now_utc();
            let Some(stored) = walk(store.as_ref(), now).await else {
                continue;
            };
            let prefix = artifact_prefix();
            tracing::debug!(
                objects = stored.global.objects,
                bytes = stored.global.bytes,
                oldest_days = stored.global.oldest_days,
                projects = stored.projects.len(),
                "the artifact prefix, measured"
            );
            // Said where somebody debugging an adapter will look, rather than
            // added to a total it is not part of or given a gauge named for a
            // prefix it is not under.
            if stored.elsewhere != Stored::default() {
                tracing::warn!(
                    objects = stored.elsewhere.objects,
                    %prefix,
                    "the object store answered a listing with keys outside the prefix"
                );
            }
            if stored.unattributed != Stored::default() {
                tracing::warn!(
                    objects = stored.unattributed.objects,
                    "the scoped artifact area holds objects that name no project"
                );
            }
            if let Err(error) = metrics.record(stored.samples(now)).await {
                tracing::warn!(%error, "the storage measurement could not be reported");
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aiwatcher_core::prompts::ObjectEntry;
    use time::Duration;

    const NOW: OffsetDateTime = OffsetDateTime::UNIX_EPOCH;

    fn entry(key: &str, size: u64, modified: Option<OffsetDateTime>) -> ObjectEntry {
        ObjectEntry {
            key: key.to_owned(),
            size,
            last_modified: modified,
        }
    }

    #[test]
    fn lateness_is_measured_to_the_tick_that_found_the_slot() {
        let samples = slot_samples(
            "curation/pii",
            "curation_pipeline",
            NOW,
            NOW + Duration::minutes(4),
            1,
        );
        assert_eq!(samples[0].name, "aiwatcher.schedule.slot.lateness");
        assert_eq!(samples[0].value, 240.0);
        assert_eq!(samples[0].unit.as_deref(), Some("s"));
    }

    #[test]
    fn a_slot_found_before_it_was_due_reports_no_lateness_rather_than_negative() {
        // Two clocks disagreeing is a fact about the host. A negative
        // observation would poison the histogram for the question it is for.
        let samples = slot_samples("d", "workflow", NOW, NOW - Duration::seconds(30), 1);
        assert_eq!(samples[0].value, 0.0);
    }

    #[test]
    fn the_backlog_is_a_depth_rather_than_a_total() {
        // One slot four minutes late is a busy tick; forty is an instance that
        // has fallen behind. Reading the lateness without the count cannot tell
        // those apart, so both come from the same tick.
        let samples = slot_samples("d", "workflow", NOW, NOW + Duration::minutes(4), 40);
        let due = &samples[1];
        assert_eq!(due.name, "aiwatcher.schedule.slots.due");
        assert_eq!(due.kind, MetricKind::Gauge);
        assert_eq!(due.value, 40.0);
    }

    #[test]
    fn a_walk_counts_what_is_there_and_how_old_the_oldest_is() {
        let stored = summarise(
            &[
                entry(
                    "artifacts/rows/ab/x/data",
                    100,
                    Some(NOW - Duration::days(9)),
                ),
                entry(
                    "artifacts/rows/cd/y/data",
                    20,
                    Some(NOW - Duration::days(2)),
                ),
            ],
            NOW,
        );
        assert_eq!(
            stored.global,
            Stored {
                objects: 2,
                bytes: 120,
                oldest_days: Some(9),
            }
        );
        assert!(stored.projects.is_empty());
    }

    #[test]
    fn a_store_that_dates_nothing_reports_no_age_rather_than_a_zero() {
        // "Nothing here is older than today" and "no key reports a date" are
        // different states, and a zero says the first when the store meant the
        // second.
        let stored = summarise(&[entry("artifacts/rows/ab/x/data", 1, None)], NOW);
        assert_eq!(stored.global.oldest_days, None);
        assert!(
            !stored
                .samples(NOW)
                .iter()
                .any(|sample| sample.name.ends_with("oldest_days"))
        );
    }

    #[test]
    fn an_empty_prefix_is_a_measurement_and_not_an_absence() {
        // Zero objects is what a fresh installation reads, and the curve from
        // there is the whole point. It has to be reported.
        let stored = summarise(&[], NOW);
        let names: Vec<_> = stored
            .samples(NOW)
            .into_iter()
            .map(|sample| sample.name)
            .collect();
        assert_eq!(
            names,
            vec!["aiwatcher.artifacts.objects", "aiwatcher.artifacts.bytes"]
        );
    }

    fn scope() -> ProjectScope {
        ProjectScope {
            organization: OrganizationId::new(),
            project: ProjectId::new(),
        }
    }

    fn in_scope(scope: ProjectScope, tail: &str) -> String {
        format!("{}/{tail}", layout::prefix(Some(scope)))
    }

    #[test]
    fn a_projects_objects_are_never_counted_as_the_deployments() {
        // A tenant's storage reported as everybody's is the mild half. The
        // sharp half is a collector built one day on "nothing global names
        // this", which would then be permission to delete a project's bytes.
        let mine = scope();
        let yours = ProjectScope {
            project: ProjectId::new(),
            ..mine
        };
        let stored = summarise(
            &[
                entry("artifacts/rows/ab/x/data", 100, None),
                entry("artifacts/cache/aa.json", 10, None),
                entry(&in_scope(mine, "rows/ab/x/data"), 7, None),
                entry(&in_scope(mine, "cache/aa.json"), 3, None),
                entry(&in_scope(yours, "rows/ab/x/data"), 1, None),
            ],
            NOW,
        );
        assert_eq!(
            stored.global,
            Stored {
                objects: 2,
                bytes: 110,
                oldest_days: None
            }
        );
        assert_eq!(
            stored.project(mine),
            Some(&Stored {
                objects: 2,
                bytes: 10,
                oldest_days: None
            })
        );
        assert_eq!(
            stored.project(yours),
            Some(&Stored {
                objects: 1,
                bytes: 1,
                oldest_days: None
            })
        );
        assert_eq!(stored.unattributed, Stored::default());
        assert_eq!(stored.elsewhere, Stored::default());
    }

    #[test]
    fn every_object_lands_in_exactly_one_bucket_and_a_project_carries_its_name() {
        let mine = scope();
        let stored = summarise(
            &[
                entry("artifacts/rows/ab/x/data", 1, None),
                entry(&in_scope(mine, "rows/ab/x/data"), 2, None),
                entry(
                    "artifacts/scopes/not-a-uuid/nor-this/registry/rows",
                    4,
                    None,
                ),
                entry("prompts/heads/aa.json", 8, None),
            ],
            NOW,
        );
        let samples = stored.samples(NOW);
        let objects: Vec<_> = samples
            .iter()
            .filter(|sample| sample.name == "aiwatcher.artifacts.objects")
            .collect();
        assert_eq!(objects.len(), 3, "global, one project, and unattributed");
        assert_eq!(
            objects.iter().map(|sample| sample.value).sum::<f64>(),
            3.0,
            "the key from outside the prefix is in no total"
        );
        let project = objects
            .iter()
            .find(|sample| sample.attributes.iter().any(|(key, _)| key == "project"))
            .expect("a project's series");
        assert!(
            project
                .attributes
                .contains(&attr("organization", mine.organization.0.to_string()))
        );
        assert!(
            project
                .attributes
                .contains(&attr("prefix", layout::prefix(Some(mine))))
        );
        let unattributed = objects
            .iter()
            .find(|sample| {
                sample
                    .attributes
                    .contains(&attr("prefix", "artifacts/scopes"))
            })
            .expect("the unattributed series");
        assert!(
            !unattributed
                .attributes
                .iter()
                .any(|(key, _)| key == "project" || key == "organization"),
            "a bucket that exists to avoid inventing a name does not carry one"
        );
        assert_eq!(stored.elsewhere.objects, 1);
    }

    #[derive(Debug)]
    struct Unreachable;

    #[async_trait::async_trait]
    impl ObjectStore for Unreachable {
        async fn get(&self, _key: &str) -> aiwatcher_core::ports::PortResult<Option<Vec<u8>>> {
            Ok(None)
        }
        async fn put(&self, _key: &str, _bytes: Vec<u8>) -> aiwatcher_core::ports::PortResult<()> {
            Ok(())
        }
        async fn list(&self, _prefix: &str) -> aiwatcher_core::ports::PortResult<Vec<ObjectEntry>> {
            Err(aiwatcher_core::ports::PortError::Unavailable {
                target: "the object store",
                message: "a bad minute".to_owned(),
            })
        }
        async fn delete(&self, _key: &str) -> aiwatcher_core::ports::PortResult<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_prefix_that_could_not_be_walked_reports_nothing_rather_than_a_zero() {
        // A store that could not be asked and a store that holds nothing are
        // different facts. Reporting the second for the first draws a cliff in
        // every graph the moment a bucket has a bad minute — and, for a
        // project's series, reads as a project whose storage has gone.
        assert_eq!(walk(&Unreachable, NOW).await, None);
    }

    #[test]
    fn a_project_that_holds_nothing_is_absent_rather_than_zero() {
        // A zero series would be a claim about a project this walk found no
        // trace of, and this walk cannot know which projects exist.
        let stored = summarise(&[entry("artifacts/rows/ab/x/data", 1, None)], NOW);
        assert!(stored.projects.is_empty());
        assert_eq!(stored.samples(NOW).len(), 2);
    }
}
