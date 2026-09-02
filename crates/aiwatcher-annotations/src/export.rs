//! Turning a project into something a training run can name.
//!
//! An export is a manifest: which images, on which side of the split, at which
//! accepted revision, under which label schema, with every exclusion and its
//! reason. It is content-addressed, so `project@export-sha256` is the string a
//! `train.started` event carries and the thing two training runs can be
//! compared through — the same rule ADR_0015 established for datasets.
//!
//! What it is not is an archive. The images already exist; copying them per
//! export would multiply storage by the number of times somebody re-dealt the
//! split. COCO is generated from the manifest on request because COCO is only
//! JSON; masks and heatmaps are generated in Python, where the array libraries
//! already are.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::images::{AnnotationRevision, ImageHead};
use crate::license::RightsPolicy;
use crate::project::{AnnotationProject, Split, SplitRatios};
use crate::schema::{GeometryKind, LabelSchema};
use crate::shapes::Geometry;
use crate::{Error, Result, digest, validate_name};

/// Why an image is not in an export.
///
/// Every one of these is listed by name in the manifest rather than silently
/// dropped. An export that quietly loses a third of a corpus reads exactly
/// like one that did not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExclusionReason {
    /// Its usage rights do not satisfy the export's policy.
    Rights,
    /// Nothing has been accepted on it.
    Unreviewed,
    /// Accepted, and the accepted revision has no shapes.
    Empty,
    /// The accepted revision was drawn against a different label schema.
    SchemaMismatch,
    /// It is a section or an elevation, not a plan.
    View,
    /// Its accepted revision is named by the head and missing from the store.
    Missing,
    /// It has shapes, and none of them are in the requested class subset.
    NoRequestedClass,
}

impl ExclusionReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rights => "rights",
            Self::Unreviewed => "unreviewed",
            Self::Empty => "empty",
            Self::SchemaMismatch => "schema_mismatch",
            Self::View => "view",
            Self::Missing => "missing",
            Self::NoRequestedClass => "no_requested_class",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ExportExclusion {
    pub image_id: String,
    pub group_id: String,
    pub reason: ExclusionReason,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

/// One image in an export, pinned to the revision that was accepted when the
/// export was built.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ExportSample {
    pub image_id: String,
    pub uri: String,
    pub width: u32,
    pub height: u32,
    pub group_id: String,
    pub split: Split,
    pub revision: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// Flattened for the manifest so a reader can see the licence mix without
    /// resolving every image.
    pub rights: String,
    pub instances: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct ExportCounts {
    pub images: usize,
    pub groups: usize,
    pub instances: usize,
    pub excluded: usize,
    /// `train | validation | test` to image count.
    pub images_per_split: BTreeMap<String, usize>,
    pub groups_per_split: BTreeMap<String, usize>,
    /// Class to instance count, over the whole export.
    pub instances_per_class: BTreeMap<String, usize>,
    /// Class to per-split instance counts. This is the table that says the test
    /// set contains four doors, which is the number that invalidates a recall
    /// figure before anybody quotes it.
    pub instances_per_class_split: BTreeMap<String, BTreeMap<String, usize>>,
}

/// What a caller asks for.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportRequest {
    pub project: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub rights_policy: RightsPolicy,
    /// Refuse an image whose accepted revision was produced entirely by a model
    /// or an import. Defaults to on, because that is the whole point of
    /// recording an origin.
    #[serde(default = "default_true")]
    pub require_human_review: bool,
    /// Empty means every class in the schema.
    #[serde(default)]
    pub classes: Vec<String>,
    /// Overrides the project's ratios for this export only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub splits: Option<SplitRatios>,
    /// Overrides the project's salt for this export only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split_salt: Option<String>,
    /// Which [`views`](crate::images::ImageRecord::view) to include.
    ///
    /// Empty means every one, which is the right default for a corpus that
    /// has only one kind of picture — most of them. A corpus that mixes kinds
    /// names the ones a model reads, and every other image is excluded *by
    /// name* in the manifest rather than quietly.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub views: Vec<String>,
}

/// The immutable result.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ExportManifest {
    /// SHA-256 of everything below except `created_at` and `note`. Two exports
    /// of an unchanged project are one export.
    pub export: String,
    pub project: String,
    pub schema_version: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    pub rights_policy: RightsPolicy,
    pub require_human_review: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub views: Vec<String>,
    pub splits: SplitRatios,
    pub split_salt: String,
    /// In schema order. The index in this list is the category id COCO uses,
    /// which is why the order is fixed by the schema and not by first sight.
    pub classes: Vec<String>,
    pub samples: Vec<ExportSample>,
    pub excluded: Vec<ExportExclusion>,
    pub counts: ExportCounts,
}

