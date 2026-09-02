//! The queued import, over an in-memory store and a fake image source.
//!
//! Every test here is named after one of [plan.md]'s acceptance criteria for
//! scalable Hub ingestion, because that is what they are: a large import
//! resumes after a restart, its progress and its rejected rows are readable
//! without opening the artifact, the same pinned source reaches the same
//! version, no address outside the allowlist is fetched, and an interrupted
//! job never appears as a completed one.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use aiwatcher_annotations::images::import::{ImportRow, ImportSource};
use aiwatcher_annotations::imports::staging::{AppendRowsRequest, StageBatchRequest};
use aiwatcher_annotations::imports::{ImportJobRequest, RejectReason};
use aiwatcher_annotations::integrations::fetch::{FetchedImage, ImageSource};
use aiwatcher_annotations::{
    LabelClass, Registry, RightsEvidence, SaveProjectRequest, SplitRatios, UsageRights,
};
use aiwatcher_core::ports::{PortError, PortResult};
use aiwatcher_core::prompts::{ObjectEntry, ObjectStore};
use aiwatcher_prompts::adapters::memory::MemoryObjectStore;
use sha2::Digest as _;

/// A one-class schema. This crate ships no vocabulary and these tests are
/// about rows rather than about drawings, so the schema is the smallest thing
/// a project will accept.
fn classes() -> Vec<LabelClass> {
    use aiwatcher_annotations::GeometryKind;
    vec![LabelClass {
        name: "edge".to_owned(),
        geometry: GeometryKind::Polyline,
        color: "#334155".to_owned(),
        description: String::new(),
        attributes: Vec::new(),
        keypoints: Vec::new(),
        optional_keypoints: Vec::new(),
        links: Vec::new(),
        ignore: false,
        layer: 0,
    }]
}

/// A picture, as a PNG header the fetcher's own gates accept.
fn png(width: u32, height: u32, salt: u8) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend_from_slice(&[0, 0, 0, 13]);
    bytes.extend_from_slice(b"IHDR");
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.push(salt);
    bytes
}

/// A hub that answers for `https://images.test/<n>.png` and nothing else.
///
/// A fake rather than a mock: it implements the same port the real one does,
/// so a test that passes here is a test about the job rather than about what
/// somebody expected the job to call.
#[derive(Debug, Default)]
struct FakeImages {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl ImageSource for FakeImages {
    async fn fetch(&self, uri: &str, _expected: Option<&str>) -> Result<FetchedImage, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let Some(name) = uri.strip_prefix("https://images.test/") else {
            return Err(format!(
                "{uri} is not a host this instance may fetch from. Only a hub's own asset host \
                 is downloaded"
            ));
        };
        let salt: u8 = name
            .trim_end_matches(".png")
            .parse()
            .map_err(|_| format!("{uri} did not download: no such row"))?;
        let bytes = png(64, 48, salt);
        Ok(FetchedImage {
            digest: hex::encode(sha2::Sha256::digest(&bytes)),
            bytes,
            content_type: "image/png".to_owned(),
            width: 64,
            height: 48,
        })
    }
}

async fn seeded() -> (Registry, Arc<FakeImages>, String) {
    let images = Arc::new(FakeImages::default());
    let registry = Registry::new(Arc::new(MemoryObjectStore::new()), "annotations")
        .with_image_source(Arc::clone(&images) as Arc<dyn ImageSource>);
    let project = registry
        .save_project(SaveProjectRequest {
            name: "corpora/import".to_owned(),
            description: "A corpus with no domain in it".to_owned(),
            classes: classes(),
            splits: SplitRatios::default(),
            split_salt: "2026-09".to_owned(),
            split_overrides: BTreeMap::new(),
        })
        .await
        .unwrap();
    (registry, images, project.name)
}

fn source() -> ImportSource {
    ImportSource {
        hub: "huggingface".to_owned(),
        dataset_id: "someone/pictures".to_owned(),
        revision: "c0ffee".repeat(2),
        config: "default".to_owned(),
        split: "train".to_owned(),
        ..ImportSource::default()
    }
}

