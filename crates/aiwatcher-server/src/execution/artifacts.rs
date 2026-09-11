//! Where a step's result goes, and how the next attempt finds it again.
//!
//! An [`ArtifactRef`] is a pointer with a digest; the bytes belong to the
//! object store the registries already write through, under a sixth prefix
//! rather than a sixth registry — an artifact has no head, no labels and no
//! list. It is bytes named by their own hash.
//!
//! ```text
//! artifacts/<kind>/<first two hex>/<sha256>/data        the bytes
//! artifacts/receipts/<sha256 of the idempotency key>    what one attempt produced
//! ```
//!
//! The receipt exists because a timeout does not prove a remote service did not
//! finish. [`ActivityExecutor::lookup`] asks the runtime whether it ran that
//! key — the Flow service can answer that, and deliberately keeps no rows. "It
//! ran" and "here is what it produced" are two questions; this holds the second.
//!
//! The receipt is written **after** the data ([`aiwatcher_jobs::ORDERING`]). A
//! crash the right way round leaves bytes nothing points at, which the next
//! attempt overwrites identically; the wrong way round leaves a completed step
//! whose artifact 404s.
//!
//! [`ActivityExecutor::lookup`]: aiwatcher_execution::ActivityExecutor::lookup

use std::collections::BTreeMap;
use std::sync::Arc;

use aiwatcher_core::ports::{PortError, PortResult};
use aiwatcher_core::prompts::ObjectStore;
use aiwatcher_core::{ArtifactKind, ArtifactRef};
use aiwatcher_execution::{ActivityError, FailureClass};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The key prefix every step result lives under, beside the five registries.
pub const PREFIX: &str = "artifacts";

/// The scheme an artifact stored here is named by.
///
/// Not `s3://` and not `file://`: which of those it really is depends on
/// `AIWATCHER_PROMPT_STORE`, and a URI that named the endpoint would be a plan
/// carrying a host. What this says is "resolve me through the object store this
/// deployment configured", which is the only thing a reader may do with it.
pub const SCHEME: &str = "object://";

/// One table of rows, as a step hands it on.
pub type Rows = Vec<BTreeMap<String, Value>>;

/// What one finished attempt produced, so the next one need not repeat it.
///
/// Keyed by the idempotency key rather than by content: the question it answers
/// is "did *this attempt* finish", and the content is what the answer is.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Receipt {
    /// `<execution>/<step>/<attempt>`, verbatim. Stored as well as hashed into
    /// the key so a receipt read out of the bucket says what it is about.
    pub idempotency_key: String,
    pub artifact: ArtifactRef,
    pub rows: usize,
    /// What the runtime said its result hashed to, in the runtime's own
    /// encoding.
    ///
    /// Never compared against [`ArtifactRef::digest`] — they are digests of two
    /// different encodings by two different languages, and making them agree
    /// byte for byte would be a cross-language contract nobody could keep. What
    /// it is for is the one check that matters: a runtime answering `done` with
    /// a digest this receipt does not name is describing a *different* run of
    /// this key, and then the stored bytes are not that run's.
    pub runtime_digest: String,
    #[serde(with = "time::serde::rfc3339")]
    pub stored_at: time::OffsetDateTime,
}

/// The object store a reactor writes results through.
#[derive(Clone, Debug)]
pub struct Artifacts {
    store: Arc<dyn ObjectStore>,
}

impl Artifacts {
    #[must_use]
    pub fn new(store: Arc<dyn ObjectStore>) -> Self {
        Self { store }
    }

    /// Store `rows` and hand back the pointer to them.
    ///
    /// The digest is of the bytes this function wrote, computed here and never
    /// taken from a caller — the prompt registry's rule, for the same reason: a
    /// content address supplied by somebody else lets two different results
    /// occupy one key.
    ///
    /// # Errors
    ///
    /// [`ActivityError`] carrying the class that decides whether to retry.
    pub async fn put_rows(&self, name: &str, rows: &Rows) -> Result<ArtifactRef, ActivityError> {
        let body = serde_json::to_vec(rows).map_err(|error| {
            ActivityError::user_code(format!("the rows do not encode: {error}"))
        })?;
        let digest = aiwatcher_jobs::digest(&body);
        let key = data_key(ArtifactKind::Rows, &digest);
        let size = body.len() as u64;
        self.store.put(&key, body).await.map_err(store_error)?;
        Ok(ArtifactRef {
            name: name.to_owned(),
            uri: format!("{SCHEME}{key}"),
            digest,
            size_bytes: Some(size),
            content_type: "application/json".to_owned(),
            kind: ArtifactKind::Rows,
            schema_ref: None,
        })
    }

