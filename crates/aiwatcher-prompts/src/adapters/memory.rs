//! A bucket that is a `HashMap`.
//!
//! For tests, and for `AIWATCHER_PROMPT_STORE=memory` — which exists so the
//! panel's prompt tab can be driven end to end with `just dev`, where nothing
//! is meant to survive a restart anyway.
//!
//! It is also what pins the [`ObjectStore`] contract: the registry's tests run
//! against this, and `tests/rustfs.rs` runs the same assertions against a real
//! RustFS. Two implementations of one trait that are only ever exercised
//! separately are two implementations that quietly disagree.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use time::OffsetDateTime;
use tokio::sync::RwLock;

use aiwatcher_core::ports::PortResult;
use aiwatcher_core::prompts::{ObjectEntry, ObjectStore};

/// Bytes and when they were written.
type Object = (Vec<u8>, OffsetDateTime);

/// An in-process object store. Cloning shares the contents.
#[derive(Debug, Clone, Default)]
pub struct MemoryObjectStore {
    // Ordered so `list` returns keys the way S3 does — lexicographically —
    // rather than in whatever order a hash map happens to hold them. A test
    // that passes only under one iteration order is not a test.
    objects: Arc<RwLock<BTreeMap<String, Object>>>,
}

impl MemoryObjectStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl ObjectStore for MemoryObjectStore {
    async fn put(&self, key: &str, body: Vec<u8>) -> PortResult<()> {
        self.objects
            .write()
            .await
            .insert(key.to_owned(), (body, OffsetDateTime::now_utc()));
        Ok(())
    }

    async fn get(&self, key: &str) -> PortResult<Option<Vec<u8>>> {
        Ok(self
            .objects
            .read()
            .await
            .get(key)
            .map(|(body, _)| body.clone()))
    }

    async fn list(&self, prefix: &str) -> PortResult<Vec<ObjectEntry>> {
        Ok(self
            .objects
            .read()
            .await
            .range(prefix.to_owned()..)
            .take_while(|(key, _)| key.starts_with(prefix))
            .map(|(key, (body, at))| ObjectEntry {
                key: key.clone(),
                size: body.len() as u64,
                last_modified: Some(*at),
            })
            .collect())
    }

    async fn delete(&self, key: &str) -> PortResult<()> {
        self.objects.write().await.remove(key);
        Ok(())
    }
}
