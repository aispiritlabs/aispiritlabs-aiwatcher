//! Building a manifest from a store, and nothing else.
//!
//! This is the composition point: it constructs each supported registry at the
//! prefix this deployment configured, asks that registry what it holds, and
//! assembles the answers. It computes no object key of its own — the one piece
//! of string arithmetic here is finding which configured prefix a listed key
//! falls under, so that a key under none of them is reported rather than
//! walked past.
//!
//! Everything it does is a read.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use aiwatcher_core::migration::{Inventory, InventoryError, ReferenceKind, sha256_hex};
use aiwatcher_core::prompts::ObjectStore;
use aiwatcher_iam::ProjectScope;

use crate::family::{FAMILIES, Family, Support};
use crate::manifest::{
    Blocker, BlockerKind, Body, FamilyReport, FamilyState, MANIFEST_SCHEMA, Manifest,
    MissingReference, PlannedObject, References, Severity, Snapshot,
};
use crate::{MigrationError, Result};

/// At most this many foreign keys are listed per family. The count is always
/// exact; the list is there to start the investigation, not to hold it.
const FOREIGN_SHOWN: usize = 20;

/// What a plan is taken against.
#[derive(Clone, Debug)]
pub struct Source {
    /// The operator's own words for this store — `fs:/snapshots/2026-09-18`.
    /// Recorded in the manifest and compared on resume.
    pub origin: String,
    /// Family name to the prefix this deployment configured for it. Anything
    /// absent uses the family's default.
    pub prefixes: BTreeMap<String, String>,
}

impl Source {
    #[must_use]
    pub fn new(origin: impl Into<String>) -> Self {
        Self {
            origin: origin.into(),
            prefixes: BTreeMap::new(),
        }
    }

    /// Override one family's prefix. The prompt registry's is a deployment
    /// setting (`AIWATCHER_PROMPT_PREFIX`), so a migration that assumed the
    /// default would inventory an empty store and report a success.
    #[must_use]
    pub fn with_prefix(mut self, family: &str, prefix: &str) -> Self {
        self.prefixes
            .insert(family.to_owned(), prefix.trim_matches('/').to_owned());
        self
    }

    fn prefix_of(&self, family: &Family) -> String {
        self.prefixes
            .get(family.name)
            .cloned()
            .unwrap_or_else(|| family.default_prefix.to_owned())
    }
}