    /// Store what a pod printed and hand back the pointer to it.
    ///
    /// Named by its own hash like every other artifact here, which is what
    /// makes reading one pod's log twice store it once — the launcher keeps
    /// the Job when a store fails, so the next pass reads the same bytes
    /// (ADR_0029).
    ///
    /// # Errors
    ///
    /// [`ActivityError`] when the store refused the bytes.
    pub async fn put_log(&self, body: &[u8]) -> Result<ArtifactRef, ActivityError> {
        let digest = aiwatcher_jobs::digest(body);
        let key = data_key(ArtifactKind::Log, &digest);
        self.store
            .put(&key, body.to_vec())
            .await
            .map_err(store_error)?;
        Ok(ArtifactRef {
            name: "log".to_owned(),
            uri: format!("{SCHEME}{key}"),
            digest,
            size_bytes: Some(body.len() as u64),
            content_type: "text/plain; charset=utf-8".to_owned(),
            kind: ArtifactKind::Log,
            schema_ref: None,
        })
    }

    /// Read rows back, and check that they are the ones the pointer names.
    ///
    /// Verified rather than trusted, on the way out as well as in: this is the
    /// one corruption no metric downstream detects — a step reading somebody
    /// else's table and succeeding.
    ///
    /// # Errors
    ///
    /// [`ActivityError`] when the object is missing, unreadable, or does not
    /// hash to what the reference claims.
    pub async fn read_rows(&self, artifact: &ArtifactRef) -> Result<Rows, ActivityError> {
        let bytes = self.get(artifact).await?.ok_or_else(|| {
            // Infrastructure, not user code: the plan and the query were fine
            // and the bytes are gone. Retrying is worth it — the object store
            // may simply have been unreachable a moment ago.
            ActivityError::new(
                FailureClass::Infrastructure,
                format!("{} holds no object", artifact.uri),
            )
        })?;
        let found = aiwatcher_jobs::digest(&bytes);
        if found != artifact.digest {
            return Err(ActivityError::new(
                FailureClass::Infrastructure,
                format!(
                    "{} holds bytes that hash to {found}, not to the {} it is named by",
                    artifact.uri, artifact.digest
                ),
            ));
        }
        serde_json::from_slice(&bytes).map_err(|error| {
            ActivityError::user_code(format!("{} does not hold a table: {error}", artifact.uri))
        })
    }

    /// Whether the bytes a reference names are actually there.
    ///
    /// # Errors
    ///
    /// [`ActivityError`] when the store could not be asked.
    pub async fn holds(&self, artifact: &ArtifactRef) -> Result<bool, ActivityError> {
        Ok(self.get(artifact).await?.is_some())
    }

    /// Record what an attempt produced. Written **after** the data.
    ///
    /// # Errors
    ///
    /// [`ActivityError`] when the store refused it.
    pub async fn put_receipt(&self, receipt: &Receipt) -> Result<(), ActivityError> {
        let body = serde_json::to_vec(receipt).map_err(|error| {
            ActivityError::user_code(format!("the receipt does not encode: {error}"))
        })?;
        self.store
            .put(&receipt_key(&receipt.idempotency_key), body)
            .await
            .map_err(store_error)
    }

    /// What an earlier attempt at this key produced, if it got that far.
    ///
    /// # Errors
    ///
    /// [`ActivityError`] when the store could not be asked. A receipt that is
    /// simply not there is `None`, which is the ordinary answer.
    pub async fn receipt(&self, idempotency_key: &str) -> Result<Option<Receipt>, ActivityError> {
        let Some(bytes) = self
            .store
            .get(&receipt_key(idempotency_key))
            .await
            .map_err(store_error)?
        else {
            return Ok(None);
        };
        // A receipt this build cannot read is treated as absent rather than as
        // an error: the work is repeatable, and refusing to run because of a
        // note about a previous run would be the note taking the run down.
        Ok(serde_json::from_slice(&bytes).ok())
    }

    async fn get(&self, artifact: &ArtifactRef) -> Result<Option<Vec<u8>>, ActivityError> {
        let key = artifact.uri.strip_prefix(SCHEME).ok_or_else(|| {
            // A `file://` or an `s3://` from somewhere else is a pointer this
            // process cannot verify. One is accepted as a *uri* and never as
            // the only reference; reading one here would be reading bytes
            // nobody in this deployment addressed.
            ActivityError::user_code(format!(
                "{} is not an artifact in this deployment's object store",
                artifact.uri
            ))
        })?;
        self.store.get(key).await.map_err(store_error)
    }
}

/// `artifacts/<kind>/<first two hex>/<sha256>/data`.
///
/// The two-character shard is what keeps a bucket listing usable at a million
/// objects; the key is immutable, and a human name for the same bytes is a
/// dataset version pointing at this digest, never this key's identity.
fn data_key(kind: ArtifactKind, digest: &str) -> String {
    format!(
        "{PREFIX}/{}/{}/{digest}/data",
        kind.as_str(),
        digest.get(..2).unwrap_or("00")
    )
}

