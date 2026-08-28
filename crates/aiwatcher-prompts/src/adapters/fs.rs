//! A directory, behaving like a bucket.
//!
//! What `just run` uses. The registry is the one part of aiwatcher that has to
//! keep something forever, and requiring an object store to be up before a
//! prompt can be saved would make `cargo run --bin aiwatcher` — which starts a
//! working instance with no setup at all — stop being true.
//!
//! It is a development adapter and says so: no atomic rename across a crash,
//! no concurrent-writer story beyond what the filesystem gives, one machine.
//! `AIWATCHER_PROMPT_STORE=s3` is what a deployment runs.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use time::OffsetDateTime;

use aiwatcher_core::ports::{PortError, PortResult};
use aiwatcher_core::prompts::{ObjectEntry, ObjectStore};

const TARGET: &str = "prompt-object-store";

/// A local directory addressed by object key.
#[derive(Debug, Clone)]
pub struct FileObjectStore {
    root: PathBuf,
}

impl FileObjectStore {
    /// Open (and create) the directory.
    ///
    /// # Errors
    ///
    /// [`PortError::Unavailable`] when the directory cannot be created.
    pub async fn open(root: impl Into<PathBuf>) -> PortResult<Self> {
        let root = root.into();
        tokio::fs::create_dir_all(&root)
            .await
            .map_err(|error| unavailable("creating the prompt directory", &error))?;
        Ok(Self { root })
    }

    /// Resolve a key to a path inside the root.
    ///
    /// Every key this store sees is built by `Registry::key_for` out of
    /// validated components, so the check below should never fire. It is here
    /// anyway: a store whose keys become filesystem paths is one refactor away
    /// from turning a caller's mistake into a write outside the data
    /// directory, and the cost of the check is one comparison.
    fn path_for(&self, key: &str) -> PortResult<PathBuf> {
        let rejected = || PortError::Rejected {
            target: TARGET,
            message: format!("{key:?} is not a usable object key"),
        };
        if key.is_empty() || key.starts_with('/') {
            return Err(rejected());
        }
        let path = self.root.join(key);
        if path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(rejected());
        }
        Ok(path)
    }

    /// Every file below `dir`, as object keys relative to the root.
    fn walk(&self, dir: &Path, found: &mut Vec<ObjectEntry>) -> std::io::Result<()> {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            // A prefix nobody has written to yet lists as empty, the way it
            // would in a bucket.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                self.walk(&path, found)?;
                continue;
            }
            let Ok(relative) = path.strip_prefix(&self.root) else {
                continue;
            };
            let Some(key) = relative.to_str() else {
                continue;
            };
            // A crash between the write and the rename leaves one of these.
            // It is not an object, and listing it as one would hand the
            // registry a key whose contents are half a document.
            if key.ends_with(".tmp") {
                continue;
            }
            found.push(ObjectEntry {
                key: key.replace(std::path::MAIN_SEPARATOR, "/"),
                size: metadata.len(),
                last_modified: metadata.modified().ok().map(OffsetDateTime::from),
            });
        }
        Ok(())
    }
}

fn unavailable(doing: &str, error: &std::io::Error) -> PortError {
    PortError::Unavailable {
        target: TARGET,
        message: format!("{doing}: {error}"),
    }
}

