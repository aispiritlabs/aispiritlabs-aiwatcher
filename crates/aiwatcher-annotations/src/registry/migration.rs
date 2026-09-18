//! What this registry holds, and where each object goes in a bound scope.
//!
//! This is the one family here whose objects are not all JSON. A blob is a
//! picture and a staged page is somebody else's rows as JSONL, so both are
//! reported as opaque bytes: a reader that tried to parse them and called the
//! failure a damaged registry would refuse every real corpus in the store.
//!
//! The ordering is this crate's own, from [`crate::store`]: the revision
//! before the image head that indexes it, the export manifest before the index
//! entry that lists it. The project head goes **last** of all, which is the
//! same rule read from the other end — a project written first would, after an
//! interrupted copy, list itself as a complete project with most of its images
//! missing, while a project written last leaves images nothing can reach and
//! nothing that reads as finished.

use std::collections::BTreeMap;

use aiwatcher_core::migration::{
    ContentKind, Inventory, InventoryError, InventoryObject, Reference, ReferenceKind, WriteOrder,
    key_is_safe, sha256_hex,
};
use aiwatcher_iam::ProjectScope;

use super::Registry;
use crate::export::ExportManifest;
use crate::images::{AnnotationRevision, BLOB_SCHEME, ImageHead};
use crate::imports::staging::StagedBatch;
use crate::imports::{ImportIndex, ImportJob, ImportManifest};
use crate::project::AnnotationProject;

/// The documents and byte objects this registry writes, and nothing else.
enum Document {
    Project,
    Image,
    Revision,
    ExportManifest,
    ExportIndex,
    Blob,
    BlobMeta,
    Batch,
    BatchPage,
    ImportJob,
    ImportShard,
    ImportManifest,
    ImportIndex,
}

impl Document {
    fn category(&self) -> &'static str {
        match self {
            Self::Project => "projects",
            Self::Image => "images",
            Self::Revision => "revisions",
            Self::ExportManifest => "exports",
            Self::ExportIndex => "export-indexes",
            Self::Blob => "blobs",
            Self::BlobMeta => "blob-metadata",
            Self::Batch => "import-batches",
            Self::BatchPage => "import-batch-pages",
            Self::ImportJob => "import-jobs",
            Self::ImportShard => "import-shards",
            Self::ImportManifest => "import-manifests",
            Self::ImportIndex => "import-indexes",
        }
    }

    fn order(&self) -> WriteOrder {
        match self {
            Self::Blob | Self::BlobMeta | Self::Revision | Self::BatchPage | Self::ImportShard => {
                WriteOrder::Content
            }
            Self::Image
            | Self::ExportManifest
            | Self::Batch
            | Self::ImportJob
            | Self::ImportManifest => WriteOrder::Record,
            Self::Project | Self::ExportIndex | Self::ImportIndex => WriteOrder::Index,
        }
    }

    fn content(&self) -> ContentKind {
        match self {
            // A picture, and somebody else's rows. Neither is this crate's
            // document, and neither is parsed to decide whether it is sound.
            Self::Blob | Self::BatchPage | Self::ImportShard => ContentKind::Opaque,
            _ => ContentKind::Json,
        }
    }
}

impl Registry {
    /// Every annotation object under this registry's prefix, and the key each
    /// one takes in `target`.
    ///
    /// Read-only: it lists, reads and hashes, and writes nothing anywhere.
    /// Blob bytes are read in full, because a content address that is not
    /// re-computed from the bytes is a label rather than a verification.
    ///
    /// # Errors
    ///
    /// [`InventoryError::Damaged`] when a JSON object under this prefix is not
    /// an annotation document, and [`InventoryError::Refused`] when the source
    /// is already bound to a project.
    pub async fn inventory(&self, target: ProjectScope) -> Result<Inventory, InventoryError> {
        if self.backend.is_scoped() {
            return Err(InventoryError::Refused(
                "a migration source must be the unscoped annotation registry".to_owned(),
            ));
        }
        let destination = self
            .for_project(target)
            .map_err(|error| InventoryError::Refused(error.to_string()))?;
        let source_prefix = format!("{}/", self.backend.prefix());
        let target_prefix = format!("{}/", destination.backend.prefix());
        let scopes = format!("{source_prefix}scopes/");
        let mut objects = Vec::new();
        let mut keys = self.backend.keys(&source_prefix).await.map_err(refused)?;
        keys.sort();
        for key in keys {
            let Some(rest) = key.strip_prefix(&source_prefix) else {
                return Err(InventoryError::Refused(format!(
                    "the store returned {key} outside the requested prefix"
                )));
            };
            if key.starts_with(&scopes) {
                continue;
            }
            let Some(document) = classify(rest) else {
                continue;
            };
            objects.push(self.describe(&key, rest, &target_prefix, &document).await?);
        }
        Ok(Inventory {
            source_prefix,
            target_prefix,
            objects,
            skipped: BTreeMap::from([(scopes, "already bound to a project scope".to_owned())]),
        })
    }