/// The idempotency key hashed, because its parts are caller strings.
///
/// A step id comes from a canvas and an execution id from a request. Neither is
/// checked against a path grammar anywhere, and a key is a path in the file
/// adapter.
fn receipt_key(idempotency_key: &str) -> String {
    format!(
        "{PREFIX}/receipts/{}.json",
        aiwatcher_jobs::digest(idempotency_key.as_bytes())
    )
}

/// An object store's failure, as the class that decides whether to retry.
///
/// The same split `PortError` already makes, translated once: `Unavailable` is
/// worth coming back for, and a refusal will refuse identically forever.
fn store_error(error: aiwatcher_core::ports::PortError) -> ActivityError {
    if error.is_retryable() {
        ActivityError::transient(error.to_string())
    } else {
        ActivityError::new(FailureClass::Infrastructure, error.to_string())
    }
}

/// How much of a result stays inline on the completion event.
///
/// Far below `MAX_INLINE_RESULT_BYTES`, and deliberately: what rides the
/// workflow stream is a *control value* somebody reads on a canvas, and the
/// rows are one `object://` away. A preview sized at the message limit would
/// be a stream that grows with the corpus.
pub const PREVIEW_ROWS: usize = 5;
pub const PREVIEW_BYTES: usize = 8 * 1024;

/// The bounded control value that rides the completion event.
pub fn preview(columns: &[String], rows: &Rows, took_ms: Option<u64>) -> Value {
    let mut sample = Vec::new();
    let mut budget = PREVIEW_BYTES;
    for row in rows.iter().take(PREVIEW_ROWS) {
        let encoded = serde_json::to_value(row).unwrap_or(Value::Null);
        let size = encoded.to_string().len();
        if size > budget {
            break;
        }
        budget -= size;
        sample.push(encoded);
    }
    json!({
        "columns": columns,
        "rows": rows.len(),
        "preview": sample,
        "took_ms": took_ms,
    })
}

/// The worker's half of the same store.
///
/// A worker is a process somebody else operates, so it reads and writes through
/// the API rather than through this store directly — [`AttemptArtifacts`] is
/// that port and this is the only implementation of it. Everything it does is
/// [`Artifacts`]' own work with the row shape widened by one step: the port
/// speaks `Vec<Value>` because a JSON body is what crosses the wire, and this
/// is where that becomes the [`Rows`] every other reader expects.
///
/// A row that is not an object is refused rather than dropped. `Rows` is a
/// table, and a worker sending `[1, 2, 3]` has a bug worth being told about at
/// the moment it happens rather than three steps later when something tries to
/// group by a column.
#[async_trait::async_trait]
impl aiwatcher_core::ports::AttemptArtifacts for Artifacts {
    async fn read_rows(&self, artifact: &ArtifactRef) -> PortResult<Vec<Value>> {
        let rows = Artifacts::read_rows(self, artifact)
            .await
            .map_err(as_port_error)?;
        Ok(rows
            .into_iter()
            .map(|row| Value::Object(row.into_iter().collect()))
            .collect())
    }

    async fn put_rows(&self, name: &str, rows: Vec<Value>) -> PortResult<ArtifactRef> {
        let table: Rows = rows
            .into_iter()
            .map(|row| match row {
                Value::Object(map) => Ok(map.into_iter().collect()),
                other => Err(PortError::Rejected {
                    target: TARGET,
                    message: format!(
                        "a row has to be an object with named columns, and this one is {}",
                        kind_of(&other)
                    ),
                }),
            })
            .collect::<Result<_, _>>()?;
        Artifacts::put_rows(self, name, &table)
            .await
            .map_err(as_port_error)
    }

    async fn holds(&self, artifact: &ArtifactRef) -> PortResult<bool> {
        Artifacts::holds(self, artifact)
            .await
            .map_err(as_port_error)
    }
}