fn stage_request(project: &str) -> StageBatchRequest {
    StageBatchRequest {
        project: project.to_owned(),
        description: "a corpus".to_owned(),
        rights: UsageRights::Licensed {
            license: "CC BY 4.0".to_owned(),
            url: Some("https://example.invalid/licence".to_owned()),
        },
        evidence: RightsEvidence {
            primary_source_url: "https://example.invalid/paper".to_owned(),
            reviewed_by: "someone".to_owned(),
            reviewed_at: None,
            note: String::new(),
        },
        source: source(),
    }
}

/// `n` rows in `families` families, so a page is not accidentally singletons.
fn rows(from: u8, count: u8, families: u8) -> Vec<ImportRow> {
    (from..from + count)
        .map(|index| ImportRow {
            image_id: None,
            uri: format!("https://images.test/{index}.png"),
            width: 0,
            height: 0,
            group_id: format!("house-{}", index % families.max(1)),
            level: None,
            view: "plan".to_owned(),
            metadata: BTreeMap::new(),
        })
        .collect()
}

/// A store that refuses one write, then heals.
///
/// The only honest way to test a resume: the job has to actually stop between
/// registering a page and recording that it did, which is exactly the window
/// the shard-before-cursor ordering exists for.
#[derive(Debug)]
struct FlakyStore {
    inner: MemoryObjectStore,
    fail_on: std::sync::Mutex<Option<String>>,
}

impl FlakyStore {
    fn new() -> Self {
        Self {
            inner: MemoryObjectStore::new(),
            fail_on: std::sync::Mutex::new(None),
        }
    }

    fn fail_writes_matching(&self, needle: &str) {
        *self.fail_on.lock().unwrap() = Some(needle.to_owned());
    }

    fn heal(&self) {
        *self.fail_on.lock().unwrap() = None;
    }
}

#[async_trait::async_trait]
impl ObjectStore for FlakyStore {
    async fn put(&self, key: &str, body: Vec<u8>) -> PortResult<()> {
        let refuse = self
            .fail_on
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|needle| key.contains(needle));
        if refuse {
            return Err(PortError::Unavailable {
                target: "test-store",
                message: "the object store is having an afternoon".to_owned(),
            });
        }
        self.inner.put(key, body).await
    }

    async fn get(&self, key: &str) -> PortResult<Option<Vec<u8>>> {
        self.inner.get(key).await
    }

    async fn list(&self, prefix: &str) -> PortResult<Vec<ObjectEntry>> {
        self.inner.list(prefix).await
    }

    async fn delete(&self, key: &str) -> PortResult<()> {
        self.inner.delete(key).await
    }
}

