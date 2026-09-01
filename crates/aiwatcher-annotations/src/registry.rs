//! The object-store side: key layout, ordering, and the reads a panel makes.
//!
//! Two orderings in here are contracts rather than choices, and both are the
//! same one the prompt registry keeps: **the revision object is written before
//! the head that indexes it**, and **the export manifest before the index entry
//! that lists it**. An index naming an object that was never stored is a list
//! whose rows 404; an unindexed object is waiting to be found again.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use aiwatcher_core::prompts::ObjectStore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::export::{
    BuiltExport, ExportManifest, ExportPage, ExportRequest, ExportSummary, resolve_split, to_coco,
};
use crate::project::{
    AnnotationProject, AnnotationRevision, ImageDetail, ImageHead, ImagePage, ProjectPage,
    ProjectSummary, RegisterImageRequest, ReviewRequest, ReviewState, RevisionSummary,
    SaveProjectRequest, SaveRevisionRequest, Split,
};
use crate::schema::LabelSchema;
use crate::{
    Error, MAX_ANNOTATIONS, MAX_BLOB_BYTES, MAX_IMAGE_PAGE, MAX_REVISION_BYTES, Result, digest,
    validate_digest, validate_name,
};

/// How a registered image points at bytes this registry holds.
///
/// A scheme of its own rather than a path, because a URI in `ImageRecord` may
/// equally be an external URL and a reader has to be able to tell. Not `blob:`:
/// the browser owns that one, and an `<img src="blob:…">` would resolve
/// against the document instead of reaching this server.
pub const BLOB_SCHEME: &str = "aiwatcher://blob/";

/// A stored image blob and what it is.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct StoredBlob {
    /// SHA-256 of the bytes, computed here. This is the `image_id`.
    pub image_id: String,
    pub content_type: String,
    pub bytes: usize,
    /// What to put in `ImageRecord::uri`. See [`BLOB_SCHEME`].
    pub uri: String,
    /// False when these exact bytes were already stored.
    pub created: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct BlobMeta {
    content_type: String,
    bytes: usize,
}

/// The result of saving a drawing.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct SavedRevision {
    pub revision: AnnotationRevision,
    pub head: ImageHead,
    /// False when this exact set of shapes was already stored.
    pub created: bool,
}

/// How an image list is narrowed.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct ImageFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<ReviewState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split: Option<Split>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    /// Matches the source, the group, the level and the image id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
}

/// One namespace in the configured authored object store.
#[derive(Clone, Debug)]
pub struct Registry {
    store: Arc<dyn ObjectStore>,
    prefix: String,
}

impl Registry {
    #[must_use]
    pub fn new(store: Arc<dyn ObjectStore>, prefix: impl Into<String>) -> Self {
        Self {
            store,
            prefix: prefix.into().trim_matches('/').to_owned(),
        }
    }

    // ── Projects ─────────────────────────────────────────────────────────────

    /// Create a project, or replace its description, splits and schema.
    ///
    /// The schema is re-derived from the classes, so an unchanged class list
    /// keeps its version and every accepted revision stays valid. A changed one
    /// is a new version, and every revision drawn under the old one is excluded
    /// from the next export by name — which is the loud failure, and the right
    /// one. Renaming a class silently would relabel history.
    pub async fn save_project(&self, request: SaveProjectRequest) -> Result<AnnotationProject> {
        request.validate()?;
        let now = OffsetDateTime::now_utc();
        let schema = LabelSchema::build(request.classes, now)?;
        let key = self.project_key(&request.name);
        let existing: Option<AnnotationProject> = self.read_json(&key).await?;
        let project = AnnotationProject {
            name: request.name,
            description: request.description,
            schema,
            splits: request.splits,
            split_salt: request.split_salt,
            split_overrides: request.split_overrides,
            created_at: existing.as_ref().map_or(now, |project| project.created_at),
            updated_at: now,
        };
        self.write_json(&key, &project).await?;
        Ok(project)
    }