    async fn describe(
        &self,
        key: &str,
        rest: &str,
        target_prefix: &str,
        document: &Document,
    ) -> Result<InventoryObject, InventoryError> {
        let body = self
            .backend
            .get_bytes(key)
            .await
            .map_err(refused)?
            .ok_or_else(|| InventoryError::Damaged {
                key: key.to_owned(),
                message: "listed by the store and then gone".to_owned(),
            })?;
        let references =
            self.references(&body, document)
                .map_err(|message| InventoryError::Damaged {
                    key: key.to_owned(),
                    message,
                })?;
        let target_key = format!("{target_prefix}{rest}");
        if !key_is_safe(&target_key) {
            return Err(InventoryError::Refused(format!(
                "{key} would map to an unusable project key"
            )));
        }
        Ok(InventoryObject {
            category: document.category().to_owned(),
            source_key: key.to_owned(),
            target_key,
            order: document.order(),
            content: document.content(),
            bytes: body.len() as u64,
            sha256: sha256_hex(&body),
            references,
        })
    }

    #[allow(clippy::too_many_lines)]
    fn references(
        &self,
        body: &[u8],
        document: &Document,
    ) -> Result<BTreeMap<String, Reference>, String> {
        let parse = |what: &str| -> Result<serde_json::Value, String> {
            serde_json::from_slice(body).map_err(|e| format!("{what}: {e}"))
        };
        let mut out = BTreeMap::new();
        match document {
            // Opaque by declaration. A picture is not this crate's document
            // and a staged page is somebody else's rows.
            Document::Blob | Document::BatchPage | Document::ImportShard => {}
            Document::BlobMeta => {
                parse("a blob's metadata")?;
            }
            Document::Project => {
                let project: AnnotationProject = serde_json::from_slice(body)
                    .map_err(|e| format!("an annotation project: {e}"))?;
                out.insert(
                    "/schema/version".to_owned(),
                    opaque(
                        &project.schema.version,
                        "the label schema's own content address",
                    ),
                );
            }
            Document::Image => {
                let head: ImageHead =
                    serde_json::from_slice(body).map_err(|e| format!("an image head: {e}"))?;
                out.insert(
                    "/image/uri".to_owned(),
                    self.blob_reference(&head.image.uri, &head.image.image_id),
                );
                if let Some(accepted) = &head.accepted {
                    out.insert(
                        "/accepted".to_owned(),
                        self.revision_reference(&head.project, &head.image.image_id, accepted),
                    );
                }
                for (index, summary) in head.revisions.iter().enumerate() {
                    out.insert(
                        format!("/revisions/{index}/revision"),
                        self.revision_reference(
                            &head.project,
                            &head.image.image_id,
                            &summary.revision,
                        ),
                    );
                }
            }
            Document::Revision => {
                let revision: AnnotationRevision = serde_json::from_slice(body)
                    .map_err(|e| format!("an annotation revision: {e}"))?;
                out.insert(
                    "/image_id".to_owned(),
                    self.image_reference(&revision.project, &revision.image_id),
                );
                out.insert(
                    "/schema_version".to_owned(),
                    opaque(
                        &revision.schema_version,
                        "the schema version these shapes were checked against",
                    ),
                );
            }
            Document::ExportManifest => {
                let manifest: ExportManifest =
                    serde_json::from_slice(body).map_err(|e| format!("an export manifest: {e}"))?;
                for (index, sample) in manifest.samples.iter().enumerate() {
                    out.insert(
                        format!("/samples/{index}/image_id"),
                        self.image_reference(&manifest.project, &sample.image_id),
                    );
                    out.insert(
                        format!("/samples/{index}/revision"),
                        self.revision_reference(
                            &manifest.project,
                            &sample.image_id,
                            &sample.revision,
                        ),
                    );
                }
            }
            Document::ExportIndex => {
                parse("an export index")?;
            }
            Document::Batch => {
                let batch: StagedBatch =
                    serde_json::from_slice(body).map_err(|e| format!("a staged batch: {e}"))?;
                for (index, page) in batch.pages.iter().enumerate() {
                    out.insert(
                        format!("/pages/{index}/digest"),
                        self.batch_page_reference(&batch.batch_id, page.index, &page.digest),
                    );
                }
            }
            Document::ImportJob => {
                let job: ImportJob =
                    serde_json::from_slice(body).map_err(|e| format!("an import job: {e}"))?;
                out.insert(
                    "/batch_id".to_owned(),
                    Reference {
                        value: job.batch_id.clone(),
                        kind: ReferenceKind::Internal {
                            source_key: self.backend.batch_key(&job.batch_id),
                        },
                    },
                );
            }
            Document::ImportManifest => {
                let manifest: ImportManifest =
                    serde_json::from_slice(body).map_err(|e| format!("an import manifest: {e}"))?;
                out.insert(
                    "/job_id".to_owned(),
                    Reference {
                        value: manifest.job_id.clone(),
                        kind: ReferenceKind::Internal {
                            source_key: self.backend.import_job_key(&manifest.job_id),
                        },
                    },
                );
                out.insert(
                    "/batch_id".to_owned(),
                    Reference {
                        value: manifest.batch_id.clone(),
                        kind: ReferenceKind::Internal {
                            source_key: self.backend.batch_key(&manifest.batch_id),
                        },
                    },
                );
            }
            Document::ImportIndex => {
                let index: ImportIndex =
                    serde_json::from_slice(body).map_err(|e| format!("an import index: {e}"))?;
                for (position, summary) in index.imports.iter().enumerate() {
                    out.insert(
                        format!("/imports/{position}/version"),
                        Reference {
                            value: summary.version.clone(),
                            kind: ReferenceKind::Internal {
                                source_key: self.backend.import_manifest_key(&summary.version),
                            },
                        },
                    );
                }
            }
        }
        Ok(out)
    }