/// Read a store and describe what moving it into `target` would be.
///
/// # Errors
///
/// [`MigrationError::Refused`] when two families are configured onto one
/// prefix, or when a mapping would put two source objects on one target key.
/// [`MigrationError::Inventory`] when a registry refuses to describe itself.
pub async fn plan(
    store: &Arc<dyn ObjectStore>,
    source: &Source,
    target: ProjectScope,
) -> Result<Manifest> {
    let mut prefixes = BTreeMap::new();
    for family in FAMILIES {
        let prefix = source.prefix_of(family);
        if let Some(other) = prefixes.insert(prefix.clone(), family.name) {
            return Err(MigrationError::Refused(format!(
                "{other} and {} are both configured onto the prefix {prefix}",
                family.name
            )));
        }
    }
    let mut listing: Vec<(String, u64)> = store
        .list("")
        .await?
        .into_iter()
        .map(|entry| (entry.key, entry.size))
        .collect();
    listing.sort();

    let mut families = BTreeMap::new();
    let mut blockers = Vec::new();
    let mut objects: Vec<PlannedObject> = Vec::new();
    let mut inventory_lines = String::new();
    let mut written_prefixes: Vec<String> = Vec::new();
    let mut target_prefixes: BTreeMap<&str, String> = BTreeMap::new();

    for family in FAMILIES {
        let prefix = source.prefix_of(family);
        let under = format!("{prefix}/");
        let keys: Vec<&(String, u64)> = listing
            .iter()
            .filter(|(key, _)| key.starts_with(&under))
            .collect();
        let count = keys.len() as u64;
        let bytes = keys.iter().map(|(_, size)| size).sum();
        let state = match family.support {
            Support::Unsupported(reason) => {
                blockers.push(Blocker {
                    kind: BlockerKind::UnsupportedFamily,
                    severity: Severity::RefusesCutover,
                    subject: family.name.to_owned(),
                    detail: format!("{reason}. {count} objects stay where they are"),
                });
                FamilyState::Unsupported {
                    reason: reason.to_owned(),
                    objects: count,
                    bytes,
                }
            }
            Support::Blocked(reason) => {
                blockers.push(Blocker {
                    kind: BlockerKind::BlockedFamily,
                    severity: Severity::RefusesCutover,
                    subject: family.name.to_owned(),
                    detail: format!("{reason}. {count} objects stay where they are"),
                });
                FamilyState::Blocked {
                    reason: reason.to_owned(),
                    objects: count,
                    bytes,
                }
            }
            Support::Supported => match inventory_of(family, store, &prefix, target).await {
                Err(InventoryError::Damaged { key, message }) => {
                    blockers.push(Blocker {
                        kind: BlockerKind::DamagedObject,
                        severity: Severity::RefusesExecution,
                        subject: key.clone(),
                        detail: message.clone(),
                    });
                    FamilyState::Damaged { key, message }
                }
                Err(other) => {
                    return Err(MigrationError::Inventory {
                        family: family.name.to_owned(),
                        source: other,
                    });
                }
                Ok(inventory) => {
                    written_prefixes.push(inventory.target_prefix.clone());
                    target_prefixes.insert(family.name, inventory.target_prefix.clone());
                    let unaccounted =
                        unaccounted_keys(&inventory, keys.iter().map(|(key, _)| key.as_str()));
                    if !unaccounted.is_empty() {
                        blockers.push(Blocker {
                            kind: BlockerKind::ForeignObject,
                            severity: Severity::RefusesExecution,
                            subject: family.name.to_owned(),
                            detail: format!(
                                "{} keys under {prefix}/ are not this registry's own \
                                     documents, starting with {}",
                                unaccounted.len(),
                                unaccounted
                                    .iter()
                                    .take(3)
                                    .cloned()
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                        });
                        FamilyState::Foreign {
                            total: unaccounted.len() as u64,
                            keys: unaccounted.into_iter().take(FOREIGN_SHOWN).collect(),
                        }
                    } else if inventory.objects.is_empty() {
                        FamilyState::Empty
                    } else {
                        let mut categories: BTreeMap<String, u64> = BTreeMap::new();
                        let mut family_bytes = 0;
                        let mut sorted = inventory.objects;
                        sorted.sort_by(|a, b| a.source_key.cmp(&b.source_key));
                        for object in sorted {
                            *categories.entry(object.category.clone()).or_default() += 1;
                            family_bytes += object.bytes;
                            inventory_lines.push_str(&format!(
                                "{}\t{}\t{}\n",
                                object.source_key, object.bytes, object.sha256
                            ));
                            objects.push(PlannedObject {
                                family: family.name.to_owned(),
                                category: object.category,
                                source_key: object.source_key,
                                target_key: object.target_key,
                                order: object.order,
                                content: object.content,
                                bytes: object.bytes,
                                sha256: object.sha256,
                                references: object.references,
                            });
                        }
                        FamilyState::Inventoried {
                            objects: categories.values().sum(),
                            bytes: family_bytes,
                            categories,
                        }
                    }
                }
            },
        };
        families.insert(
            family.name.to_owned(),
            FamilyReport {
                prefix: prefix.clone(),
                owner: family.owner.to_owned(),
                target_prefix: target_prefixes.get(family.name).cloned(),
                state,
            },
        );
    }

    for unknown in unknown_prefixes(&listing, &prefixes) {
        blockers.push(Blocker {
            kind: BlockerKind::UnknownPrefix,
            severity: Severity::RefusesCutover,
            subject: unknown.clone(),
            detail: "no registry in this build owns this prefix; it is neither inventoried nor \
                     migrated"
                .to_owned(),
        });
    }

    check_mapping(&objects)?;
    let references = summarise_references(&objects, &mut blockers);
    objects.sort_by(|a, b| (a.order, &a.target_key).cmp(&(b.order, &b.target_key)));
    blockers.sort();
    blockers.dedup();

    let listing_digest = listing_digest(&listing, &written_prefixes);
    Manifest::seal(Body {
        schema: MANIFEST_SCHEMA,
        tool: "aiwatcher-migration".to_owned(),
        target,
        snapshot: Snapshot {
            origin: source.origin.clone(),
            prefixes: FAMILIES
                .iter()
                .map(|family| (family.name.to_owned(), source.prefix_of(family)))
                .collect(),
            inventory_digest: sha256_hex(inventory_lines.as_bytes()),
            listing_digest,
            objects: objects.len() as u64,
            bytes: objects.iter().map(|object| object.bytes).sum(),
        },
        families,
        objects,
        references,
        identity: crate::manifest::IdentityPolicy::default(),
        blockers,
    })
    .map_err(MigrationError::from)
}

/// Ask one registry what it holds. The `match` is the dispatch: a family
/// declared supported with no arm here would not compile.
async fn inventory_of(
    family: &Family,
    store: &Arc<dyn ObjectStore>,
    prefix: &str,
    target: ProjectScope,
) -> std::result::Result<Inventory, InventoryError> {
    match family.name {
        "annotations" => {
            aiwatcher_annotations::Registry::new(store.clone(), prefix)
                .inventory(target)
                .await
        }
        "datasets" => {
            aiwatcher_datasets::Registry::new(store.clone(), prefix)
                .inventory(target)
                .await
        }
        "prompts" => {
            aiwatcher_prompts::Registry::new(
                store.clone(),
                aiwatcher_prompts::RegistryConfig {
                    prefix: prefix.to_owned(),
                    ..aiwatcher_prompts::RegistryConfig::default()
                },
            )
            .inventory(target)
            .await
        }
        "training" => {
            aiwatcher_training::Registry::new(store.clone(), prefix)
                .inventory(target)
                .await
        }
        other => Err(InventoryError::Refused(format!(
            "{other} is declared supported and has no adapter in this build"
        ))),
    }
}

/// Keys under this family's prefix that its own adapter neither inventoried
/// nor declared skipped.
fn unaccounted_keys<'a>(inventory: &Inventory, keys: impl Iterator<Item = &'a str>) -> Vec<String> {
    let known: BTreeSet<&str> = inventory
        .objects
        .iter()
        .map(|object| object.source_key.as_str())
        .collect();
    keys.filter(|key| {
        !known.contains(key)
            && !inventory
                .skipped
                .keys()
                .any(|skipped| key.starts_with(skipped.as_str()))
    })
    .map(ToOwned::to_owned)
    .collect()
}

