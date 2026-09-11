//! Neutral byte storage; each domain owns its keys and persistence rules.

use async_trait::async_trait;
use time::OffsetDateTime;

use crate::ports::PortResult;

/// One object in the store.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectEntry {
    pub key: String,
    pub size: u64,
    pub last_modified: Option<OffsetDateTime>,
}

/// Bytes, by key.
///
/// Each domain owns its key layout, immutable versions and indexes. Adapters
/// implement only bytes, so moving a registry between a directory and a bucket
/// does not move its rules. ADR_0030 preserves the historical reexports.
///
/// The contract is S3's, because that is what the production implementation
/// is: `put` overwrites, `get` returns `None` for a missing key rather than an
/// error, `list` is prefix-scoped and returns every match, and `delete` on a
/// missing key succeeds.
#[async_trait]
pub trait ObjectStore: Send + Sync + std::fmt::Debug {
    async fn put(&self, key: &str, body: Vec<u8>) -> PortResult<()>;

    /// Atomically publish a complete object only when the key is absent.
    /// `false` means another complete object already owns the key. A transport
    /// failure is ambiguous: retry with the same bytes, then read the winner.
    /// Adapters without this capability must refuse, never emulate read/put.
    async fn create(&self, _key: &str, _body: Vec<u8>) -> PortResult<bool> {
        Err(crate::ports::PortError::Rejected {
            target: "object-store",
            message: "atomic object creation is not supported".into(),
        })
    }

    async fn get(&self, key: &str) -> PortResult<Option<Vec<u8>>>;

    async fn list(&self, prefix: &str) -> PortResult<Vec<ObjectEntry>>;

    async fn delete(&self, key: &str) -> PortResult<()>;
}