#[tokio::test]
async fn an_import_killed_between_a_page_and_its_cursor_resumes_without_duplicating_a_row() {
    let store = Arc::new(FlakyStore::new());
    let images = Arc::new(FakeImages::default());
    let registry = Registry::new(Arc::clone(&store) as Arc<dyn ObjectStore>, "annotations")
        .with_image_source(Arc::clone(&images) as Arc<dyn ImageSource>);
    let project = registry
        .save_project(SaveProjectRequest {
            name: "corpora/import".to_owned(),
            description: String::new(),
            classes: classes(),
            splits: SplitRatios::default(),
            split_salt: "2026-09".to_owned(),
            split_overrides: BTreeMap::new(),
        })
        .await
        .unwrap()
        .name;

    let batch = registry
        .stage_import(stage_request(&project), "tester")
        .await
        .unwrap();
    for page in 0..3u8 {
        registry
            .append_import_rows(AppendRowsRequest {
                batch: batch.batch_id.clone(),
                page: Some(page as usize),
                rows: rows(page * 10, 10, 5),
            })
            .await
            .unwrap();
    }
    let job = registry
        .queue_import(
            ImportJobRequest {
                batch: batch.batch_id.clone(),
                dry_run: false,
            },
            "tester",
        )
        .await
        .unwrap();
    assert_eq!(job.pages, 3);
    assert_eq!(job.rows, 30);

    // The second page's result shard cannot be written. Its images register,
    // and the cursor never passes it.
    store.fail_writes_matching("/results/000001.jsonl");
    let parked = registry.run_import(&job.job_id, "worker-a").await.unwrap();
    assert_eq!(
        parked.state.as_str(),
        "queued",
        "an outage is worth retrying"
    );
    assert_eq!(
        parked.cursor, 1,
        "the cursor never passed the missing shard"
    );
    assert_eq!(
        parked.counts.accepted, 10,
        "a page that did not commit did not count"
    );

    store.heal();
    let done = registry.run_import(&job.job_id, "worker-b").await.unwrap();
    assert_eq!(done.state.as_str(), "completed");
    assert_eq!(done.cursor, 3);
    assert_eq!(
        done.counts.rows_considered, 30,
        "thirty rows were staged and thirty were considered, whatever the store did in between"
    );
    assert_eq!(done.counts.accepted, 30);
    assert_eq!(done.counts.rejected, 0);

    let registered = registry
        .images(&project, &Default::default(), 0, 200)
        .await
        .unwrap();
    assert_eq!(
        registered.images.len(),
        30,
        "an image id is the content address of its bytes, so re-doing a page is not a second copy"
    );
}

#[tokio::test]
async fn re_running_the_same_pinned_batch_reaches_the_same_version() {
    let (registry, _images, project) = seeded().await;
    let mut versions = Vec::new();
    for _ in 0..2 {
        let batch = registry
            .stage_import(stage_request(&project), "tester")
            .await
            .unwrap();
        registry
            .append_import_rows(AppendRowsRequest {
                batch: batch.batch_id.clone(),
                page: None,
                rows: rows(0, 6, 3),
            })
            .await
            .unwrap();
        let job = registry
            .queue_import(
                ImportJobRequest {
                    batch: batch.batch_id.clone(),
                    dry_run: false,
                },
                "tester",
            )
            .await
            .unwrap();
        let done = registry.run_import(&job.job_id, "worker").await.unwrap();
        versions.push(done.version.expect("a completed import has a version"));
    }
    assert_eq!(
        versions[0], versions[1],
        "the same rows on the same terms are the same import, whoever staged them"
    );
}

#[tokio::test]
async fn progress_and_rejected_rows_are_readable_without_opening_the_artifact() {
    let (registry, _images, project) = seeded().await;
    let batch = registry
        .stage_import(stage_request(&project), "tester")
        .await
        .unwrap();
    let mut page = rows(0, 4, 2);
    // Two rows nothing will fetch: one address nobody allowlisted, one that
    // the source has no row for.
    page.push(ImportRow {
        uri: "https://169.254.169.254/latest/meta-data/".to_owned(),
        ..page[0].clone()
    });
    page.push(ImportRow {
        uri: "https://images.test/not-a-number.png".to_owned(),
        ..page[0].clone()
    });
    registry
        .append_import_rows(AppendRowsRequest {
            batch: batch.batch_id.clone(),
            page: None,
            rows: page,
        })
        .await
        .unwrap();

    let job = registry
        .queue_import(
            ImportJobRequest {
                batch: batch.batch_id.clone(),
                dry_run: false,
            },
            "tester",
        )
        .await
        .unwrap();
    let done = registry.run_import(&job.job_id, "worker").await.unwrap();

    assert_eq!(done.counts.accepted, 4);
    assert_eq!(done.counts.rejected, 2);
    assert_eq!(done.progress(), Some(1.0));
    assert_eq!(done.rejects.get("address_refused"), Some(&1));
    assert_eq!(done.rejects.get("unreachable"), Some(&1));

    let refused = registry.import_rejects(&done.job_id, 0, 10).await.unwrap();
    assert_eq!(refused.total, 2);
    assert_eq!(refused.rows.len(), 2);
    assert!(refused.rows.iter().any(
        |row| row.reason == RejectReason::AddressRefused && row.uri.contains("169.254.169.254")
    ));
    assert!(
        refused.rows.iter().all(|row| !row.detail.is_empty()),
        "a count says how many; the sentence says what to fix"
    );
}