    /// `aiwatcher://blob/<image_id>` is this registry's own bytes; anything
    /// else is a URI somebody else holds and nothing here can move.
    fn blob_reference(&self, uri: &str, image_id: &str) -> Reference {
        Reference {
            value: uri.to_owned(),
            kind: if uri.strip_prefix(BLOB_SCHEME) == Some(image_id) {
                ReferenceKind::Internal {
                    source_key: self.backend.blob_key(image_id),
                }
            } else {
                ReferenceKind::Opaque {
                    reason: "image bytes stored outside this registry".to_owned(),
                }
            },
        }
    }

    fn image_reference(&self, project: &str, image_id: &str) -> Reference {
        Reference {
            value: image_id.to_owned(),
            kind: ReferenceKind::Internal {
                source_key: self.backend.image_key(project, image_id),
            },
        }
    }

    fn revision_reference(&self, project: &str, image_id: &str, revision: &str) -> Reference {
        Reference {
            value: revision.to_owned(),
            kind: ReferenceKind::Internal {
                source_key: self.backend.revision_key(project, image_id, revision),
            },
        }
    }

    fn batch_page_reference(&self, batch: &str, page: usize, digest: &str) -> Reference {
        Reference {
            value: digest.to_owned(),
            kind: ReferenceKind::Internal {
                source_key: self.backend.batch_page_key(batch, page),
            },
        }
    }
}

/// The store refusing a key or a read, in the migration vocabulary.
///
/// `Store` keeps its retryability; everything else this backend can answer with
/// is about the key, and a migration that retried one of those would retry for
/// ever.
fn refused(error: crate::Error) -> InventoryError {
    match error {
        crate::Error::Store(port) => InventoryError::Store(port),
        crate::Error::Corrupt { key, message } => InventoryError::Damaged { key, message },
        other => InventoryError::Refused(other.to_string()),
    }
}

fn opaque(value: &str, reason: &str) -> Reference {
    Reference {
        value: value.to_owned(),
        kind: ReferenceKind::Opaque {
            reason: reason.to_owned(),
        },
    }
}