    pub async fn projects(&self) -> Result<ProjectPage> {
        let mut projects = Vec::new();
        for entry in self
            .store
            .list(&format!("{}/projects/", self.prefix))
            .await?
        {
            if !entry.key.ends_with("/head.json") {
                continue;
            }
            if let Some(project) = self.read_json::<AnnotationProject>(&entry.key).await? {
                projects.push(project);
            }
        }
        projects.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(ProjectPage { projects })
    }

    pub async fn project(&self, name: &str) -> Result<AnnotationProject> {
        validate_name(name, "a project")?;
        self.read_json(&self.project_key(name))
            .await?
            .ok_or_else(|| Error::NotFound(format!("the project {name}")))
    }

    /// A project with the counts that answer "is there enough data yet".
    pub async fn project_summary(&self, name: &str) -> Result<ProjectSummary> {
        let project = self.project(name).await?;
        let heads = self.image_heads(name).await?;
        let mut groups: BTreeSet<String> = BTreeSet::new();
        let mut per_class: BTreeMap<String, usize> = BTreeMap::new();
        let mut accepted = 0;
        let mut instances = 0;
        for head in &heads {
            groups.insert(head.image.group_id.clone());
            if let Some(summary) = head.accepted_summary() {
                accepted += 1;
                instances += summary.shape_count;
                for (class, count) in &summary.per_class {
                    *per_class.entry(class.clone()).or_default() += count;
                }
            }
        }
        Ok(ProjectSummary {
            project,
            images: heads.len(),
            accepted,
            groups: groups.len(),
            instances,
            per_class,
        })
    }

    // ── Image bytes ──────────────────────────────────────────────────────────

    /// Store an upload under the digest of its own bytes.
    ///
    /// The digest is computed here and the client's claim about it is never
    /// read. Content addressing whose address comes from the caller is not
    /// content addressing.
    pub async fn put_blob(&self, body: Vec<u8>, content_type: &str) -> Result<StoredBlob> {
        if body.is_empty() {
            return Err(Error::Invalid("the upload is empty".to_owned()));
        }
        if body.len() > MAX_BLOB_BYTES {
            return Err(Error::TooLarge {
                what: "the image",
                size: body.len(),
                limit: MAX_BLOB_BYTES,
            });
        }
        let content_type = if content_type.starts_with("image/") {
            content_type.to_owned()
        } else {
            sniff(&body).to_owned()
        };
        let image_id = digest(&body);
        let meta_key = self.blob_meta_key(&image_id);
        let created = self.read_json::<BlobMeta>(&meta_key).await?.is_none();
        let bytes = body.len();
        if created {
            // Bytes first, sidecar second: a described object that is not there
            // is worse than an undescribed one that is.
            self.store.put(&self.blob_key(&image_id), body).await?;
            self.write_json(
                &meta_key,
                &BlobMeta {
                    content_type: content_type.clone(),
                    bytes,
                },
            )
            .await?;
        }
        Ok(StoredBlob {
            // Not `blob:` — that is a scheme the browser already owns, and an
            // `<img src="blob:…">` would resolve against the document instead
            // of reaching this server.
            uri: format!("{BLOB_SCHEME}{image_id}"),
            image_id,
            content_type,
            bytes,
            created,
        })
    }

    /// The bytes and content type of an uploaded image.
    pub async fn blob(&self, image_id: &str) -> Result<(Vec<u8>, String)> {
        validate_digest(image_id, "an image id")?;
        let body = self
            .store
            .get(&self.blob_key(image_id))
            .await?
            .ok_or_else(|| Error::NotFound(format!("the image {image_id}")))?;
        let content_type = self
            .read_json::<BlobMeta>(&self.blob_meta_key(image_id))
            .await?
            .map_or_else(|| sniff(&body).to_owned(), |meta| meta.content_type);
        Ok((body, content_type))
    }

