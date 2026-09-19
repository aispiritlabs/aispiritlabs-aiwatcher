//! Looking at the target, and copying into it.
//!
//! The executor's first act is to **plan again** from the live store and
//! refuse unless it reaches the manifest it was handed. That one step does
//! three jobs at once, and it is why nothing here trusts a path out of a file
//! somebody could have edited: the keys it writes are the ones the registries
//! just computed, the digests are the ones it just read, and a source that
//! moved under the migration produces a different plan and stops it.
//!
//! After that the loop is four reads and one write per object, in the
//! registries' own order:
//!
//! 1. the source's bytes, hashed and compared with the plan;
//! 2. the target's, so identical bytes are a no-op and different bytes are a
//!    conflict that overwrites nothing;
//! 3. the write, create-only unless the operator has stated the store is
//!    theirs alone;
//! 4. the target again, because a write that reported success and stored
//!    something else is the one failure nothing downstream catches;
//! 5. the checkpoint, only now.
//!
//! A checkpoint written after the read-back can only lag, and a lagging
//! checkpoint costs a repeated copy of bytes that are already identical. One
//! written before would lose an object.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aiwatcher_core::migration::{key_is_safe, sha256_hex};
use aiwatcher_core::ports::PortError;
use aiwatcher_core::prompts::ObjectStore;
use tokio::io::AsyncWriteExt as _;

use crate::authority::TargetAuthority;
use crate::manifest::{
    Blocker, BlockerKind, Checkpoint, Conflict, IamCheck, Manifest, PlannedObject, Publication,
    Receipt, Severity, Survey,
};
use crate::plan::{Source, plan};
use crate::{MigrationError, Result};

/// Where a run writes, and where it remembers how far it got.
#[derive(Clone, Debug)]
pub struct Destination {
    /// The operator's words for this store. Recorded in the checkpoint, and
    /// compared on resume: progress made against one store is not progress
    /// against another.
    pub label: String,
    pub checkpoint: PathBuf,
}

/// What an operator has stated about the store, beyond what it can be asked.
#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    /// The store cannot publish create-only, and this run has it to itself.
    ///
    /// Without this the executor refuses such a store rather than falling back
    /// to read-then-write, because read-then-write claims a safety against
    /// concurrent writers that it does not have.
    pub exclusive_access: bool,
}

/// What the target holds now, for a plan somebody is about to run.
///
/// Reads only. It writes nothing, including nothing to test whether it could.
///
/// # Errors
///
/// [`MigrationError::ManifestEdited`] when the manifest no longer matches its
/// own id, and whatever the store refuses.
pub async fn survey(
    store: &Arc<dyn ObjectStore>,
    manifest: &Manifest,
    authority: &dyn TargetAuthority,
) -> Result<Survey> {
    if !manifest.identity_holds()? {
        return Err(MigrationError::ManifestEdited);
    }
    let iam = authority.admit(manifest.body.target).await?;
    let mut absent = 0;
    let mut identical = 0;
    let mut conflicts = Vec::new();
    for object in &manifest.body.objects {
        match store.get(&object.target_key).await? {
            None => absent += 1,
            Some(bytes) if sha256_hex(&bytes) == object.sha256 => identical += 1,
            Some(bytes) => conflicts.push(Conflict {
                source_key: object.source_key.clone(),
                target_key: object.target_key.clone(),
                source_sha256: object.sha256.clone(),
                target_sha256: sha256_hex(&bytes),
            }),
        }
    }
    let mut blockers = manifest.body.blockers.clone();
    if let IamCheck::NotChecked { reason } = &iam {
        blockers.push(Blocker {
            kind: BlockerKind::NonAuthoritativeIam,
            severity: Severity::RefusesExecution,
            subject: "target".to_owned(),
            detail: reason.clone(),
        });
    }
    if let IamCheck::Verified {
        authoritative: false,
        authority: named,
        ..
    } = &iam
    {
        blockers.push(non_authoritative(named));
    }
    blockers.sort();
    blockers.dedup();
    Ok(Survey {
        manifest_id: manifest.manifest_id.clone(),
        target: manifest.body.target,
        iam,
        audience: authority.audience(manifest.body.target).await,
        absent,
        identical,
        conflicts,
        blockers,
    })
}

