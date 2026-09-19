//! Which registries this tool can move, which it will not, and why.
//!
//! The list is exhaustive over the prefixes this workspace writes into the
//! authored object store, and every entry carries an answer. That is the point:
//! a family nobody wrote an adapter for has to read as *unsupported*, never as
//! a count of zero, because zero and "nobody looked" produce the same-looking
//! migration report and only one of them means the data arrived.
//!
//! A prefix that is in the store and in no entry here is not walked past
//! either — see [`crate::plan`], which reports it as an unknown prefix.

/// What this build can do with one registry's objects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    /// An owner adapter in this build inventories it, and the executor can
    /// copy it.
    Supported,
    /// Nobody has written an adapter, or the owner's layout is still moving.
    /// Named rather than omitted.
    Unsupported(&'static str),
    /// Copying the bytes would break something that cannot be repaired
    /// afterwards. This is stronger than unsupported and needs the owner's own
    /// protocol, not an adapter.
    Blocked(&'static str),
}

/// One registry: its default prefix, who owns it, and what may be done with it.
#[derive(Clone, Copy, Debug)]
pub struct Family {
    pub name: &'static str,
    pub default_prefix: &'static str,
    pub owner: &'static str,
    pub support: Support,
}

impl Family {
    #[must_use]
    pub fn is_supported(&self) -> bool {
        matches!(self.support, Support::Supported)
    }
}

/// Every prefix this workspace writes into the authored object store.
///
/// Ordered by name so a report reads the same way twice.
pub const FAMILIES: &[Family] = &[
    Family {
        name: "annotations",
        default_prefix: "annotations",
        owner: "aiwatcher-annotations",
        support: Support::Supported,
    },
    Family {
        name: "artifacts",
        default_prefix: "artifacts",
        owner: "aiwatcher-server (execution artifacts and attempt receipts)",
        support: Support::Unsupported(
            "the scoped layout has landed (ADR_0033) and nobody has written the adapter. Step \
             outputs and attempt receipts are addressed by a canonical key the execution \
             catalog computes and validates on read, and the bytes and the manifest are \
             written by two crates, so the owner has to answer for both halves at once",
        ),
    },
    Family {
        name: "conversations",
        default_prefix: "conversations",
        owner: "aiwatcher-conversations",
        support: Support::Blocked(
            "archived content is sealed with the object's own key path as the HKDF info and \
             the AEAD associated data (ADR_0021), so the same ciphertext under a project key \
             does not open. Moving it needs a re-seal, which is a decryption, a new retention \
             clock and an erasure surface — the owner's protocol, never a byte copy",
        ),
    },
    Family {
        name: "datasets",
        default_prefix: "datasets",
        owner: "aiwatcher-datasets",
        support: Support::Supported,
    },
    Family {
        name: "evaluation-bundles",
        default_prefix: "evaluation-bundles",
        owner: "aiwatcher-server (the evaluation source adapter)",
        support: Support::Unsupported(
            "a staged bundle is what an approval was admitted over, and the approval's own \
             digest covers it — so bundle and approval move together or not at all, which is \
             the evaluation registry's own move rather than a generic copier's",
        ),
    },
    Family {
        name: "evaluation-judges",
        default_prefix: "evaluation-judges",
        owner: "aiwatcher-evaluation",
        support: Support::Unsupported(
            "kept judge replies and calibration sets are read back by declaration id. The \
             scoped layout has landed; the adapter has not",
        ),
    },
    Family {
        name: "evaluation-reviews",
        default_prefix: "evaluation-reviews",
        owner: "aiwatcher-evaluation",
        support: Support::Unsupported(
            "case reviews are project-scoped already; nobody has written the adapter that \
             says which of their objects go where",
        ),
    },
    Family {
        name: "evaluation-scopes",
        default_prefix: "evaluation-scopes",
        owner: "aiwatcher-evaluation",
        support: Support::Unsupported(
            "this prefix already holds project-scoped evaluation data; it is a destination \
             rather than a source",
        ),
    },
    Family {
        name: "evaluation-scorers",
        default_prefix: "evaluation-scorers",
        owner: "aiwatcher-evaluation",
        support: Support::Unsupported(
            "the recorded scorer catalog is what a card is pinned against and is written by \
             the work role for the whole deployment rather than by anybody's project, so it \
             is not a project's to hold a copy of",
        ),
    },
    Family {
        name: "evaluation-variant-artifacts",
        default_prefix: "evaluation-variant-artifacts",
        owner: "aiwatcher-evaluation",
        support: Support::Unsupported(
            "bytes a variant pins by digest, staged for admission. Scoped already; no adapter",
        ),
    },
    Family {
        name: "evaluations",
        default_prefix: "evaluations",
        owner: "aiwatcher-evaluation",
        support: Support::Unsupported(
            "published evidence, approvals, withdrawal markers and the retention record. \
             Approvals and withdrawals are authoritative decisions rather than a derivable \
             index, and publication races collection at one immutable key — so this family \
             needs its owner's own move, not a generic copier",
        ),
    },
    Family {
        name: "prompts",
        default_prefix: "prompts",
        owner: "aiwatcher-prompts",
        support: Support::Supported,
    },
    Family {
        name: "schedules",
        default_prefix: "schedules",
        owner: "aiwatcher-execution",
        support: Support::Unsupported(
            "a schedule is a mutable head whose tick writes slot outcomes; copying one starts \
             a second writer for the same intention. A slot has no scoped form either — a \
             bound store refuses one by name — so a project has no unattended run to copy a \
             schedule into (ADR_0033 pt. 7)",
        ),
    },
    Family {
        name: "training",
        default_prefix: "training",
        owner: "aiwatcher-training",
        support: Support::Supported,
    },
    Family {
        name: "variant-observations",
        default_prefix: "variant-observations",
        owner: "aiwatcher-projector",
        support: Support::Unsupported(
            "a projector fold's own periods, written create-only from a position in the log. \
             It is rebuilt by folding, never by copying, and a copied period would be counted \
             beside the one the fold writes",
        ),
    },
    Family {
        name: "workflows",
        default_prefix: "workflows",
        owner: "aiwatcher-execution",
        support: Support::Unsupported(
            "definition heads and revisions read by the compiler. They are project-scoped \
             already — `/workflow-definitions` answers for one project — so what is missing \
             here is the adapter and not a decision",
        ),
    },
];

/// The family a top-level path segment belongs to, if any.
#[must_use]
pub fn of_prefix(prefix: &str) -> Option<&'static Family> {
    FAMILIES
        .iter()
        .find(|family| family.default_prefix == prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_family_is_named_once_and_answers_what_may_be_done_with_it() {
        let mut names: Vec<_> = FAMILIES.iter().map(|family| family.name).collect();
        let listed = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), listed, "a family is listed twice");
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted, "the list is read in order; keep it sorted");
        for family in FAMILIES {
            match family.support {
                Support::Supported => (),
                Support::Unsupported(reason) | Support::Blocked(reason) => {
                    assert!(reason.len() > 40, "{}: say why", family.name);
                }
            }
        }
    }

    #[test]
    fn the_conversation_archive_is_blocked_rather_than_merely_unsupported() {
        let archive = of_prefix("conversations").expect("the archive is a known family");
        assert!(matches!(archive.support, Support::Blocked(_)));
        assert!(!archive.is_supported());
    }
}