/// Top-level prefixes in the store that belong to no configured family.
fn unknown_prefixes(
    listing: &[(String, u64)],
    configured: &BTreeMap<String, &'static str>,
) -> BTreeSet<String> {
    listing
        .iter()
        .filter(|(key, _)| {
            !configured
                .keys()
                .any(|prefix| key.starts_with(&format!("{prefix}/")))
        })
        .map(|(key, _)| {
            key.split_once('/')
                .map_or_else(|| key.clone(), |(head, _)| head.to_owned())
        })
        .collect()
}

/// Every key the migration does not itself write, by key and size.
///
/// The project prefixes this run would write into are left out on purpose: a
/// half-finished copy must not change the fingerprint the resume compares
/// against, or the second half could never run.
fn listing_digest(listing: &[(String, u64)], written: &[String]) -> String {
    let mut lines = String::new();
    for (key, size) in listing {
        if written.iter().any(|prefix| key.starts_with(prefix)) {
            continue;
        }
        lines.push_str(&format!("{key}\t{size}\n"));
    }
    sha256_hex(lines.as_bytes())
}

/// No two sources on one target, and no source read twice.
fn check_mapping(objects: &[PlannedObject]) -> Result<()> {
    let mut sources = BTreeSet::new();
    let mut targets: BTreeMap<&str, &str> = BTreeMap::new();
    for object in objects {
        if !sources.insert(object.source_key.as_str()) {
            return Err(MigrationError::Refused(format!(
                "{} is inventoried twice",
                object.source_key
            )));
        }
        if let Some(other) = targets.insert(&object.target_key, &object.source_key) {
            return Err(MigrationError::Refused(format!(
                "{} and {} both map onto {}",
                other, object.source_key, object.target_key
            )));
        }
    }
    Ok(())
}

/// Count what the objects point at, and name every internal reference with
/// nothing behind it.
fn summarise_references(objects: &[PlannedObject], blockers: &mut Vec<Blocker>) -> References {
    let present: BTreeSet<&str> = objects
        .iter()
        .map(|object| object.source_key.as_str())
        .collect();
    let mut summary = References {
        note: "Every reference is recorded exactly as it was stored and none is rewritten. \
               Query text and authored block specs are not analysed for dependencies."
            .to_owned(),
        ..References::default()
    };
    for object in objects {
        for (pointer, reference) in &object.references {
            match &reference.kind {
                ReferenceKind::Internal { source_key } => {
                    summary.internal += 1;
                    if !present.contains(source_key.as_str()) {
                        summary.internal_missing.push(MissingReference {
                            source_key: object.source_key.clone(),
                            pointer: pointer.clone(),
                            names: source_key.clone(),
                        });
                    }
                }
                ReferenceKind::Foreign { owner } => {
                    *summary.foreign.entry(owner.clone()).or_default() += 1;
                }
                ReferenceKind::Opaque { reason } => {
                    *summary.opaque.entry(reason.clone()).or_default() += 1;
                }
            }
        }
    }
    summary
        .internal_missing
        .sort_by(|a, b| (&a.source_key, &a.pointer).cmp(&(&b.source_key, &b.pointer)));
    for missing in &summary.internal_missing {
        blockers.push(Blocker {
            kind: BlockerKind::DanglingReference,
            severity: Severity::RefusesCutover,
            subject: missing.source_key.clone(),
            detail: format!(
                "{} names {}, which is not in this snapshot. It is copied as it stands; the \
                 source was already inconsistent and rewriting a reference would change \
                 history to make the copy look tidy",
                missing.pointer, missing.names
            ),
        });
    }
    summary
}