#[tokio::test]
async fn no_row_may_send_this_process_at_an_address_nobody_allowlisted() {
    let (registry, images, project) = seeded().await;
    let batch = registry
        .stage_import(stage_request(&project), "tester")
        .await
        .unwrap();
    registry
        .append_import_rows(AppendRowsRequest {
            batch: batch.batch_id.clone(),
            page: None,
            rows: vec![ImportRow {
                image_id: None,
                uri: "https://aiwatcher.internal/api/v1/events".to_owned(),
                width: 0,
                height: 0,
                group_id: "house-1".to_owned(),
                level: None,
                view: String::new(),
                metadata: BTreeMap::new(),
            }],
        })
        .await
        .unwrap();
    let job = registry
        .queue_import(
            ImportJobRequest {
                batch: batch.batch_id.clone(),
                dry_run: false,
            },
            "tester",
        )
        .await
        .unwrap();
    let done = registry.run_import(&job.job_id, "worker").await.unwrap();

    assert_eq!(done.counts.accepted, 0);
    assert_eq!(done.rejects.get("address_refused"), Some(&1));
    assert_eq!(
        images.calls.load(Ordering::SeqCst),
        1,
        "the port was asked and refused; what must not happen is a fetch nobody bounded"
    );
}

#[tokio::test]
async fn an_interrupted_import_never_appears_as_a_completed_one() {
    let (registry, _images, project) = seeded().await;
    let batch = registry
        .stage_import(stage_request(&project), "tester")
        .await
        .unwrap();
    for page in 0..2u8 {
        registry
            .append_import_rows(AppendRowsRequest {
                batch: batch.batch_id.clone(),
                page: Some(page as usize),
                rows: rows(page * 4, 4, 2),
            })
            .await
            .unwrap();
    }
    let job = registry
        .queue_import(
            ImportJobRequest {
                batch: batch.batch_id.clone(),
                dry_run: false,
            },
            "tester",
        )
        .await
        .unwrap();
    let cancelled = registry.cancel_import(&job.job_id).await.unwrap();
    assert_eq!(cancelled.state.as_str(), "cancelled");
    assert!(cancelled.version.is_none());

    let after = registry.run_import(&job.job_id, "worker").await.unwrap();
    assert_eq!(after.state.as_str(), "cancelled");
    assert!(after.version.is_none());
    assert!(
        registry.imports().await.unwrap().imports.is_empty(),
        "a cancelled job publishes nothing; the index is what a reader trusts"
    );
}

#[tokio::test]
async fn a_sealed_batch_takes_no_more_rows() {
    let (registry, _images, project) = seeded().await;
    let batch = registry
        .stage_import(stage_request(&project), "tester")
        .await
        .unwrap();
    registry
        .append_import_rows(AppendRowsRequest {
            batch: batch.batch_id.clone(),
            page: None,
            rows: rows(0, 2, 2),
        })
        .await
        .unwrap();
    registry
        .queue_import(
            ImportJobRequest {
                batch: batch.batch_id.clone(),
                dry_run: false,
            },
            "tester",
        )
        .await
        .unwrap();

    let error = registry
        .append_import_rows(AppendRowsRequest {
            batch: batch.batch_id.clone(),
            page: None,
            rows: rows(9, 1, 1),
        })
        .await
        .expect_err("a sealed batch is pinned");
    assert!(error.to_string().contains("sealed"), "{error}");
}