/// What this port calls the thing it could not reach.
const TARGET: &str = "the object store";

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// An activity failure as a port failure, keeping the retry decision.
///
/// The classes carry it already, so this reads rather than re-decides: an
/// unreachable store is `Unavailable` and worth repeating, and bytes that do
/// not hash to what they are named by are `Rejected` and will not hash
/// differently on the next call.
fn as_port_error(error: ActivityError) -> PortError {
    if error.class.is_retryable() {
        PortError::Unavailable {
            target: TARGET,
            message: error.message,
        }
    } else {
        PortError::Rejected {
            target: TARGET,
            message: error.message,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_preview_is_a_control_value_and_never_the_rows() {
        // What rides the workflow stream is what somebody reads on a canvas.
        // The rows are one `object://` away, and a preview sized at the
        // message limit would be a stream that grows with the corpus.
        let rows: Rows = (0..500)
            .map(|n| {
                std::collections::BTreeMap::from([(
                    "text".to_owned(),
                    Value::String("x".repeat(200) + &n.to_string()),
                )])
            })
            .collect();
        let preview = preview(&["text".to_owned()], &rows, Some(12));

        assert_eq!(preview["rows"], 500, "the count is the whole table's");
        assert_eq!(
            preview["preview"].as_array().expect("an array").len(),
            PREVIEW_ROWS,
            "and the sample is not"
        );
        assert!(preview.to_string().len() < PREVIEW_BYTES * 2);
    }

    #[test]
    fn a_preview_stops_at_its_byte_budget_before_its_row_budget() {
        let rows: Rows = (0..PREVIEW_ROWS)
            .map(|_| {
                std::collections::BTreeMap::from([(
                    "text".to_owned(),
                    Value::String("x".repeat(PREVIEW_BYTES)),
                )])
            })
            .collect();
        let preview = preview(&["text".to_owned()], &rows, None);
        assert!(
            preview["preview"].as_array().expect("an array").len() < PREVIEW_ROWS,
            "one row over the budget is one row too many"
        );
    }

    use aiwatcher_prompts::adapters::memory::MemoryObjectStore;

    use super::*;

    fn artifacts() -> Artifacts {
        Artifacts::new(Arc::new(MemoryObjectStore::new()))
    }

    fn rows() -> Rows {
        vec![BTreeMap::from([(
            "text".to_owned(),
            Value::String("hello".to_owned()),
        )])]
    }

    #[tokio::test]
    async fn two_attempts_over_the_same_rows_write_one_object() {
        // A content address, so a deterministic retry costs a `put` and not a
        // second copy of the table — and so a lineage does not fork where
        // nothing forked.
        let artifacts = artifacts();
        let first = artifacts.put_rows("rows", &rows()).await.expect("a put");
        let again = artifacts.put_rows("rows", &rows()).await.expect("a put");
        assert_eq!(first.uri, again.uri);
        assert_eq!(first.digest, again.digest);
        assert!(first.uri.starts_with("object://artifacts/rows/"));
        assert_eq!(artifacts.read_rows(&first).await.expect("a read"), rows());
    }

    #[tokio::test]
    async fn a_pointer_to_bytes_that_are_not_there_is_infrastructure_and_not_user_code() {
        // The plan was fine and the query was fine. Calling it user code would
        // send somebody to read a Flow script that has nothing wrong with it.
        let artifacts = artifacts();
        let missing = ArtifactRef::new("rows", "object://artifacts/rows/ab/abcd/data", "abcd");
        let error = artifacts
            .read_rows(&missing)
            .await
            .expect_err("an absent object");
        assert_eq!(error.class, FailureClass::Infrastructure);
        assert!(!artifacts.holds(&missing).await.expect("a probe"));
    }

    #[tokio::test]
    async fn bytes_that_do_not_hash_to_what_they_are_named_by_are_refused() {
        // The one corruption nothing downstream detects: a step reading
        // somebody else's table and succeeding.
        let artifacts = artifacts();
        let stored = artifacts.put_rows("rows", &rows()).await.expect("a put");
        let lying = ArtifactRef {
            digest: "00".repeat(32),
            ..stored
        };
        let error = artifacts
            .read_rows(&lying)
            .await
            .expect_err("a digest that does not match");
        assert!(error.message.contains("hash to"), "{error}");
    }

    #[tokio::test]
    async fn a_receipt_says_what_one_attempt_produced_and_nothing_about_another() {
        let artifacts = artifacts();
        let artifact = artifacts.put_rows("rows", &rows()).await.expect("a put");
        let receipt = Receipt {
            idempotency_key: "exec-1/read/1".to_owned(),
            artifact,
            rows: 1,
            runtime_digest: "flow-digest".to_owned(),
            stored_at: time::OffsetDateTime::UNIX_EPOCH,
        };
        artifacts.put_receipt(&receipt).await.expect("a receipt");

        let found = artifacts
            .receipt("exec-1/read/1")
            .await
            .expect("a read")
            .expect("the receipt");
        assert_eq!(found.runtime_digest, "flow-digest");
        assert_eq!(found.rows, 1);
        assert!(
            artifacts
                .receipt("exec-1/read/2")
                .await
                .expect("a read")
                .is_none(),
            "the next attempt has its own key and no receipt of its own"
        );
    }

    #[tokio::test]
    async fn an_artifact_from_somebody_elses_store_is_refused_rather_than_fetched() {
        // A `file://` on a shared volume is a pointer nothing outside that
        // node can verify, and reading one here would be reading
        // bytes nobody in this deployment addressed.
        let error = artifacts()
            .read_rows(&ArtifactRef::new("rows", "file:///tmp/rows.json", "ab"))
            .await
            .expect_err("a foreign pointer");
        assert!(error.message.contains("object store"), "{error}");
    }
}
