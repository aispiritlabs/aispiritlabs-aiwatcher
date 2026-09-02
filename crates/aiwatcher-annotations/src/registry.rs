//! The facade: projects, exports, and one door onto every other slice.
//!
//! What is left here after the slices moved out is the shape of the crate.
//! [`Registry`] owns the private store backend and one job that no slice may
//! do for itself — **resolving a project** — and then hands the work to
//! [`crate::images`] or does the export itself.
//!
//! That one job being here is the reason the delegations are not noise. Every
//! image operation needs the project's schema, its split policy and the
//! knowledge that it exists at all; answering that in the facade means one
//! `NotFound`, in one place, in the same words, and an import of six hundred
//! rows resolving it once rather than six hundred times.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use aiwatcher_core::prompts::ObjectStore;
use serde_json::Value;
use time::OffsetDateTime;

use crate::export::{
    BuiltExport, ExportManifest, ExportPage, ExportRequest, ExportSummary, to_coco,
};
use crate::images::import::{ImportReport, ImportRequest};
use crate::images::{
    AnnotationRevision, ImageDetail, ImageFilter, ImageHead, ImagePage, RegisterImageRequest,
    ReviewRequest, SaveRevisionRequest, SavedRevision, StoredBlob,
};
use crate::project::{AnnotationProject, ProjectPage, ProjectSummary, SaveProjectRequest, Split};
use crate::schema::LabelSchema;
use crate::store::Backend;
use crate::{Error, Result, images, validate_digest, validate_name};

/// One namespace in the configured authored object store.
#[derive(Clone, Debug)]
pub struct Registry {
    backend: Backend,
}