    // ── Images ───────────────────────────────────────────────────────────────

    /// Register an image into a project, or update what is known about it.
    ///
    /// Re-registering keeps the revisions and the review state: the same plan
    /// re-uploaded with a corrected `group_id` is a correction, not a reset.
    pub async fn register_image(&self, request: RegisterImageRequest) -> Result<ImageHead> {
        let project = self.project(&request.project).await?;
        validate_digest(&request.image_id, "an image id")?;
        validate_name(&request.group_id, "a group")?;
        if request.uri.trim().is_empty() {
            return Err(Error::Invalid("an image needs a uri".to_owned()));
        }
        if request.width == 0 || request.height == 0 {
            return Err(Error::Invalid(
                "an image needs a non-zero width and height".to_owned(),
            ));
        }

        let key = self.image_key(&project.name, &request.image_id);
        let existing: Option<ImageHead> = self.read_json(&key).await?;
        let now = OffsetDateTime::now_utc();
        let head = match existing {
            Some(mut head) => {
                head.image.uri = request.uri;
                head.image.width = request.width;
                head.image.height = request.height;
                head.image.group_id = request.group_id;
                head.image.source = request.source;
                head.image.rights = request.rights;
                head.image.view = request.view;
                head.image.level = request.level;
                if !request.metadata.is_empty() {
                    head.image.metadata = request.metadata;
                }
                head
            }
            None => ImageHead {
                project: project.name.clone(),
                image: crate::project::ImageRecord {
                    image_id: request.image_id,
                    uri: request.uri,
                    width: request.width,
                    height: request.height,
                    group_id: request.group_id,
                    source: request.source,
                    rights: request.rights,
                    view: request.view,
                    level: request.level,
                    metadata: request.metadata,
                    registered_at: now,
                },
                review: ReviewState::Draft,
                accepted: None,
                reviewed_by: None,
                reviewed_at: None,
                revisions: Vec::new(),
                note: String::new(),
            },
        };
        self.write_json(&key, &head).await?;
        Ok(head)
    }

    pub async fn images(
        &self,
        project: &str,
        filter: &ImageFilter,
        offset: usize,
        limit: usize,
    ) -> Result<ImagePage> {
        let definition = self.project(project).await?;
        let limit = limit.clamp(1, MAX_IMAGE_PAGE);
        let needle = filter.search.as_ref().map(|value| value.to_lowercase());
        let mut heads: Vec<ImageHead> = self
            .image_heads(project)
            .await?
            .into_iter()
            .filter(|head| filter.review.is_none_or(|wanted| head.review == wanted))
            .filter(|head| {
                filter
                    .group_id
                    .as_ref()
                    .is_none_or(|wanted| &head.image.group_id == wanted)
            })
            .filter(|head| {
                filter.split.is_none_or(|wanted| {
                    resolve_split(
                        &definition,
                        &head.image.group_id,
                        &definition.split_salt,
                        definition.splits,
                    ) == wanted
                })
            })
            .filter(|head| {
                needle.as_ref().is_none_or(|needle| {
                    head.image.image_id.contains(needle.as_str())
                        || head.image.group_id.to_lowercase().contains(needle)
                        || head.image.source.to_lowercase().contains(needle)
                        || head
                            .image
                            .level
                            .as_ref()
                            .is_some_and(|level| level.to_lowercase().contains(needle))
                })
            })
            .collect();

        // Newest registration first, then by id so the order is total.
        heads.sort_by(|left, right| {
            right
                .image
                .registered_at
                .cmp(&left.image.registered_at)
                .then_with(|| left.image.image_id.cmp(&right.image.image_id))
        });
        let total = heads.len();
        let page: Vec<ImageHead> = heads.into_iter().skip(offset).take(limit).collect();
        let next_offset = (offset + page.len() < total).then_some(offset + page.len());
        Ok(ImagePage {
            images: page,
            total,
            offset,
            limit,
            next_offset,
        })
    }

