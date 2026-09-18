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
//! A project-bound store puts both families below
//! `artifacts/scopes/<org>/<project>/registry/`. The scope is outside the bytes
//! and their digest. Reads require an exact canonical data reference in this
//! store's namespace; knowing another namespace's URI is not access to it.
//! This is a library boundary, not a project execution dispatcher or IAM gate.
//!
//! What a key here looks like is [`aiwatcher_execution::artifact::layout`], and
//! it is shared rather than restated: the catalog writes the `manifest.json`
//! beside the `data` this module writes, and two spellings of one rule is a
//! manifest under one prefix describing bytes under another.
//!
//! [`ProjectArtifacts`] is how a caller gets both halves for one project at
//! once, because a scoped byte store beside a deployment-wide catalog would
//! record a project's outputs where anybody can read them.
//!
//! [`ActivityExecutor::lookup`]: aiwatcher_execution::ActivityExecutor::lookup

use std::collections::BTreeMap;
use std::sync::Arc;

use aiwatcher_core::ports::{PortError, PortResult};
use aiwatcher_core::prompts::ObjectStore;
use aiwatcher_core::{ArtifactKind, ArtifactRef};
use aiwatcher_execution::artifact::layout;
use aiwatcher_execution::{
    ActivityError, ArtifactCatalog, FailureClass, ObjectArtifactCatalog, StoreError,
};
use aiwatcher_iam::ProjectScope;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The key prefix every step result lives under, beside the five registries.
pub const PREFIX: &str = layout::PREFIX;

/// The scheme an artifact stored here is named by.
///
/// Not `s3://` and not `file://`: which of those it really is depends on
/// `AIWATCHER_PROMPT_STORE`, and a URI that named the endpoint would be a plan
/// carrying a host. What this says is "resolve me through the object store this
/// deployment configured", which is the only thing a reader may do with it.
pub const SCHEME: &str = layout::SCHEME;

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
    scope: Option<ProjectScope>,
    prefix: String,
}

impl Artifacts {
    #[must_use]
    pub fn new(store: Arc<dyn ObjectStore>) -> Self {
        Self {
            store,
            scope: None,
            prefix: layout::prefix(None),
        }
    }

    /// Bind the byte store and attempt receipts to one project, without changing
    /// content digests. This is storage isolation, not IAM authorization: callers
    /// must obtain the scope from trusted execution ownership and check grants.
    /// No production dispatcher uses this yet.
    ///
    /// # Errors
    ///
    /// Policy when rebinding an already scoped store to a different project.
    pub fn for_project(&self, scope: ProjectScope) -> Result<Self, ActivityError> {
        if let Some(current) = self.scope {
            return if current == scope {
                Ok(self.clone())
            } else {
                Err(scope_error())
            };
        }
        Ok(Self {
            store: Arc::clone(&self.store),
            scope: Some(scope),
            prefix: layout::prefix(Some(scope)),
        })
    }

    /// The prefix this store writes and reads under.
    ///
    /// Read by [`ProjectArtifacts`], so that pairing a store with the catalog
    /// that describes it is a comparison rather than an assumption.
    #[must_use]
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    #[must_use]
    pub const fn project_scope(&self) -> Option<ProjectScope> {
        self.scope
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
        self.put_spelled_rows(name, body).await
    }