#[async_trait]
impl ObjectStore for FileObjectStore {
    async fn put(&self, key: &str, body: Vec<u8>) -> PortResult<()> {
        let path = self.path_for(key)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| unavailable("creating an object directory", &error))?;
        }
        // Written beside and renamed over: a crash mid-write leaves the
        // previous version readable rather than a truncated JSON document that
        // every later read fails on.
        let temporary = path.with_extension("tmp");
        tokio::fs::write(&temporary, &body)
            .await
            .map_err(|error| unavailable("writing an object", &error))?;
        tokio::fs::rename(&temporary, &path)
            .await
            .map_err(|error| unavailable("replacing an object", &error))
    }

    async fn get(&self, key: &str) -> PortResult<Option<Vec<u8>>> {
        let path = self.path_for(key)?;
        match tokio::fs::read(&path).await {
            Ok(body) => Ok(Some(body)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(unavailable("reading an object", &error)),
        }
    }

    async fn list(&self, prefix: &str) -> PortResult<Vec<ObjectEntry>> {
        // The walk is synchronous and the tree is small — a few files per
        // prompt — but it is still filesystem work, so it goes to the blocking
        // pool rather than stalling a reactor thread.
        let store = self.clone();
        let root = self.root.clone();
        let prefix = prefix.to_owned();
        tokio::task::spawn_blocking(move || {
            let mut found = Vec::new();
            store.walk(&root, &mut found)?;
            found.retain(|entry| entry.key.starts_with(&prefix));
            Ok(found)
        })
        .await
        .map_err(|error| PortError::Other {
            target: TARGET,
            source: Box::new(error),
        })?
        .map_err(|error: std::io::Error| unavailable("listing objects", &error))
    }

    async fn delete(&self, key: &str) -> PortResult<()> {
        let path = self.path_for(key)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(unavailable("deleting an object", &error)),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// A directory of its own per test.
    ///
    /// Named rather than timestamped: these run concurrently in one process,
    /// and two of them sharing a root means one deletes the other's tree
    /// halfway through — which shows up as a rare, unreproducible failure.
    async fn temporary_store(test: &str) -> (FileObjectStore, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("aiwatcher-prompts-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = FileObjectStore::open(&root).await.expect("opens");
        (store, root)
    }

    #[tokio::test]
    async fn a_missing_key_reads_as_absent_rather_than_as_an_error() {
        let (store, root) = temporary_store("missing").await;
        assert_eq!(store.get("prompts/nothing/head.json").await.unwrap(), None);
        // And deleting one succeeds, the way S3 answers 204.
        store.delete("prompts/nothing/head.json").await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn objects_round_trip_and_list_under_their_prefix() {
        let (store, root) = temporary_store("round-trip").await;
        store
            .put("prompts/a/head.json", b"{}".to_vec())
            .await
            .unwrap();
        store
            .put("prompts/a/versions/ff.json", b"[]".to_vec())
            .await
            .unwrap();
        store
            .put("prompts/b/head.json", b"{}".to_vec())
            .await
            .unwrap();

        assert_eq!(
            store.get("prompts/a/versions/ff.json").await.unwrap(),
            Some(b"[]".to_vec())
        );

        let mut keys: Vec<String> = store
            .list("prompts/a/")
            .await
            .unwrap()
            .into_iter()
            .map(|entry| entry.key)
            .collect();
        keys.sort();
        assert_eq!(keys, ["prompts/a/head.json", "prompts/a/versions/ff.json"]);

        assert_eq!(store.list("prompts/").await.unwrap().len(), 3);
        assert!(
            store
                .list("prompts/never-written/")
                .await
                .unwrap()
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_key_that_climbs_out_of_the_root_is_refused() {
        let (store, root) = temporary_store("hostile-keys").await;
        for hostile in ["../escape.json", "/etc/passwd", "a/../../escape.json", ""] {
            let error = store.put(hostile, b"x".to_vec()).await.expect_err(hostile);
            assert!(!error.is_retryable(), "{hostile}: {error}");
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_put_over_an_existing_key_replaces_it() {
        let (store, root) = temporary_store("replace").await;
        store
            .put("prompts/a/head.json", b"one".to_vec())
            .await
            .unwrap();
        store
            .put("prompts/a/head.json", b"two".to_vec())
            .await
            .unwrap();
        assert_eq!(
            store.get("prompts/a/head.json").await.unwrap(),
            Some(b"two".to_vec())
        );
        // The temporary file the atomic write goes through must not be left
        // behind as an object of its own.
        let keys: Vec<String> = store
            .list("prompts/")
            .await
            .unwrap()
            .into_iter()
            .map(|entry| entry.key)
            .collect();
        assert_eq!(keys, ["prompts/a/head.json"]);
        let _ = std::fs::remove_dir_all(root);
    }
}