    /// One image, with a revision's shapes: the one asked for, else the
    /// accepted one, else the newest.
    pub async fn image(
        &self,
        project: &str,
        image_id: &str,
        revision: Option<&str>,
    ) -> Result<ImageDetail> {
        let definition = self.project(project).await?;
        let head = self.image_head(project, image_id).await?;
        let wanted = match revision {
            Some(revision) => Some(revision.to_owned()),
            None => head
                .accepted
                .clone()
                .or_else(|| head.latest().map(|summary| summary.revision.clone())),
        };
        let revision = match wanted {
            Some(revision) => {
                validate_digest(&revision, "a revision")?;
                self.read_json::<AnnotationRevision>(
                    &self.revision_key(project, image_id, &revision),
                )
                .await?
            }
            None => None,
        };
        let split = resolve_split(
            &definition,
            &head.image.group_id,
            &definition.split_salt,
            definition.splits,
        );
        Ok(ImageDetail {
            head,
            revision,
            split,
        })
    }

    // ── Revisions ────────────────────────────────────────────────────────────

    /// Validate a drawing against the project's schema and store it.
    ///
    /// Validation is not advisory here: a revision that does not validate is
    /// not stored at all. The alternative — storing it and marking it invalid —
    /// puts the decision about whether a half-drawn door is a training target
    /// into whatever reads it next.
    pub async fn save_revision(
        &self,
        request: SaveRevisionRequest,
        author: &str,
    ) -> Result<SavedRevision> {
        let project = self.project(&request.project).await?;
        let mut head = self.image_head(&project.name, &request.image_id).await?;
        if request.annotations.len() > MAX_ANNOTATIONS {
            return Err(Error::TooLarge {
                what: "the revision",
                size: request.annotations.len(),
                limit: MAX_ANNOTATIONS,
            });
        }
        crate::shapes::validate(
            &request.annotations,
            &project.schema,
            head.image.width,
            head.image.height,
        )?;

        // Author, notes and time are outside the identity: the same drawing by
        // two people is one revision, exactly as the same prompt text is one
        // version.
        let identity = serde_json::to_vec(&(
            &project.name,
            &request.image_id,
            &project.schema.version,
            &request.annotations,
        ))
        .map_err(|error| Error::Invalid(format!("the revision could not be encoded: {error}")))?;
        if identity.len() > MAX_REVISION_BYTES {
            return Err(Error::TooLarge {
                what: "the encoded revision",
                size: identity.len(),
                limit: MAX_REVISION_BYTES,
            });
        }
        let id = digest(&identity);
        let key = self.revision_key(&project.name, &request.image_id, &id);
        let existing: Option<AnnotationRevision> = self.read_json(&key).await?;
        let created = existing.is_none();
        let revision = match existing {
            Some(revision) => revision,
            None => AnnotationRevision {
                project: project.name.clone(),
                image_id: request.image_id.clone(),
                schema_version: project.schema.version.clone(),
                revision: id,
                annotations: request.annotations,
                author: author.to_owned(),
                notes: request.notes,
                created_at: OffsetDateTime::now_utc(),
            },
        };
        if created {
            // Object before index. See the module note.
            self.write_json(&key, &revision).await?;
        }

        let summary = RevisionSummary::of(&revision);
        head.revisions
            .retain(|held| held.revision != summary.revision);
        head.revisions.insert(0, summary);
        if request.accept {
            head.accepted = Some(revision.revision.clone());
            head.review = ReviewState::Accepted;
            head.reviewed_by = Some(author.to_owned());
            head.reviewed_at = Some(OffsetDateTime::now_utc());
        } else if head.review == ReviewState::Draft {
            head.review = ReviewState::Draft;
        }
        self.write_json(&self.image_key(&project.name, &request.image_id), &head)
            .await?;

        Ok(SavedRevision {
            revision,
            head,
            created,
        })
    }

