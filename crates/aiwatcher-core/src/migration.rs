//! What a registry says about its own stored objects, so a migration tool can
//! sequence them without inventing a second set of key rules.
//!
//! Every registry here owns its key layout — the prompt registry's
//! `{name}/versions/{id}.json`, the annotation registry's hashed project
//! segments, the training registry's digested run ids. A copier that re-derived
//! them would be a second implementation of each, free to drift, and the day it
//! drifted it would write bytes under a key nothing reads.
//!
//! So the owner answers, in this vocabulary: which objects it holds, which key
//! each of them takes in a bound scope, in what order they may be written, what
//! is JSON and what is opaque bytes, and what each object points at. The tool
//! plans, verifies, sequences and copies. It computes no key.
//!
//! Three rules carry the vocabulary:
//!
//! * **The owner names both keys.** `target_key` comes from the owner's own
//!   scope binding, never from the tool's string arithmetic.
//! * **Every key under the source prefix is accounted for.** An object this
//!   adapter does not recognise is absent from both [`Inventory::objects`] and
//!   [`Inventory::skipped`], and the tool reads that as a schema it must refuse.
//! * **A reference is inventoried, never rewritten.** [`ReferenceKind`] says
//!   whether the owner resolves it here, whether another registry owns it, or
//!   whether it is text this adapter does not interpret. Rewriting one would be
//!   changing history to make a copy look consistent.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ports::PortError;

/// Where an object sits in its registry's own write ordering.
///
/// Every authored registry in this workspace keeps the same two orderings —
/// the version object before the head that indexes it, the export manifest
/// before the index entry that lists it — because an index naming an object
/// that was never stored is a list whose rows 404, while an unindexed object
/// is merely waiting to be found again. A copy has to keep that ordering or a
/// crash halfway through leaves the target in the state the rule exists to
/// prevent.
///
/// Sorting is by rank, and objects of one rank are unordered with respect to
/// each other. An owner that needs a finer ordering than three ranks has a
/// dependency this vocabulary cannot express, and should say so rather than
/// approximate it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteOrder {
    /// Bytes something else points at: a content-addressed blob, an immutable
    /// version, a staged shard. Written first.
    Content,
    /// A record that names content: an image head, an export manifest, a run
    /// record. Written once its content exists.
    Record,
    /// A mutable index or head that names records. Written last, so a partial
    /// copy is an unindexed object rather than a row that 404s.
    Index,
}

/// Whether the owner stores this object as JSON of a shape it can parse.
///
/// `Opaque` is not "the owner did not look": it is the owner saying the bytes
/// are not its own document — an image, a `.jsonl` shard of somebody else's
/// rows — so a reader must not report a parse failure as a damaged registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Json,
    Opaque,
}

/// What one field of a stored object points at.
///
/// Three answers rather than two, because "this adapter cannot resolve it" and
/// "another registry owns it" lead to different operator decisions: the second
/// names the migration that has to happen as well, and the first names a value
/// nobody can check from here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReferenceKind {
    /// Another object in this same inventory, by its source key. The tool
    /// checks that it is there; a missing one is a dangling reference in the
    /// source, reported and copied as it stands.
    Internal { source_key: String },
    /// A resource another registry owns, named by that registry's crate. Only
    /// that registry's own adapter could resolve it, so this one does not try.
    Foreign { owner: String },
    /// Text or a runtime identifier this adapter does not interpret: a
    /// checkpoint URI, an execution id, a query's own vocabulary.
    Opaque { reason: String },
}

/// One reference, as it was stored and as the owner reads it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    /// Exactly the value in the stored object. Never normalised.
    pub value: String,
    #[serde(flatten)]
    pub kind: ReferenceKind,
}

/// One stored object, as its owner describes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryObject {
    /// The owner's own word for what this object is — `versions`, `heads`,
    /// `blobs`. Counted and reported; never parsed.
    pub category: String,
    pub source_key: String,
    /// Where this object goes in the bound scope, computed by the owner.
    pub target_key: String,
    pub order: WriteOrder,
    pub content: ContentKind,
    pub bytes: u64,
    /// SHA-256 of the original bytes, hex. What a copy is verified against.
    pub sha256: String,
    /// JSON-pointer to reference, sorted.
    pub references: BTreeMap<String, Reference>,
}

/// Everything one registry holds under one prefix, and where it would go.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inventory {
    pub source_prefix: String,
    pub target_prefix: String,
    /// In no particular order. The tool sorts.
    pub objects: Vec<InventoryObject>,
    /// Sub-prefixes under `source_prefix` this inventory deliberately leaves
    /// behind, each with the reason — project-scoped keys being the one every
    /// adapter has. A key under neither an object nor one of these is a schema
    /// this adapter does not recognise, and the tool refuses rather than
    /// guessing which of the two it meant.
    pub skipped: BTreeMap<String, String>,
}

/// Why an inventory could not be taken.
#[derive(Debug, thiserror::Error)]
pub enum InventoryError {
    /// The bytes under this key are not this registry's own document. A
    /// migration stops here: copying a document nobody can read into a project
    /// makes it that project's problem, and skipping it loses it silently.
    #[error("{key} does not hold this registry's own schema: {message}")]
    Damaged { key: String, message: String },
    /// The scope could not be bound, or the configured prefix is unsafe.
    #[error("{0}")]
    Refused(String),
    #[error(transparent)]
    Store(#[from] PortError),
}

/// SHA-256 of some bytes, hex.
///
/// One definition, because a manifest, an owner's inventory and the executor's
/// read-back all have to agree on what "the same bytes" means, and three
/// spellings of one hash is three chances to disagree.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

/// Segments no key may contain, wherever it came from.
///
/// Applied by the owner when it builds a key and again by the tool before it
/// writes one, because the second caller has a manifest somebody could have
/// edited between the two.
#[must_use]
pub fn key_is_safe(key: &str) -> bool {
    !key.is_empty()
        && !key.starts_with('/')
        && !key.contains('\\')
        && !key.chars().any(char::is_control)
        && !key
            .split('/')
            .any(|segment| matches!(segment, "" | "." | ".."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_with_a_traversal_or_an_empty_segment_is_not_safe() {
        for key in [
            "",
            "/absolute",
            "prompts//head.json",
            "prompts/../secret",
            "prompts/./head.json",
            "prompts/x\\secret",
            "prompts/head\u{0}.json",
        ] {
            assert!(!key_is_safe(key), "{key:?} must be refused");
        }
        for key in [
            "prompts/a.b/head.json",
            "prompts/scopes/0198/0199/registry/a/versions/ff.json",
            "annotations/blobs/abcdef",
        ] {
            assert!(key_is_safe(key), "{key:?} must be admitted");
        }
    }

    #[test]
    fn write_order_sorts_content_before_records_before_indexes() {
        let mut ranks = vec![WriteOrder::Index, WriteOrder::Content, WriteOrder::Record];
        ranks.sort();
        assert_eq!(
            ranks,
            [WriteOrder::Content, WriteOrder::Record, WriteOrder::Index]
        );
    }
}