#[tokio::test]
async fn re_sending_a_page_is_a_retry_and_changing_one_is_a_refusal() {
    let (registry, _images, project) = seeded().await;
    let batch = registry
        .stage_import(stage_request(&project), "tester")
        .await
        .unwrap();
    let first = registry
        .append_import_rows(AppendRowsRequest {
            batch: batch.batch_id.clone(),
            page: Some(0),
            rows: rows(0, 3, 2),
        })
        .await
        .unwrap();
    assert!(first.created);

    let retried = registry
        .append_import_rows(AppendRowsRequest {
            batch: batch.batch_id.clone(),
            page: Some(0),
            rows: rows(0, 3, 2),
        })
        .await
        .unwrap();
    assert!(
        !retried.created,
        "identical bytes are an acknowledged retry"
    );
    assert_eq!(retried.total_rows, 3);

    let changed = registry
        .append_import_rows(AppendRowsRequest {
            batch: batch.batch_id.clone(),
            page: Some(0),
            rows: rows(5, 3, 2),
        })
        .await
        .expect_err("a page is immutable once written");
    assert!(changed.to_string().contains("different rows"), "{changed}");
}

#[tokio::test]
async fn a_batch_whose_every_page_is_singletons_says_so_before_anything_is_registered() {
    let (registry, _images, project) = seeded().await;
    let batch = registry
        .stage_import(stage_request(&project), "tester")
        .await
        .unwrap();
    for page in 0..2u8 {
        registry
            .append_import_rows(AppendRowsRequest {
                batch: batch.batch_id.clone(),
                page: Some(page as usize),
                // families == rows: the filename mapping.
                rows: rows(page * 4, 4, 255),
            })
            .await
            .unwrap();
    }
    let job = registry
        .queue_import(
            ImportJobRequest {
                batch: batch.batch_id.clone(),
                dry_run: true,
            },
            "tester",
        )
        .await
        .unwrap();
    assert!(
        job.warnings.iter().any(|line| line.contains("own family")),
        "{:?}",
        job.warnings
    );
}

#[tokio::test]
async fn a_dry_run_checks_every_row_and_publishes_nothing() {
    let (registry, _images, project) = seeded().await;
    let batch = registry
        .stage_import(stage_request(&project), "tester")
        .await
        .unwrap();
    registry
        .append_import_rows(AppendRowsRequest {
            batch: batch.batch_id.clone(),
            page: None,
            rows: rows(0, 5, 2),
        })
        .await
        .unwrap();
    let job = registry
        .queue_import(
            ImportJobRequest {
                batch: batch.batch_id.clone(),
                dry_run: true,
            },
            "tester",
        )
        .await
        .unwrap();
    let done = registry.run_import(&job.job_id, "worker").await.unwrap();

    assert_eq!(done.counts.accepted, 5);
    assert!(done.version.is_some(), "a dry run still has a receipt");
    assert!(
        registry.imports().await.unwrap().imports.is_empty(),
        "what it does not have is a published import"
    );
    assert_eq!(
        registry
            .images(&project, &Default::default(), 0, 100)
            .await
            .unwrap()
            .images
            .len(),
        0,
        "and it registered nothing"
    );
}

#[tokio::test]
async fn a_claim_the_curated_table_contradicts_is_refused_before_the_job_exists() {
    use aiwatcher_annotations::SourceUsage;

    let (registry, _images, project) = seeded().await;
    let batch = registry
        .stage_import(
            StageBatchRequest {
                rights: UsageRights::Licensed {
                    license: "MIT".to_owned(),
                    url: None,
                },
                source: ImportSource {
                    curated_source: Some("someone/pictures".to_owned()),
                    curated_usage: Some(SourceUsage::NonCommercial),
                    ..source()
                },
                ..stage_request(&project)
            },
            "tester",
        )
        .await
        .unwrap();
    registry
        .append_import_rows(AppendRowsRequest {
            batch: batch.batch_id.clone(),
            page: None,
            rows: rows(0, 2, 2),
        })
        .await
        .unwrap();

    let error = registry
        .queue_import(
            ImportJobRequest {
                batch: batch.batch_id.clone(),
                dry_run: false,
            },
            "tester",
        )
        .await
        .expect_err("a human read that licence at the source");
    assert!(
        registry.import_jobs().await.unwrap().jobs.is_empty(),
        "{error}"
    );
}