/// Copy the plan into the target, resuming whatever a checkpoint already
/// covers.
///
/// # Errors
///
/// Every refusal here leaves the target as it found it. In particular
/// [`MigrationError::SnapshotChanged`] when re-planning the live store no
/// longer reaches this manifest, [`MigrationError::Conflict`] when a target
/// key holds different bytes, and [`MigrationError::ReadBack`] when a write
/// reported success and stored something else.
pub async fn execute(
    store: &Arc<dyn ObjectStore>,
    manifest: &Manifest,
    destination: &Destination,
    authority: &dyn TargetAuthority,
    options: Options,
) -> Result<Receipt> {
    if !manifest.identity_holds()? {
        return Err(MigrationError::ManifestEdited);
    }
    let iam = authority.admit(manifest.body.target).await?;
    let IamCheck::Verified { .. } = &iam else {
        return Err(MigrationError::Authority(
            "an execution needs the deployment's IAM to confirm the target organization and \
             project; a dry run may be offline and a write may not"
                .to_owned(),
        ));
    };

    // Plan again, and require the same answer. This is the path revalidation,
    // the source-change check and the foreign-schema check, all at once.
    let source = Source {
        origin: manifest.body.snapshot.origin.clone(),
        prefixes: manifest.body.snapshot.prefixes.clone(),
    };
    let fresh = plan(store, &source, manifest.body.target).await?;
    if fresh.manifest_id != manifest.manifest_id {
        return Err(MigrationError::SnapshotChanged(format!(
            "the store no longer plans to the manifest under review ({} became {}). Re-plan, \
             review the difference, and only then run it",
            short(&manifest.manifest_id),
            short(&fresh.manifest_id)
        )));
    }
    let refused = fresh.refuses_execution();
    if !refused.is_empty() {
        return Err(MigrationError::Blocked(
            refused
                .iter()
                .map(|blocker| format!("{}: {}", blocker.subject, blocker.detail))
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }

    let mut checkpoint = load_checkpoint(&destination.checkpoint, &fresh, &destination.label)
        .await?
        .unwrap_or_else(|| Checkpoint::new(&fresh, &destination.label));
    let mut done: BTreeSet<String> = checkpoint.done.iter().cloned().collect();

    let publication = if options.exclusive_access {
        Publication::ExclusiveAccess
    } else {
        Publication::CreateOnly
    };
    let mut receipt = Receipt {
        manifest_id: fresh.manifest_id.clone(),
        target: fresh.body.target,
        iam: iam.clone(),
        // Who reaches what this run is about to copy, read before it copies
        // anything: an operator who finds the answer unacceptable stops here
        // rather than after the bytes have moved.
        audience: authority.audience(manifest.body.target).await,
        publication,
        written: 0,
        already_identical: 0,
        resumed: 0,
        bytes_written: 0,
        complete: false,
        cutover_ready: false,
        blockers: fresh.body.blockers.clone(),
    };

    for object in &fresh.body.objects {
        if done.contains(&object.target_key) {
            receipt.resumed += 1;
            continue;
        }
        check_target_key(&fresh, object)?;
        let bytes = read_source(store, object).await?;
        match store.get(&object.target_key).await? {
            Some(existing) if existing == bytes => receipt.already_identical += 1,
            Some(existing) => {
                return Err(MigrationError::Conflict {
                    source_key: object.source_key.clone(),
                    target_key: object.target_key.clone(),
                    target_sha256: sha256_hex(&existing),
                });
            }
            None => {
                publish(store, object, bytes.clone(), publication).await?;
                receipt.written += 1;
                receipt.bytes_written += object.bytes;
            }
        }
        read_back(store, object, &bytes).await?;
        done.insert(object.target_key.clone());
        checkpoint.done.push(object.target_key.clone());
        checkpoint.updated_at = time::OffsetDateTime::now_utc().unix_timestamp();
        store_checkpoint(&destination.checkpoint, &checkpoint).await?;
    }

    receipt.complete = fresh
        .body
        .objects
        .iter()
        .all(|object| done.contains(&object.target_key));
    if let IamCheck::Verified {
        authoritative: false,
        authority: named,
        ..
    } = &iam
    {
        receipt.blockers.push(non_authoritative(named));
    }
    receipt.blockers.sort();
    receipt.blockers.dedup();
    receipt.cutover_ready = receipt.complete && receipt.blockers.is_empty();
    Ok(receipt)
}

/// A target key is only ever one the registries just computed, and it is still
/// checked: against the family's own destination prefix, and against the
/// segments no key may contain.
fn check_target_key(manifest: &Manifest, object: &PlannedObject) -> Result<()> {
    let family =
        manifest.body.families.get(&object.family).ok_or_else(|| {
            MigrationError::Refused(format!("{} names no family", object.source_key))
        })?;
    let prefix = family.target_prefix.as_deref().ok_or_else(|| {
        MigrationError::Refused(format!(
            "{} declared no destination prefix, so nothing can check where its objects go",
            object.family
        ))
    })?;
    if !key_is_safe(&object.target_key) || !object.target_key.starts_with(prefix) {
        return Err(MigrationError::Refused(format!(
            "{} would be written to {}, which is outside {prefix}",
            object.source_key, object.target_key
        )));
    }
    Ok(())
}

/// The source's bytes, as the plan says they are.
async fn read_source(store: &Arc<dyn ObjectStore>, object: &PlannedObject) -> Result<Vec<u8>> {
    let bytes = store
        .get(&object.source_key)
        .await?
        .ok_or_else(|| MigrationError::SnapshotChanged(format!("{} is gone", object.source_key)))?;
    if sha256_hex(&bytes) != object.sha256 {
        return Err(MigrationError::SnapshotChanged(format!(
            "{} no longer holds the bytes it was planned from",
            object.source_key
        )));
    }
    Ok(bytes)
}

async fn publish(
    store: &Arc<dyn ObjectStore>,
    object: &PlannedObject,
    bytes: Vec<u8>,
    publication: Publication,
) -> Result<()> {
    match publication {
        Publication::ExclusiveAccess => Ok(store.put(&object.target_key, bytes).await?),
        Publication::CreateOnly => match store.create(&object.target_key, bytes.clone()).await {
            Ok(true) => Ok(()),
            // Somebody else got there first. Identical bytes are the safe
            // repetition this whole design is built around; different ones are
            // a conflict, and neither is an overwrite.
            Ok(false) => match store.get(&object.target_key).await? {
                Some(existing) if existing == bytes => Ok(()),
                Some(existing) => Err(MigrationError::Conflict {
                    source_key: object.source_key.clone(),
                    target_key: object.target_key.clone(),
                    target_sha256: sha256_hex(&existing),
                }),
                None => Err(MigrationError::SnapshotChanged(format!(
                    "{} refused a create and then held nothing",
                    object.target_key
                ))),
            },
            Err(PortError::Rejected { message, .. }) => Err(MigrationError::NoAtomicCreate {
                key: object.target_key.clone(),
                message,
            }),
            Err(other) => Err(other.into()),
        },
    }
}

async fn read_back(
    store: &Arc<dyn ObjectStore>,
    object: &PlannedObject,
    bytes: &[u8],
) -> Result<()> {
    match store.get(&object.target_key).await? {
        Some(stored) if stored == bytes && sha256_hex(&stored) == object.sha256 => Ok(()),
        _ => Err(MigrationError::ReadBack {
            key: object.target_key.clone(),
        }),
    }
}

fn non_authoritative(named: &str) -> Blocker {
    Blocker {
        kind: BlockerKind::NonAuthoritativeIam,
        severity: Severity::RefusesCutover,
        subject: named.to_owned(),
        detail: "the target was admitted by an IAM store no deployment authorizes from. The \
                 copy is faithful and this is a rehearsal, not a cutover"
            .to_owned(),
    }
}

fn short(id: &str) -> &str {
    &id[..id.len().min(12)]
}

// ── The checkpoint file ──────────────────────────────────────────────────────

/// Read progress, and refuse progress that belongs to something else.
///
/// # Errors
///
/// [`MigrationError::CheckpointMismatch`] when the file is another plan's,
/// another target's or another store's — which is the case that would
/// otherwise skip objects that were never copied.
pub async fn load_checkpoint(
    path: &Path,
    manifest: &Manifest,
    destination: &str,
) -> Result<Option<Checkpoint>> {
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let checkpoint: Checkpoint =
        serde_json::from_slice(&bytes).map_err(|_| MigrationError::CheckpointMismatch {
            path: path.display().to_string(),
            detail: "it is not a migration checkpoint".to_owned(),
        })?;
    if !checkpoint.binds_to(manifest, destination) {
        return Err(MigrationError::CheckpointMismatch {
            path: path.display().to_string(),
            detail: format!(
                "it records progress for plan {} into organization {} project {} at {}",
                short(&checkpoint.manifest_id),
                checkpoint.target.organization.0,
                checkpoint.target.project.0,
                checkpoint.destination
            ),
        });
    }
    Ok(Some(checkpoint))
}

/// Write progress so that a power cut leaves either the old file or the new
/// one, and never half of either.
async fn store_checkpoint(path: &Path, checkpoint: &Checkpoint) -> Result<()> {
    let staging = path.with_extension("writing");
    let bytes = serde_json::to_vec_pretty(checkpoint)?;
    let mut file = tokio::fs::File::create(&staging).await?;
    file.write_all(&bytes).await?;
    file.sync_all().await?;
    drop(file);
    tokio::fs::rename(&staging, path).await?;
    // The rename itself is only durable once the directory is.
    if let Some(directory) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::File::open(directory).await?.sync_all().await?;
    }
    Ok(())
}