impl Registry {
    #[must_use]
    pub fn new(store: Arc<dyn ObjectStore>, prefix: impl Into<String>) -> Self {
        Self {
            backend: Backend::new(store, prefix),
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
        let key = self.backend.project_key(&request.name);
        let existing: Option<AnnotationProject> = self.backend.read_json(&key).await?;
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
        self.backend.write_json(&key, &project).await?;
        Ok(project)
    }

    pub async fn projects(&self) -> Result<ProjectPage> {
        let mut projects = Vec::new();
        for key in self.backend.keys(&self.backend.projects_prefix()).await? {
            if !key.ends_with("/head.json") {
                continue;
            }
            if let Some(project) = self.backend.read_json::<AnnotationProject>(&key).await? {
                projects.push(project);
            }
        }
        projects.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(ProjectPage { projects })
    }

    pub async fn project(&self, name: &str) -> Result<AnnotationProject> {
        validate_name(name, "a project")?;
        self.backend
            .read_json(&self.backend.project_key(name))
            .await?
            .ok_or_else(|| Error::NotFound(format!("the project {name}")))
    }

    /// A project with the counts that answer "is there enough data yet".
    pub async fn project_summary(&self, name: &str) -> Result<ProjectSummary> {
        let project = self.project(name).await?;
        let heads = images::heads(&self.backend, name).await?;
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

    // ── Images ───────────────────────────────────────────────────────────────
    //
    // Every one of these resolves the project and hands over. The bodies live
    // in `crate::images`; what is here is the door and the project lookup.

    /// Store image bytes under the digest this server computes.
    ///
    /// # Errors
    /// When the upload is empty, over the blob size limit, or the store refuses.
    pub async fn put_blob(&self, body: Vec<u8>, content_type: &str) -> Result<StoredBlob> {
        images::put_blob(&self.backend, body, content_type).await
    }

    /// The bytes and content type of an uploaded image.
    ///
    /// # Errors
    /// When the id is not a digest, or nothing is stored under it.
    pub async fn blob(&self, image_id: &str) -> Result<(Vec<u8>, String)> {
        images::blob(&self.backend, image_id).await
    }

    /// Register an image into a project, or update what is known about it.
    ///
    /// # Errors
    /// When the project does not exist or the image does not validate.
    pub async fn register_image(&self, request: RegisterImageRequest) -> Result<ImageHead> {
        let project = self.project(&request.project).await?;
        images::register(&self.backend, &project, request).await
    }

    /// Register many images from rows a pipeline produced.
    ///
    /// # Errors
    /// When the project does not exist, the batch is over
    /// [`MAX_IMPORT_ROWS`](crate::images::import::MAX_IMPORT_ROWS), or the
    /// asserted rights over-claim against the curated table.
    pub async fn import_images(&self, request: ImportRequest) -> Result<ImportReport> {
        let project = self.project(&request.project).await?;
        images::import(&self.backend, &project, request).await
    }

    /// One page of a project's images, newest registration first.
    ///
    /// # Errors
    /// When the project does not exist.
    pub async fn images(
        &self,
        project: &str,
        filter: &ImageFilter,
        offset: usize,
        limit: usize,
    ) -> Result<ImagePage> {
        let definition = self.project(project).await?;
        images::list(&self.backend, &definition, filter, offset, limit).await
    }

    /// One image, with a revision's shapes: the one asked for, else the
    /// accepted one, else the newest.
    ///
    /// # Errors
    /// When the project or the image does not exist.
    pub async fn image(
        &self,
        project: &str,
        image_id: &str,
        revision: Option<&str>,
    ) -> Result<ImageDetail> {
        let definition = self.project(project).await?;
        images::detail(&self.backend, &definition, image_id, revision).await
    }

    /// Validate a drawing against the project's schema and store it.
    ///
    /// # Errors
    /// When the project or image does not exist, or the drawing does not
    /// validate — in which case every problem is reported, not the first.
    pub async fn save_revision(
        &self,
        request: SaveRevisionRequest,
        author: &str,
    ) -> Result<SavedRevision> {
        let project = self.project(&request.project).await?;
        images::save_revision(&self.backend, &project, request, author).await
    }

    /// Move an image's review state, and pin the revision an export will read.
    ///
    /// # Errors
    /// When the project or image does not exist, or an acceptance names no
    /// revision.
    pub async fn review(&self, request: ReviewRequest, reviewer: &str) -> Result<ImageHead> {
        let project = self.project(&request.project).await?;
        images::review(&self.backend, &project, request, reviewer).await
    }

    // ── Exports ──────────────────────────────────────────────────────────────

    /// Build, or find, the immutable manifest for the project as it stands.
    pub async fn export(&self, request: ExportRequest) -> Result<BuiltExport> {
        let project = self.project(&request.project).await?;
        let heads = images::heads(&self.backend, &project.name).await?;
        let mut pairs: Vec<(ImageHead, Option<AnnotationRevision>)> =
            Vec::with_capacity(heads.len());
        for head in heads {
            let revision = match head.accepted.as_ref() {
                Some(accepted) => {
                    images::revision(&self.backend, &project.name, &head.image.image_id, accepted)
                        .await?
                }
                None => None,
            };
            pairs.push((head, revision));
        }

        let manifest = crate::export::build(&project, &request, &pairs, OffsetDateTime::now_utc())?;
        let key = self.backend.export_key(&project.name, &manifest.export);
        let existing: Option<ExportManifest> = self.backend.read_json(&key).await?;
        let created = existing.is_none();
        let manifest = existing.unwrap_or(manifest);
        if created {
            // Manifest before index, same ordering as everything else here.
            self.backend.write_json(&key, &manifest).await?;
            let index_key = self.backend.export_index_key(&project.name);
            let mut index: ExportPage = self
                .backend
                .read_json(&index_key)
                .await?
                .unwrap_or_default();
            index
                .exports
                .retain(|summary| summary.export != manifest.export);
            index.exports.insert(0, ExportSummary::of(&manifest));
            self.backend.write_json(&index_key, &index).await?;
        }
        Ok(BuiltExport { manifest, created })
    }

    pub async fn exports(&self, project: &str) -> Result<ExportPage> {
        validate_name(project, "a project")?;
        Ok(self
            .backend
            .read_json(&self.backend.export_index_key(project))
            .await?
            .unwrap_or_default())
    }

    pub async fn export_manifest(&self, project: &str, export: &str) -> Result<ExportManifest> {
        validate_name(project, "a project")?;
        validate_digest(export, "an export")?;
        self.backend
            .read_json(&self.backend.export_key(project, export))
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
            if let Some(revision) =
                images::revision(&self.backend, project, &sample.image_id, &sample.revision).await?
            {
                revisions.insert(sample.image_id.clone(), revision);
            }
        }
        Ok(to_coco(&manifest, &definition.schema, &revisions, split))
    }
}
