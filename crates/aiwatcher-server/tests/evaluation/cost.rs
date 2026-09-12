//! What a read of durable evidence costs, counted rather than estimated.
//!
//! The four questions the FTI plan asks — a summary, a page of cases, a page
//! of the catalogue and one sweep — measured in object-store operations at the
//! instance's starting limits. Operations rather than milliseconds: this is a
//! store on the other side of a network, so a request is the unit that scales,
//! and a wall clock here would measure a `BTreeMap`.
use super::*;
use aiwatcher_prompts::adapters::memory::MemoryObjectStore;
use std::sync::atomic::AtomicUsize;

#[derive(Debug, Default)]
pub(super) struct Counts {
    gets: AtomicUsize,
    lists: AtomicUsize,
    writes: AtomicUsize,
    bytes: AtomicUsize,
}
impl Counts {
    fn reset(&self) {
        for counter in [&self.gets, &self.lists, &self.writes, &self.bytes] {
            counter.store(0, Ordering::SeqCst);
        }
    }
    fn read(&self) -> (usize, usize, usize, usize) {
        (
            self.gets.load(Ordering::SeqCst),
            self.lists.load(Ordering::SeqCst),
            self.writes.load(Ordering::SeqCst),
            self.bytes.load(Ordering::SeqCst),
        )
    }
}

#[derive(Debug)]
pub(super) struct Counting {
    inner: Arc<dyn ObjectStore>,
    pub(super) counts: Arc<Counts>,
}
#[async_trait]
impl ObjectStore for Counting {
    async fn put(&self, key: &str, value: Vec<u8>) -> PortResult<()> {
        self.counts.writes.fetch_add(1, Ordering::SeqCst);
        self.inner.put(key, value).await
    }
    async fn create(&self, key: &str, value: Vec<u8>) -> PortResult<bool> {
        self.counts.writes.fetch_add(1, Ordering::SeqCst);
        self.inner.create(key, value).await
    }
    async fn get(&self, key: &str) -> PortResult<Option<Vec<u8>>> {
        self.counts.gets.fetch_add(1, Ordering::SeqCst);
        let found = self.inner.get(key).await?;
        self.counts
            .bytes
            .fetch_add(found.as_ref().map_or(0, Vec::len), Ordering::SeqCst);
        found.map_or(Ok(None), |bytes| Ok(Some(bytes)))
    }
    async fn list(&self, prefix: &str) -> PortResult<Vec<ObjectEntry>> {
        self.counts.lists.fetch_add(1, Ordering::SeqCst);
        self.inner.list(prefix).await
    }
    async fn delete(&self, key: &str) -> PortResult<()> {
        self.counts.writes.fetch_add(1, Ordering::SeqCst);
        self.inner.delete(key).await
    }
}

fn counting() -> (Arc<dyn ObjectStore>, Arc<Counts>) {
    let counts = Arc::new(Counts::default());
    (
        Arc::new(Counting {
            inner: Arc::new(MemoryObjectStore::new()),
            counts: counts.clone(),
        }),
        counts,
    )
}

/// The instance's starting limits: 10 000 selected cases, pages of 200.
const CASES: u64 = 10_000;
const CATALOGUE: usize = 50;

#[tokio::test]
async fn a_summary_and_a_page_cost_what_they_read_rather_than_the_whole_result() {
    let (store, counts) = counting();
    let registry = registry(store, Arc::new(Source::default()));
    let receipt = publish(&registry, request("big", CASES), "editor", 100)
        .await
        .unwrap();

    counts.reset();
    let detail = registry.get("big", "viewer", 200).await.unwrap().unwrap();
    let (gets, lists, _, bytes) = counts.read();
    assert_eq!(detail.state, EvidenceState::Complete);
    assert_eq!(detail.counts.unwrap().scored, CASES as usize);
    println!("summary of {CASES} cases: {gets} gets, {lists} lists, {bytes} bytes");
    // Claim, tombstone, approval record, withdrawal marker, metadata. The fifty
    // shards behind it are read when somebody reads them.
    assert!(
        gets <= 6,
        "a summary must not scale with the number of shards: {gets} gets"
    );
    assert!(bytes < 1_000_000, "{bytes} bytes for a header");

    counts.reset();
    let page = registry
        .cases("big", &receipt.version, None, Some(200), "viewer", 200)
        .await
        .unwrap()
        .unwrap();
    let (gets, lists, _, bytes) = counts.read();
    assert_eq!(page.cases.len(), 200);
    println!("first page of 200: {gets} gets, {lists} lists, {bytes} bytes");
    // The summary's reads, plus exactly the two shards this page is made of.
    assert!(
        gets <= 8,
        "a page must read its own shards and no others: {gets} gets"
    );

    counts.reset();
    let last = registry
        .cases(
            "big",
            &receipt.version,
            Some(&format!("{}:{}", receipt.version, CASES - 200)),
            Some(200),
            "viewer",
            200,
        )
        .await
        .unwrap()
        .unwrap();
    let (gets, ..) = counts.read();
    assert_eq!(last.cases.len(), 200);
    assert!(
        gets <= 8,
        "the last page costs what the first one does: {gets} gets"
    );
}

#[tokio::test]
async fn a_catalogue_page_and_a_sweep_do_not_reread_every_published_result() {
    let (store, counts) = counting();
    let registry = registry(store, Arc::new(Source::default()));
    for n in 0..CATALOGUE {
        publish(&registry, request(&format!("row-{n:03}"), 3), "editor", 100)
            .await
            .unwrap();
    }

    counts.reset();
    let page = registry
        .list(None, 200, None, None, "viewer", 200)
        .await
        .unwrap();
    let (gets, lists, _, bytes) = counts.read();
    assert_eq!(page.evaluations.len(), CATALOGUE);
    println!("catalogue of {CATALOGUE}: {gets} gets, {lists} lists, {bytes} bytes");
    // The catalogue row and the header behind it. The claim and the tombstone
    // are what the index replaced: the row it holds is written by the
    // publication that won the gate and marked by the retirement, so a page
    // asks neither — and it lists one key per result rather than every object
    // under `evaluations/`, which is what the threshold was about. ADR_0030.
    assert!(
        gets < 3 * CATALOGUE,
        "a row must cost a header, not a result: {gets} gets"
    );
    assert_eq!(lists, 1, "one listing, of the catalogue and nothing else");

    counts.reset();
    assert_eq!(registry.sweep("retention-worker", 200).await.unwrap(), 0);
    assert!(registry.retention().await.unwrap().is_none());
    let (gets, lists, writes, bytes) = counts.read();
    println!(
        "one sweep of {CATALOGUE}: {gets} gets, {lists} lists, {writes} writes, {bytes} bytes"
    );
    assert_eq!(writes, 0, "a sweep with nothing to retire writes nothing");
    assert!(
        gets <= 3 * CATALOGUE,
        "retention reads deadlines, not results: {gets} gets"
    );
}
