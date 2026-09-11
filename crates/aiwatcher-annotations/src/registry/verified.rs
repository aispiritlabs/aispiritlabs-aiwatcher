//! Export evidence checked against immutable content and current access decisions.
use super::Registry;
use crate::store::Backend;
use crate::{
    AnnotationProject, AnnotationRevision, BLOB_SCHEME, Error, ExportManifest, ImageHead,
    LabelSchema, MAX_BLOB_BYTES, MAX_REVISION_BYTES, Result, ReviewState, RightsPolicy, Split,
    digest, validate_digest, validate_name,
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

struct Budget(usize);
impl Budget {
    async fn bytes(&mut self, backend: &Backend, key: &str, limit: usize) -> Result<Vec<u8>> {
        let body = backend
            .get_bytes(key)
            .await?
            .ok_or_else(|| Error::NotFound(key.into()))?;
        let limit = limit.min(self.0);
        if body.len() > limit {
            return Err(Error::TooLarge {
                what: "annotation evidence bytes",
                size: body.len(),
                limit,
            });
        }
        self.0 -= body.len();
        Ok(body)
    }
    async fn json<T: DeserializeOwned>(
        &mut self,
        backend: &Backend,
        key: &str,
        limit: usize,
    ) -> Result<T> {
        serde_json::from_slice(&self.bytes(backend, key, limit).await?)
            .map_err(|e| corrupt(key, e.to_string()))
    }
}
fn corrupt(key: &str, message: impl Into<String>) -> Error {
    Error::Corrupt {
        key: key.into(),
        message: message.into(),
    }
}
fn refused(message: &str) -> Error {
    Error::Rejected(vec![message.into()])
}

impl Registry {
    /// Verify one split's export, pinned revisions and stored image bytes.
    ///
    /// Current rights and review must still permit use. External image URLs
    /// are not fetched. A changed schema is refused because older schemas are
    /// not stored separately. The export index and latest revision are not pins.
    /// Reads allow 1,000 export samples and 100 MiB of stored content in total.
    pub async fn verified_coco(&self, project: &str, export: &str, split: Split) -> Result<Value> {
        validate_name(project, "a project")?;
        validate_digest(export, "an export")?;
        let backend = &self.backend;
        let mut budget = Budget(100 * 1024 * 1024);
        let key = backend.export_key(project, export);
        let manifest: ExportManifest = budget.json(backend, &key, 4 * 1024 * 1024).await?;
        if manifest.project != project
            || manifest.export != export
            || manifest.content_digest()? != export
        {
            return Err(corrupt(&key, "export content does not match its identity"));
        }
        if manifest.samples.len() > 1000 {
            return Err(Error::TooLarge {
                what: "export samples",
                size: manifest.samples.len(),
                limit: 1000,
            });
        }
        if manifest.rights_policy == RightsPolicy::Any {
            return Err(refused(
                "verified evidence requires commercial or research rights",
            ));
        }
        let project_key = backend.project_key(project);
        let definition: AnnotationProject =
            budget.json(backend, &project_key, 4 * 1024 * 1024).await?;
        let schema = LabelSchema::build(
            definition.schema.classes.clone(),
            definition.schema.created_at,
        )
        .map_err(|e| corrupt(&project_key, e.to_string()))?;
        if definition.name != project || schema.version != definition.schema.version {
            return Err(corrupt(
                &project_key,
                "project schema content does not match its identity",
            ));
        }
        if schema.version != manifest.schema_version {
            return Err(refused("the pinned export schema is no longer available"));
        }
        let classes: Vec<_> = schema
            .class_names()
            .into_iter()
            .filter(|name| manifest.classes.contains(name))
            .collect();
        if classes != manifest.classes {
            return Err(corrupt(&key, "invalid export class order"));
        }
        let mut seen = BTreeSet::new();
        let mut revisions = BTreeMap::new();
        for sample in &manifest.samples {
            validate_digest(&sample.image_id, "an image")
                .map_err(|e| corrupt(&key, e.to_string()))?;
            validate_digest(&sample.revision, "a revision")
                .map_err(|e| corrupt(&key, e.to_string()))?;
            if !seen.insert(&sample.image_id) {
                return Err(corrupt(&key, "duplicate sample"));
            }
            if sample.split != split {
                continue;
            }
            let head_key = backend.image_key(project, &sample.image_id);
            let head: ImageHead = budget.json(backend, &head_key, 4 * 1024 * 1024).await?;
            if head.project != project || head.image.image_id != sample.image_id {
                return Err(corrupt(&head_key, "image identity mismatch"));
            }
            if !head.image.rights.allows(manifest.rights_policy)
                || head.review != ReviewState::Accepted
                || head.accepted.is_none()
            {
                return Err(refused(
                    "image rights or review no longer permit this export",
                ));
            }
            // Only locally stored content can establish byte integrity here.
            let uri = format!("{BLOB_SCHEME}{}", sample.image_id);
            if sample.uri != uri || head.image.uri != uri {
                return Err(refused("verified exports require stored image blobs"));
            }
            if head.image.width != sample.width
                || head.image.height != sample.height
                || head.image.group_id != sample.group_id
            {
                return Err(refused("image dimensions or grouping changed since export"));
            }
            let revision_key = backend.revision_key(project, &sample.image_id, &sample.revision);
            let revision: AnnotationRevision = budget
                .json(backend, &revision_key, MAX_REVISION_BYTES + 256 * 1024)
                .await?;
            let identity = crate::images::revision_identity(
                &revision.project,
                &revision.image_id,
                &revision.schema_version,
                &revision.annotations,
            )?;
            if revision.project != project
                || revision.image_id != sample.image_id
                || revision.schema_version != manifest.schema_version
                || revision.revision != sample.revision
                || digest(&identity) != sample.revision
                || identity.len() > MAX_REVISION_BYTES
            {
                return Err(corrupt(
                    &revision_key,
                    "revision content does not match its pin",
                ));
            }
            crate::shapes::validate(&revision.annotations, &schema, sample.width, sample.height)
                .map_err(|e| corrupt(&revision_key, e.to_string()))?;
            if revision
                .annotations
                .iter()
                .filter(|a| manifest.classes.contains(&a.class))
                .count()
                != sample.instances
            {
                return Err(corrupt(&key, "sample instance count mismatch"));
            }
            if manifest.require_human_review
                && !crate::RevisionSummary::of(&revision).touched_by_a_human()
            {
                return Err(refused("the pinned revision has no human contribution"));
            }
            let blob_key = backend.blob_key(&sample.image_id);
            let body = budget.bytes(backend, &blob_key, MAX_BLOB_BYTES).await?;
            if digest(&body) != sample.image_id {
                return Err(corrupt(&blob_key, "image bytes do not match their digest"));
            }
            revisions.insert(sample.image_id.clone(), revision);
        }
        Ok(crate::to_coco(&manifest, &schema, &revisions, Some(split)))
    }
}