impl ExportManifest {
    /// `project@export`, the string a training run records.
    #[must_use]
    pub fn reference(&self) -> String {
        format!("{}@{}", self.project, self.export)
    }

    #[must_use]
    pub fn sample(&self, image_id: &str) -> Option<&ExportSample> {
        self.samples
            .iter()
            .find(|sample| sample.image_id == image_id)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ExportSummary {
    pub export: String,
    pub project: String,
    pub schema_version: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    pub rights_policy: RightsPolicy,
    pub counts: ExportCounts,
}

impl ExportSummary {
    #[must_use]
    pub fn of(manifest: &ExportManifest) -> Self {
        Self {
            export: manifest.export.clone(),
            project: manifest.project.clone(),
            schema_version: manifest.schema_version.clone(),
            created_at: manifest.created_at,
            note: manifest.note.clone(),
            rights_policy: manifest.rights_policy,
            counts: manifest.counts.clone(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct ExportPage {
    pub exports: Vec<ExportSummary>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct BuiltExport {
    pub manifest: ExportManifest,
    /// False when this exact export already existed.
    pub created: bool,
}

/// Which side of the split a family falls on.
///
/// Deterministic in `group_id` and the salt, and *only* in those: adding an
/// image never moves an existing family, and adding a whole new supplier never
/// re-deals the old one. A shuffle-and-slice would do both, which is how a test
/// set quietly acquires images it has already been trained on.
#[must_use]
pub fn split_for(group_id: &str, salt: &str, ratios: SplitRatios) -> Split {
    let mut hasher = Sha256::new();
    hasher.update(salt.as_bytes());
    hasher.update([0]);
    hasher.update(group_id.as_bytes());
    let bytes = hasher.finalize();
    let mut head = [0_u8; 8];
    head.copy_from_slice(&bytes[..8]);
    let bucket = u64::from_be_bytes(head) % 100;
    if bucket < u64::from(ratios.train) {
        Split::Train
    } else if bucket < u64::from(ratios.train + ratios.validation) {
        Split::Validation
    } else {
        Split::Test
    }
}

/// Resolve a family's split, honouring the project's explicit overrides.
#[must_use]
pub fn resolve_split(
    project: &AnnotationProject,
    group_id: &str,
    salt: &str,
    ratios: SplitRatios,
) -> Split {
    project
        .split_overrides
        .get(group_id)
        .copied()
        .unwrap_or_else(|| split_for(group_id, salt, ratios))
}

/// Assemble a manifest from every image head and the revisions they accept.
///
/// Pure: the caller does the reading, this does the deciding. That is what
/// makes the split rule, the rights rule and the review rule testable without
/// an object store.
pub fn build(
    project: &AnnotationProject,
    request: &ExportRequest,
    images: &[(ImageHead, Option<AnnotationRevision>)],
    created_at: OffsetDateTime,
) -> Result<ExportManifest> {
    validate_name(&request.project, "a project")?;
    let ratios = request.splits.unwrap_or(project.splits);
    ratios.validate()?;
    let salt = request
        .split_salt
        .clone()
        .unwrap_or_else(|| project.split_salt.clone());

    let classes = if request.classes.is_empty() {
        project.schema.class_names()
    } else {
        for class in &request.classes {
            if project.schema.class(class).is_none() {
                return Err(Error::Invalid(format!(
                    "the class {class} is not in this project's schema"
                )));
            }
        }
        // Schema order, not request order: the category ids must not depend on
        // how somebody happened to type the filter.
        project
            .schema
            .class_names()
            .into_iter()
            .filter(|name| request.classes.contains(name))
            .collect()
    };

    let mut samples: Vec<ExportSample> = Vec::new();
    let mut excluded: Vec<ExportExclusion> = Vec::new();
    let mut counts = ExportCounts::default();
    let mut groups: BTreeMap<String, Split> = BTreeMap::new();

    for (head, revision) in images {
        let group_id = head.image.group_id.clone();
        let mut exclude = |reason: ExclusionReason, detail: String| {
            excluded.push(ExportExclusion {
                image_id: head.image.image_id.clone(),
                group_id: group_id.clone(),
                reason,
                detail,
            });
        };

        if !request.views.is_empty() && !request.views.contains(&head.image.view) {
            exclude(
                ExclusionReason::View,
                format!(
                    "the view is {:?}; this export takes {}",
                    head.image.view,
                    request.views.join(", ")
                ),
            );
            continue;
        }
        if !head.image.rights.allows(request.rights_policy) {
            exclude(
                ExclusionReason::Rights,
                format!(
                    "{} does not satisfy a {} export",
                    head.image.rights.summary(),
                    request.rights_policy.as_str()
                ),
            );
            continue;
        }
        let Some(accepted) = head.accepted.as_ref() else {
            exclude(
                ExclusionReason::Unreviewed,
                format!("review is {}", head.review.as_str()),
            );
            continue;
        };
        let Some(revision) = revision else {
            exclude(
                ExclusionReason::Missing,
                format!("the head accepts {accepted}, which is not in the store"),
            );
            continue;
        };
        if revision.schema_version != project.schema.version {
            exclude(
                ExclusionReason::SchemaMismatch,
                format!(
                    "drawn against schema {}",
                    &revision.schema_version[..12.min(revision.schema_version.len())]
                ),
            );
            continue;
        }
        if request.require_human_review && !RevisionOrigins::of(revision).touched_by_a_human() {
            exclude(
                ExclusionReason::Unreviewed,
                "every shape came from a model or an import".to_owned(),
            );
            continue;
        }
        let kept: Vec<_> = revision
            .annotations
            .iter()
            .filter(|annotation| classes.contains(&annotation.class))
            .collect();
        if revision.annotations.is_empty() {
            exclude(ExclusionReason::Empty, String::new());
            continue;
        }
        if kept.is_empty() {
            exclude(
                ExclusionReason::NoRequestedClass,
                format!(
                    "{} shapes, none in the requested classes",
                    revision.annotations.len()
                ),
            );
            continue;
        }

        let split = resolve_split(project, &group_id, &salt, ratios);
        groups.insert(group_id.clone(), split);
        for annotation in &kept {
            *counts
                .instances_per_class
                .entry(annotation.class.clone())
                .or_default() += 1;
            *counts
                .instances_per_class_split
                .entry(annotation.class.clone())
                .or_default()
                .entry(split.as_str().to_owned())
                .or_default() += 1;
        }
        counts.instances += kept.len();
        *counts
            .images_per_split
            .entry(split.as_str().to_owned())
            .or_default() += 1;

        samples.push(ExportSample {
            image_id: head.image.image_id.clone(),
            uri: head.image.uri.clone(),
            width: head.image.width,
            height: head.image.height,
            group_id,
            split,
            revision: revision.revision.clone(),
            source: head.image.source.clone(),
            rights: head.image.rights.summary(),
            instances: kept.len(),
            level: head.image.level.clone(),
        });
    }

    // Stable order, so the digest does not depend on how the store listed.
    samples.sort_by(|left, right| left.image_id.cmp(&right.image_id));
    excluded.sort_by(|left, right| left.image_id.cmp(&right.image_id));
    counts.images = samples.len();
    counts.excluded = excluded.len();
    counts.groups = groups.len();
    for split in groups.values() {
        *counts
            .groups_per_split
            .entry(split.as_str().to_owned())
            .or_default() += 1;
    }

    let identity = serde_json::to_vec(&json!({
        "project": request.project,
        "schema_version": project.schema.version,
        "rights_policy": request.rights_policy,
        "require_human_review": request.require_human_review,
        "views": request.views,
        "splits": ratios,
        "split_salt": salt,
        "classes": classes,
        "samples": samples,
        "excluded": excluded,
    }))
    .map_err(|error| Error::Invalid(format!("the export could not be encoded: {error}")))?;

    Ok(ExportManifest {
        export: digest(&identity),
        project: request.project.clone(),
        schema_version: project.schema.version.clone(),
        created_at,
        note: request.note.clone(),
        rights_policy: request.rights_policy,
        require_human_review: request.require_human_review,
        views: request.views.clone(),
        splits: ratios,
        split_salt: salt,
        classes,
        samples,
        excluded,
        counts,
    })
}

/// A cheap view over a revision's origins, so `build` does not have to compute
/// the whole [`crate::project::RevisionSummary`] to answer one question.
struct RevisionOrigins {
    human: usize,
}

impl RevisionOrigins {
    fn of(revision: &AnnotationRevision) -> Self {
        Self {
            human: revision
                .annotations
                .iter()
                .filter(|annotation| annotation.origin == crate::shapes::Origin::Human)
                .count(),
        }
    }

    const fn touched_by_a_human(&self) -> bool {
        self.human > 0
    }
}

/// Render an export as a COCO document.
///
/// COCO because every detection and segmentation trainer reads it, and because
/// it is only JSON — the part of an export that needs numpy is the part that
/// stays in Python. Polylines and keypoint sets have no COCO segmentation, so
/// they carry their vector geometry in the `aiwatcher` extension field on each
/// annotation, and a consumer that only understands COCO sees a bounding box.
///
/// `split` filters to one side. Passing `None` produces the whole export with a
/// `split` field on each image, which is what a consumer that does its own
/// batching wants.
#[must_use]
pub fn to_coco(
    manifest: &ExportManifest,
    schema: &LabelSchema,
    revisions: &BTreeMap<String, AnnotationRevision>,
    split: Option<Split>,
) -> Value {
    let categories: Vec<Value> = manifest
        .classes
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let class = schema.class(name);
            json!({
                "id": index + 1,
                "name": name,
                "supercategory": class.map_or("shape", |class| class.geometry.as_str()),
                "aiwatcher": {
                    "geometry": class.map_or(GeometryKind::Polygon, |class| class.geometry).as_str(),
                    "keypoints": class.map(|class| class.keypoints.clone()).unwrap_or_default(),
                    "ignore": class.is_some_and(|class| class.ignore),
                },
            })
        })
        .collect();

    let mut images = Vec::new();
    let mut annotations = Vec::new();
    let mut annotation_id = 1_u64;

    for (index, sample) in manifest.samples.iter().enumerate() {
        if split.is_some_and(|wanted| wanted != sample.split) {
            continue;
        }
        let image_id = index + 1;
        images.push(json!({
            "id": image_id,
            "file_name": sample.image_id,
            "coco_url": sample.uri,
            "width": sample.width,
            "height": sample.height,
            "aiwatcher": {
                "image_id": sample.image_id,
                "group_id": sample.group_id,
                "split": sample.split.as_str(),
                "revision": sample.revision,
                "source": sample.source,
                "rights": sample.rights,
                "level": sample.level,
            },
        }));

        let Some(revision) = revisions.get(&sample.image_id) else {
            continue;
        };
        for annotation in &revision.annotations {
            let Some(category) = manifest
                .classes
                .iter()
                .position(|name| name == &annotation.class)
            else {
                continue;
            };
            let bbox = annotation.geometry.bounds().unwrap_or([0.0, 0.0, 0.0, 0.0]);
            let mut record = json!({
                "id": annotation_id,
                "image_id": image_id,
                "category_id": category + 1,
                "bbox": bbox,
                "area": if annotation.geometry.area() > 0.0 {
                    annotation.geometry.area()
                } else {
                    bbox[2] * bbox[3]
                },
                "iscrowd": 0,
                "aiwatcher": {
                    "annotation_id": annotation.id,
                    "origin": annotation.origin.as_str(),
                    "attributes": annotation.attributes,
                    "links": annotation.links,
                    "text": annotation.text,
                    "geometry": annotation.geometry,
                },
            });
            if let Geometry::Polygon { exterior, holes } = &annotation.geometry {
                let mut rings = vec![flatten(exterior)];
                rings.extend(holes.iter().map(|hole| flatten(hole)));
                record["segmentation"] = json!(rings);
            }
            if let Geometry::Keypoints { points } = &annotation.geometry
                && let Some(class) = schema.class(&annotation.class)
            {
                let mut flat: Vec<f64> = Vec::with_capacity(class.keypoints.len() * 3);
                for name in &class.keypoints {
                    match points.iter().find(|point| &point.name == name) {
                        Some(point) => {
                            flat.push(point.at[0]);
                            flat.push(point.at[1]);
                            flat.push(if point.visible { 2.0 } else { 1.0 });
                        }
                        None => flat.extend([0.0, 0.0, 0.0]),
                    }
                }
                record["num_keypoints"] = json!(points.len());
                record["keypoints"] = json!(flat);
            }
            annotations.push(record);
            annotation_id += 1;
        }
    }

    json!({
        "info": {
            "description": format!("aiwatcher annotation export {}", manifest.reference()),
            "version": manifest.export,
            "date_created": manifest.created_at.to_string(),
            "aiwatcher": {
                "project": manifest.project,
                "export": manifest.export,
                "schema_version": manifest.schema_version,
                "rights_policy": manifest.rights_policy.as_str(),
                "split": split.map(Split::as_str),
                "splits": manifest.splits,
                "split_salt": manifest.split_salt,
            },
        },
        "licenses": [],
        "categories": categories,
        "images": images,
        "annotations": annotations,
    })
}

fn flatten(ring: &[crate::shapes::Point]) -> Vec<f64> {
    ring.iter().flat_map(|point| [point[0], point[1]]).collect()
}

const fn default_true() -> bool {
    true
}