/// Which object a relative key is, if it is one of this registry's at all.
///
/// `exports/index.json` is matched before the manifest arm it would otherwise
/// fall into: the index is the one export key that is not an export.
fn classify(rest: &str) -> Option<Document> {
    let json = |file: &str| file.ends_with(".json");
    let jsonl = |file: &str| file.ends_with(".jsonl");
    match rest.split('/').collect::<Vec<_>>().as_slice() {
        ["projects", _, "head.json"] => Some(Document::Project),
        ["projects", _, "images", file] if json(file) => Some(Document::Image),
        ["projects", _, "revisions", _, file] if json(file) => Some(Document::Revision),
        ["projects", _, "exports", "index.json"] => Some(Document::ExportIndex),
        ["projects", _, "exports", file] if json(file) => Some(Document::ExportManifest),
        ["blobs", file] if json(file) => Some(Document::BlobMeta),
        ["blobs", _] => Some(Document::Blob),
        ["imports", "batches", _, "manifest.json"] => Some(Document::Batch),
        ["imports", "batches", _, "pages", file] if jsonl(file) => Some(Document::BatchPage),
        ["imports", "jobs", _, "job.json"] => Some(Document::ImportJob),
        ["imports", "jobs", _, "results" | "rejects", file] if jsonl(file) => {
            Some(Document::ImportShard)
        }
        ["imports", "manifests", "index.json"] => Some(Document::ImportIndex),
        ["imports", "manifests", file] if json(file) => Some(Document::ImportManifest),
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use aiwatcher_core::prompts::ObjectStore as _;
    use aiwatcher_iam::{OrganizationId, ProjectId};
    use aiwatcher_prompts::adapters::memory::MemoryObjectStore;
    use serde_json::json;
    use std::sync::Arc;

    fn scope() -> ProjectScope {
        ProjectScope {
            organization: OrganizationId::new(),
            project: ProjectId::new(),
        }
    }

    async fn seeded() -> (Arc<MemoryObjectStore>, Registry) {
        let store = Arc::new(MemoryObjectStore::new());
        let registry = Registry::new(store.clone(), "annotations");
        registry
            .save_project(
                serde_json::from_value(json!({
                    "name": "plans/first",
                    "classes": [{"name": "wall", "geometry": "polyline"}]
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        let blob = registry
            .put_blob(b"\x89PNG\r\n\x1a\nnot a picture".to_vec(), "image/png")
            .await
            .unwrap();
        registry
            .register_image(
                serde_json::from_value(json!({
                    "project": "plans/first", "image_id": blob.image_id, "uri": blob.uri,
                    "width": 10, "height": 10, "group_id": "house-1",
                    "rights": {"kind": "unknown"}
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        (store, registry)
    }

    #[tokio::test]
    async fn a_picture_is_opaque_bytes_and_an_image_head_names_the_blob_it_points_at() {
        let (store, registry) = seeded().await;
        let before = store.list("").await.unwrap();
        let inventory = registry.inventory(scope()).await.unwrap();
        assert_eq!(store.list("").await.unwrap(), before, "reads only");

        let blob = inventory
            .objects
            .iter()
            .find(|object| object.category == "blobs")
            .expect("a blob");
        assert_eq!(
            blob.content,
            ContentKind::Opaque,
            "a picture is not this crate's document and is never parsed"
        );
        assert_eq!(blob.order, WriteOrder::Content);
        assert!(blob.references.is_empty());

        let head = inventory
            .objects
            .iter()
            .find(|object| object.category == "images")
            .expect("an image head");
        assert_eq!(head.order, WriteOrder::Record);
        let ReferenceKind::Internal { source_key } = &head.references["/image/uri"].kind else {
            panic!("an uploaded image's bytes are in this registry");
        };
        assert_eq!(source_key, &blob.source_key);

        let project = inventory
            .objects
            .iter()
            .find(|object| object.category == "projects")
            .expect("a project head");
        assert_eq!(
            project.order,
            WriteOrder::Index,
            "a project written last leaves unreachable images rather than a project that \
             reads as complete"
        );
        // Sorting by the declared order is what the copy follows.
        let mut ordered: Vec<_> = inventory
            .objects
            .iter()
            .map(|object| (object.order, object.category.as_str()))
            .collect();
        ordered.sort();
        assert_eq!(
            ordered.first().map(|entry| entry.0),
            Some(WriteOrder::Content)
        );
        assert_eq!(ordered.last().map(|entry| entry.0), Some(WriteOrder::Index));
    }

    #[tokio::test]
    async fn an_export_index_is_an_index_and_not_one_more_export() {
        let store = Arc::new(MemoryObjectStore::new());
        let registry = Registry::new(store.clone(), "annotations");
        let hashed = crate::digest("plans/first".as_bytes());
        store
            .put(
                &format!("annotations/projects/{hashed}/exports/index.json"),
                b"{\"exports\":[]}".to_vec(),
            )
            .await
            .unwrap();
        let inventory = registry.inventory(scope()).await.unwrap();
        let entry = inventory.objects.first().expect("the index is inventoried");
        assert_eq!(entry.category, "export-indexes");
        assert_eq!(entry.order, WriteOrder::Index);
    }

    #[tokio::test]
    async fn a_stray_file_is_neither_migrated_nor_declared_skipped() {
        let (store, registry) = seeded().await;
        store
            .put("annotations/notes.txt", b"a stray file".to_vec())
            .await
            .unwrap();
        let inventory = registry.inventory(scope()).await.unwrap();
        assert!(
            !inventory
                .objects
                .iter()
                .any(|object| object.source_key == "annotations/notes.txt")
        );
        assert!(
            !inventory
                .skipped
                .keys()
                .any(|prefix| "annotations/notes.txt".starts_with(prefix.as_str()))
        );
    }
}