    /// Move an image's review state, and pin the revision an export will read.
    pub async fn review(&self, request: ReviewRequest, reviewer: &str) -> Result<ImageHead> {
        let project = self.project(&request.project).await?;
        let mut head = self.image_head(&project.name, &request.image_id).await?;
        match request.review {
            ReviewState::Accepted => {
                let revision = request.revision.ok_or_else(|| {
                    Error::Invalid(
                        "accepting an image has to name the revision being accepted".to_owned(),
                    )
                })?;
                validate_digest(&revision, "a revision")?;
                if !head
                    .revisions
                    .iter()
                    .any(|summary| summary.revision == revision)
                {
                    return Err(Error::NotFound(format!(
                        "the revision {revision} on this image"
                    )));
                }
                head.accepted = Some(revision);
            }
            _ => {
                if request.revision.is_some() {
                    return Err(Error::Invalid(format!(
                        "only an accepted image names a revision; this one is {}",
                        request.review.as_str()
                    )));
                }
                head.accepted = None;
            }
        }
        head.review = request.review;
        head.reviewed_by = Some(reviewer.to_owned());
        head.reviewed_at = Some(OffsetDateTime::now_utc());
        head.note = request.note;
        self.write_json(&self.image_key(&project.name, &request.image_id), &head)
            .await?;
        Ok(head)
    }

    // ── Exports ──────────────────────────────────────────────────────────────

    /// Build, or find, the immutable manifest for the project as it stands.
    pub async fn export(&self, request: ExportRequest) -> Result<BuiltExport> {
        let project = self.project(&request.project).await?;
        let heads = self.image_heads(&project.name).await?;
        let mut pairs: Vec<(ImageHead, Option<AnnotationRevision>)> =
            Vec::with_capacity(heads.len());
        for head in heads {
            let revision = match head.accepted.as_ref() {
                Some(accepted) => {
                    self.read_json::<AnnotationRevision>(&self.revision_key(
                        &project.name,
                        &head.image.image_id,
                        accepted,
                    ))
                    .await?
                }
                None => None,
            };
            pairs.push((head, revision));
        }

        let manifest = crate::export::build(&project, &request, &pairs, OffsetDateTime::now_utc())?;
        let key = self.export_key(&project.name, &manifest.export);
        let existing: Option<ExportManifest> = self.read_json(&key).await?;
        let created = existing.is_none();
        let manifest = existing.unwrap_or(manifest);
        if created {
            // Manifest before index, same ordering as everything else here.
            self.write_json(&key, &manifest).await?;
            let index_key = self.export_index_key(&project.name);
            let mut index: ExportPage = self.read_json(&index_key).await?.unwrap_or_default();
            index
                .exports
                .retain(|summary| summary.export != manifest.export);
            index.exports.insert(0, ExportSummary::of(&manifest));
            self.write_json(&index_key, &index).await?;
        }
        Ok(BuiltExport { manifest, created })
    }

    pub async fn exports(&self, project: &str) -> Result<ExportPage> {
        validate_name(project, "a project")?;
        Ok(self
            .read_json(&self.export_index_key(project))
            .await?
            .unwrap_or_default())
    }

    pub async fn export_manifest(&self, project: &str, export: &str) -> Result<ExportManifest> {
        validate_name(project, "a project")?;
        validate_digest(export, "an export")?;
        self.read_json(&self.export_key(project, export))
            .await?
            .ok_or_else(|| Error::NotFound(format!("the export {export}")))
    }

    /// The export as a COCO document, optionally narrowed to one split.
    ///
    /// This reads one revision object per sample. That is the cost of not
    /// storing a second copy of every annotation per export, and it is paid
    /// once per training run rather than once per epoch.
    pub async fn coco(&self, project: &str, export: &str, split: Option<Split>) -> Result<Value> {
        let definition = self.project(project).await?;
        let manifest = self.export_manifest(project, export).await?;
        let mut revisions: BTreeMap<String, AnnotationRevision> = BTreeMap::new();
        for sample in &manifest.samples {
            if split.is_some_and(|wanted| wanted != sample.split) {
                continue;
            }
            if let Some(revision) = self
                .read_json::<AnnotationRevision>(&self.revision_key(
                    project,
                    &sample.image_id,
                    &sample.revision,
                ))
                .await?
            {
                revisions.insert(sample.image_id.clone(), revision);
            }
        }
        Ok(to_coco(&manifest, &definition.schema, &revisions, split))
    }