    /// Store a table's bytes as they are — rows a caller spelled with an
    /// integer a parsed row would round — named by their own hash.
    ///
    /// # Errors
    ///
    /// [`ActivityError`] carrying the class that decides whether to retry.
    pub async fn put_spelled_rows(
        &self,
        name: &str,
        body: Vec<u8>,
    ) -> Result<ArtifactRef, ActivityError> {
        let digest = aiwatcher_jobs::digest(&body);
        let key = layout::data_key(&self.prefix, ArtifactKind::Rows, &digest);
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
        let key = layout::data_key(&self.prefix, ArtifactKind::Log, &digest);
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

    /// Read the bytes back, and check that they are the ones the pointer names.
    ///
    /// Verified rather than trusted, on the way out as well as in: this is the
    /// one corruption no metric downstream detects — a step reading somebody
    /// else's table and succeeding. A pod's log is read through here too, and
    /// gets the same check for a duller reason: an object that does not hash
    /// to its name is not this attempt's output, whatever it holds.
    ///
    /// # Errors
    ///
    /// [`ActivityError`] when the object is missing, unreadable, or does not
    /// hash to what the reference claims.
    pub async fn read_bytes(&self, artifact: &ArtifactRef) -> Result<Vec<u8>, ActivityError> {
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
        Ok(bytes)
    }

    /// Read rows back, verified the same way and then parsed.
    ///
    /// # Errors
    ///
    /// [`ActivityError`] from [`read_bytes`](Self::read_bytes), or user code
    /// when the verified bytes are not a table.
    pub async fn read_rows(&self, artifact: &ArtifactRef) -> Result<Rows, ActivityError> {
        let bytes = self.read_bytes(artifact).await?;
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
        self.reference_key(&receipt.artifact)?;
        let body = serde_json::to_vec(receipt).map_err(|error| {
            ActivityError::user_code(format!("the receipt does not encode: {error}"))
        })?;
        self.store
            .put(&receipt_key(&self.prefix, &receipt.idempotency_key), body)
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
            .get(&receipt_key(&self.prefix, idempotency_key))
            .await
            .map_err(store_error)?
        else {
            return Ok(None);
        };
        // A receipt this build cannot read is treated as absent rather than as
        // an error: the work is repeatable, and refusing to run because of a
        // note about a previous run would be the note taking the run down.
        let Ok(receipt) = serde_json::from_slice::<Receipt>(&bytes) else {
            return Ok(None);
        };
        if receipt.idempotency_key != idempotency_key {
            return Err(scope_error());
        }
        self.reference_key(&receipt.artifact)?;
        Ok(Some(receipt))
    }

    async fn get(&self, artifact: &ArtifactRef) -> Result<Option<Vec<u8>>, ActivityError> {
        let key = self.reference_key(artifact)?;
        self.store.get(key).await.map_err(store_error)
    }

    fn reference_key<'a>(&self, artifact: &'a ArtifactRef) -> Result<&'a str, ActivityError> {
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
        // Exact canonical keys, not a prefix check: traversal, encoded paths,
        // receipts and another registry must never become artifact bytes. The
        // legacy reader also cannot open project bytes, even knowing their URI.
        if !layout::addresses(&self.prefix, artifact) {
            return Err(scope_error());
        }
        Ok(key)
    }
}

fn scope_error() -> ActivityError {
    ActivityError::new(
        FailureClass::Policy,
        "artifact or receipt does not belong to this storage scope",
    )
}

