//! What the registry does to an image, over the [`Backend`].
//!
//! Free functions rather than methods, and that is the whole shape of the
//! slice: they take the backend and an already-resolved project, so nothing
//! here can reach for a project it was not given and nothing here owns state.
//! [`Registry`](crate::Registry) resolves the project once and calls in.
//!
//! The ordering rule from [`crate::store`] lives here in one place —
//! [`save_revision`] writes the revision object before the head that indexes
//! it — because this is the only module that writes both.

use time::OffsetDateTime;

use crate::export::resolve_split;
use crate::images::{
    AnnotationRevision, BLOB_SCHEME, BlobMeta, ImageDetail, ImageFilter, ImageHead, ImagePage,
    ImageRecord, RegisterImageRequest, ReviewRequest, ReviewState, RevisionSummary,
    SaveRevisionRequest, SavedRevision, StoredBlob,
};
use crate::project::AnnotationProject;
use crate::store::{Backend, sniff};
use crate::{
    Error, MAX_ANNOTATIONS, MAX_BLOB_BYTES, MAX_IMAGE_PAGE, MAX_REVISION_BYTES, Result, digest,
    validate_digest, validate_name,
};

// ── Image bytes ──────────────────────────────────────────────────────────

/// Store an upload under the digest of its own bytes.
///
/// The digest is computed here and the client's claim about it is never
/// read. Content addressing whose address comes from the caller is not
/// content addressing.
pub(crate) async fn put_blob(
    backend: &Backend,
    body: Vec<u8>,
    content_type: &str,
) -> Result<StoredBlob> {
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
    let meta_key = backend.blob_meta_key(&image_id);
    let created = backend.read_json::<BlobMeta>(&meta_key).await?.is_none();
    let bytes = body.len();
    if created {
        // Bytes first, sidecar second: a described object that is not there
        // is worse than an undescribed one that is.
        backend
            .put_bytes(&backend.blob_key(&image_id), body)
            .await?;
        backend
            .write_json(
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
pub(crate) async fn blob(backend: &Backend, image_id: &str) -> Result<(Vec<u8>, String)> {
    validate_digest(image_id, "an image id")?;
    let body = backend
        .get_bytes(&backend.blob_key(image_id))
        .await?
        .ok_or_else(|| Error::NotFound(format!("the image {image_id}")))?;
    let content_type = backend
        .read_json::<BlobMeta>(&backend.blob_meta_key(image_id))
        .await?
        .map_or_else(|| sniff(&body).to_owned(), |meta| meta.content_type);
    Ok((body, content_type))
}

// ── Images ───────────────────────────────────────────────────────────────

/// Register an image into a project, or update what is known about it.
///
/// Re-registering keeps the revisions and the review state: the same plan
/// re-uploaded with a corrected `group_id` is a correction, not a reset.
/// Everything `register_image` checks before it touches the store.
///
/// Extracted so a dry run and a write cannot disagree about what is valid.
pub(crate) fn check(request: &RegisterImageRequest) -> Result<()> {
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
    Ok(())
}

pub(crate) async fn register(
    backend: &Backend,
    project: &AnnotationProject,
    request: RegisterImageRequest,
) -> Result<ImageHead> {
    check(&request)?;

    let key = backend.image_key(&project.name, &request.image_id);
    let existing: Option<ImageHead> = backend.read_json(&key).await?;
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
            image: ImageRecord {
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
    backend.write_json(&key, &head).await?;
    Ok(head)
}

pub(crate) async fn list(
    backend: &Backend,
    project: &AnnotationProject,
    filter: &ImageFilter,
    offset: usize,
    limit: usize,
) -> Result<ImagePage> {
    let limit = limit.clamp(1, MAX_IMAGE_PAGE);
    let needle = filter.search.as_ref().map(|value| value.to_lowercase());
    let mut heads: Vec<ImageHead> = heads(backend, &project.name)
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
                    project,
                    &head.image.group_id,
                    &project.split_salt,
                    project.splits,
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
pub(crate) async fn detail(
    backend: &Backend,
    project: &AnnotationProject,
    image_id: &str,
    revision: Option<&str>,
) -> Result<ImageDetail> {
    let head = head(backend, &project.name, image_id).await?;
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
            backend
                .read_json::<AnnotationRevision>(&backend.revision_key(
                    &project.name,
                    image_id,
                    &revision,
                ))
                .await?
        }
        None => None,
    };
    let split = resolve_split(
        project,
        &head.image.group_id,
        &project.split_salt,
        project.splits,
    );
    Ok(ImageDetail {
        head,
        revision,
        split,
    })
}

// ── Store plumbing ───────────────────────────────────────────────────────

pub(crate) async fn head(backend: &Backend, project: &str, image_id: &str) -> Result<ImageHead> {
    validate_digest(image_id, "an image id")?;
    backend
        .read_json(&backend.image_key(project, image_id))
        .await?
        .ok_or_else(|| Error::NotFound(format!("the image {image_id} in {project}")))
}

/// Every image head in a project.
///
/// A full listing rather than a page: the split, the counts and an export all
/// need every head, and a project is bounded by how many plans somebody drew.
///
/// # Errors
/// When the store refuses, or a stored head is not one.
pub(crate) async fn heads(backend: &Backend, project: &str) -> Result<Vec<ImageHead>> {
    let mut heads = Vec::new();
    for key in backend.keys(&backend.images_prefix(project)).await? {
        if !key.ends_with(".json") {
            continue;
        }
        if let Some(head) = backend.read_json::<ImageHead>(&key).await? {
            heads.push(head);
        }
    }
    Ok(heads)
}

/// One stored revision's shapes, or `None` when nothing is under that id.
///
/// # Errors
/// When the id is not a digest, or the stored object is not a revision.
pub(crate) async fn revision(
    backend: &Backend,
    project: &str,
    image_id: &str,
    revision: &str,
) -> Result<Option<AnnotationRevision>> {
    validate_digest(revision, "a revision")?;
    backend
        .read_json(&backend.revision_key(project, image_id, revision))
        .await
}

// ── Revisions ────────────────────────────────────────────────────────────

/// Validate a drawing against the project's schema and store it.
///
/// Validation is not advisory here: a revision that does not validate is
/// not stored at all. The alternative — storing it and marking it invalid —
/// puts the decision about whether a half-drawn door is a training target
/// into whatever reads it next.
pub(crate) async fn save_revision(
    backend: &Backend,
    project: &AnnotationProject,
    request: SaveRevisionRequest,
    author: &str,
) -> Result<SavedRevision> {
    let mut head = head(backend, &project.name, &request.image_id).await?;
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
    let key = backend.revision_key(&project.name, &request.image_id, &id);
    let existing: Option<AnnotationRevision> = backend.read_json(&key).await?;
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
        backend.write_json(&key, &revision).await?;
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
    backend
        .write_json(&backend.image_key(&project.name, &request.image_id), &head)
        .await?;

    Ok(SavedRevision {
        revision,
        head,
        created,
    })
}

/// Move an image's review state, and pin the revision an export will read.
pub(crate) async fn review(
    backend: &Backend,
    project: &AnnotationProject,
    request: ReviewRequest,
    reviewer: &str,
) -> Result<ImageHead> {
    let mut head = head(backend, &project.name, &request.image_id).await?;
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
    backend
        .write_json(&backend.image_key(&project.name, &request.image_id), &head)
        .await?;
    Ok(head)
}