    // ── Store plumbing ───────────────────────────────────────────────────────

    async fn image_head(&self, project: &str, image_id: &str) -> Result<ImageHead> {
        validate_digest(image_id, "an image id")?;
        self.read_json(&self.image_key(project, image_id))
            .await?
            .ok_or_else(|| Error::NotFound(format!("the image {image_id} in {project}")))
    }

    async fn image_heads(&self, project: &str) -> Result<Vec<ImageHead>> {
        let mut heads = Vec::new();
        for entry in self
            .store
            .list(&format!(
                "{}/projects/{}/images/",
                self.prefix,
                Self::id(project)
            ))
            .await?
        {
            if !entry.key.ends_with(".json") {
                continue;
            }
            if let Some(head) = self.read_json::<ImageHead>(&entry.key).await? {
                heads.push(head);
            }
        }
        Ok(heads)
    }

    fn id(name: &str) -> String {
        digest(name.as_bytes())
    }

    fn project_key(&self, name: &str) -> String {
        format!("{}/projects/{}/head.json", self.prefix, Self::id(name))
    }

    fn image_key(&self, project: &str, image_id: &str) -> String {
        format!(
            "{}/projects/{}/images/{image_id}.json",
            self.prefix,
            Self::id(project)
        )
    }

    fn revision_key(&self, project: &str, image_id: &str, revision: &str) -> String {
        format!(
            "{}/projects/{}/revisions/{image_id}/{revision}.json",
            self.prefix,
            Self::id(project)
        )
    }

    fn export_key(&self, project: &str, export: &str) -> String {
        format!(
            "{}/projects/{}/exports/{export}.json",
            self.prefix,
            Self::id(project)
        )
    }

    fn export_index_key(&self, project: &str) -> String {
        format!(
            "{}/projects/{}/exports/index.json",
            self.prefix,
            Self::id(project)
        )
    }

    /// Blobs are keyed by content and shared across projects: the same plan
    /// registered into two projects is one copy of the bytes.
    fn blob_key(&self, image_id: &str) -> String {
        format!("{}/blobs/{image_id}", self.prefix)
    }

    fn blob_meta_key(&self, image_id: &str) -> String {
        format!("{}/blobs/{image_id}.meta.json", self.prefix)
    }

    async fn read_json<T: for<'de> Deserialize<'de>>(&self, key: &str) -> Result<Option<T>> {
        let Some(body) = self.store.get(key).await? else {
            return Ok(None);
        };
        serde_json::from_slice(&body)
            .map(Some)
            .map_err(|error| Error::Corrupt {
                key: key.to_owned(),
                message: error.to_string(),
            })
    }

    async fn write_json<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let body = serde_json::to_vec(value).map_err(|error| Error::Corrupt {
            key: key.to_owned(),
            message: error.to_string(),
        })?;
        self.store.put(key, body).await?;
        Ok(())
    }
}

/// Enough of a content type to serve the bytes back correctly.
///
/// Deliberately tiny: the browser sent a type, this is the fallback for an
/// import that did not, and guessing wrong costs a broken `<img>` rather than
/// anything else.
fn sniff(body: &[u8]) -> &'static str {
    match body {
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [0xFF, 0xD8, 0xFF, ..] => "image/jpeg",
        [b'G', b'I', b'F', b'8', ..] => "image/gif",
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => "image/webp",
        body if body.starts_with(b"<?xml") || body.starts_with(b"<svg") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}