/// The idempotency key hashed, because its parts are caller strings.
///
/// A step id comes from a canvas and an execution id from a request. Neither is
/// checked against a path grammar anywhere, and a key is a path in the file
/// adapter.
fn receipt_key(prefix: &str, idempotency_key: &str) -> String {
    format!(
        "{prefix}/receipts/{}.json",
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

/// The two halves of one project's artifact storage, bound together.
///
/// A step produces bytes and a note about them, and the two are written by
/// different code in different crates: [`Artifacts`] stores the object, the
/// [`ArtifactCatalog`] stores the manifest, the lineage pointer and the cache
/// entry that finds it again. Handed out separately they can be bound to
/// different scopes, and the failure that follows is silent in the direction
/// that matters — a project's outputs described in the deployment-wide index,
/// where a cache lookup for anybody answers with them.
///
/// So they are constructed together, from one scope, and this is the door that
/// hands out both. [`Self::bind`] checks the two prefixes agree afterwards
/// rather than trusting that it passed the same scope twice. Each half can
/// still be bound on its own, so this is the shape a caller is given rather
/// than a wall around the parts.
///
/// # What this is not
///
/// **It authorizes nothing.** No grant, lease or principal is read here, and
/// holding one of these is not permission to use it. A caller takes the scope
/// from trusted durable execution ownership — never a plan, a parameter, a
/// worker's name or a definition's author — and checks the grant *before* it
/// asks this pair anything, a cache lookup included: a hit is an answer about a
/// project's data whether or not any work follows. Nothing constructs one on a
/// production path.
#[derive(Clone, Debug)]
pub struct ProjectArtifacts {
    artifacts: Artifacts,
    catalog: Arc<dyn ArtifactCatalog>,
    scope: ProjectScope,
}

impl ProjectArtifacts {
    /// The byte store and the catalog for one project, over one object store.
    ///
    /// # Errors
    ///
    /// [`ActivityError`] of class `Policy` when either half refuses the scope,
    /// or when the two do not agree on the prefix they would write under —
    /// which is not reachable today and is checked because the alternative is
    /// noticing it as an artifact nobody can open.
    pub fn bind(store: &Arc<dyn ObjectStore>, scope: ProjectScope) -> Result<Self, ActivityError> {
        let artifacts = Artifacts::new(Arc::clone(store)).for_project(scope)?;
        let catalog = ObjectArtifactCatalog::new(Arc::clone(store))
            .for_project(scope)
            .map_err(|error: StoreError| {
                ActivityError::new(FailureClass::Policy, error.to_string())
            })?;
        if catalog.prefix() != artifacts.prefix() {
            return Err(scope_error());
        }
        Ok(Self {
            artifacts,
            catalog: Arc::new(catalog) as Arc<dyn ArtifactCatalog>,
            scope,
        })
    }

    /// Where this project's bytes go.
    #[must_use]
    pub const fn artifacts(&self) -> &Artifacts {
        &self.artifacts
    }

    /// What describes them, and what a cache key resolves to.
    #[must_use]
    pub const fn catalog(&self) -> &Arc<dyn ArtifactCatalog> {
        &self.catalog
    }

    /// The project both halves are bound to.
    #[must_use]
    pub const fn scope(&self) -> ProjectScope {
        self.scope
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

    async fn read_bytes(&self, artifact: &ArtifactRef) -> PortResult<Vec<u8>> {
        Artifacts::read_bytes(self, artifact)
            .await
            .map_err(as_port_error)
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

    async fn put_rows_as_spelled(&self, name: &str, spelled: String) -> PortResult<ArtifactRef> {
        let rows: Vec<Value> =
            serde_json::from_str(&spelled).map_err(|error| PortError::Rejected {
                target: TARGET,
                message: format!("the rows are not a JSON array: {error}"),
            })?;
        if let Some(other) = rows.iter().find(|row| !row.is_object()) {
            return Err(PortError::Rejected {
                target: TARGET,
                message: format!(
                    "a row has to be an object with named columns, and this one is {}",
                    kind_of(other)
                ),
            });
        }
        Artifacts::put_spelled_rows(self, name, spelled.into_bytes())
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
    async fn rows_spelled_with_a_wide_integer_are_kept_digit_for_digit_and_still_have_to_be_rows() {
        use aiwatcher_core::ports::AttemptArtifacts;
        let artifacts = artifacts();
        let spelled = r#"[{"case_id": "c1", "answer": 123456789012345678901234567890}]"#;
        let stored =
            AttemptArtifacts::put_rows_as_spelled(&artifacts, "answers", spelled.to_owned())
                .await
                .expect("a put");
        let bytes = artifacts.read_bytes(&stored).await.expect("a read");
        assert_eq!(bytes, spelled.as_bytes());
        assert_eq!(stored.digest, aiwatcher_jobs::digest(spelled.as_bytes()));

        let refused =
            AttemptArtifacts::put_rows_as_spelled(&artifacts, "answers", "[1]".to_owned()).await;
        assert!(
            matches!(refused, Err(PortError::Rejected { .. })),
            "{refused:?}"
        );
    }

    #[tokio::test]
    async fn a_pointer_to_bytes_that_are_not_there_is_infrastructure_and_not_user_code() {
        // The plan was fine and the query was fine. Calling it user code would
        // send somebody to read a Flow script that has nothing wrong with it.
        let artifacts = artifacts();
        let digest = "ab".repeat(32);
        let missing = ArtifactRef::new(
            "rows",
            format!("object://artifacts/rows/ab/{digest}/data"),
            digest,
        )
        .of_kind(ArtifactKind::Rows);
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
        let store = Arc::new(MemoryObjectStore::new());
        let artifacts = Artifacts::new(store.clone());
        let stored = artifacts.put_rows("rows", &rows()).await.expect("a put");
        store
            .put(stored.uri.strip_prefix(SCHEME).unwrap(), b"[]".to_vec())
            .await
            .expect("corrupt the stored bytes");
        let error = artifacts
            .read_rows(&stored)
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
