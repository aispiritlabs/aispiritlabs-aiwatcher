//! What a key under the artifact prefix looks like, in one place.
//!
//! Two callers need the same answer and live in two crates: the catalog here
//! writes the manifest, the lineage pointer and the cache entry, and
//! `aiwatcher-server`'s `Artifacts` writes the bytes those describe and the
//! receipt beside them. They shared a rule and each spelled it out, which is
//! the arrangement that ends with a manifest under one prefix and its bytes
//! under another — a catalog row nobody can open, and the one failure that
//! looks exactly like a working one.
//!
//! ```text
//! artifacts/<kind>/<aa>/<sha256>/data           the bytes            (the writer)
//! artifacts/<kind>/<aa>/<sha256>/manifest.json  what they are        (the catalog)
//! artifacts/cache/<sha256 of the key>.json      what a key resolved to
//! artifacts/lineage/<sha256 of the id>/<sha256>.json  what one run produced
//! artifacts/receipts/<sha256 of the key>.json   what one attempt produced
//!
//! artifacts/scopes/<organization>/<project>/registry/…   all of it, in a project
//! ```
//!
//! ## The scope is outside the digest
//!
//! A project prefix changes where bytes live and never what they hash to, so
//! the same table in two projects is one digest and two URIs. That is what
//! keeps a content address a content address: a reference carries the scope in
//! its key rather than in a field beside it, and a reader that resolves the key
//! is a reader that has already checked the scope.
//!
//! ## `scopes` is not a kind
//!
//! [`ArtifactKind::as_str`] answers `rows`, `code`, `preview`, `log`, `model`,
//! `report` and `blob`, and the catalog's other two families are `cache` and
//! `lineage`. None of them is `scopes`, so no global key can be spelled into
//! the scoped area and no scoped key reads as a global one. The separation is
//! the alphabet rather than a check somebody has to remember to make.

use aiwatcher_core::{ArtifactKind, ArtifactRef};
use aiwatcher_iam::{OrganizationId, ProjectId, ProjectScope};

/// The prefix every artifact family shares, global and scoped alike.
pub const PREFIX: &str = "artifacts";

/// The scheme an artifact stored through this deployment's object store is
/// named by.
///
/// Not `s3://` and not `file://`: which of those it really is depends on
/// `AIWATCHER_PROMPT_STORE`, and a URI that named the endpoint would be a plan
/// carrying a host. What this says is "resolve me through the object store this
/// deployment configured", which is the only thing a reader may do with it.
pub const SCHEME: &str = "object://";

/// The segment below [`PREFIX`] that everything belonging to a project sits
/// under.
const SCOPES: &str = "scopes";

/// The segment that closes a scope's own prefix.
///
/// Its own word rather than nothing, so a project's artifacts can gain a
/// sibling family later without every existing key becoming ambiguous.
const REGISTRY: &str = "registry";

/// Where one storage scope keeps everything it holds.
///
/// `None` is the deployment-wide namespace every artifact written before
/// projects existed lives in, and it keeps its keys unchanged.
#[must_use]
pub fn prefix(scope: Option<ProjectScope>) -> String {
    match scope {
        None => PREFIX.to_owned(),
        Some(scope) => format!(
            "{PREFIX}/{SCOPES}/{}/{}/{REGISTRY}",
            scope.organization.0, scope.project.0
        ),
    }
}

/// `<prefix>/<kind>/<first two hex>/<sha256>/data`.
///
/// The two-character shard is what keeps a bucket listing usable at a million
/// objects; the key is immutable, and a human name for the same bytes is a
/// dataset version pointing at this digest, never this key's identity.
#[must_use]
pub fn data_key(prefix: &str, kind: ArtifactKind, digest: &str) -> String {
    format!(
        "{prefix}/{}/{}/{digest}/data",
        kind.as_str(),
        digest.get(..2).unwrap_or("00")
    )
}

/// The one reference a reader in this namespace may resolve.
#[must_use]
pub fn data_uri(prefix: &str, kind: ArtifactKind, digest: &str) -> String {
    format!("{SCHEME}{}", data_key(prefix, kind, digest))
}

/// `<prefix>/<kind>/<first two hex>/<sha256>/manifest.json`, beside the bytes.
#[must_use]
pub fn manifest_key(prefix: &str, kind: ArtifactKind, digest: &str) -> String {
    format!(
        "{prefix}/{}/{}/{digest}/manifest.json",
        kind.as_str(),
        digest.get(..2).unwrap_or("00")
    )
}

/// `<prefix>/cache/<sha256 of the key>.json`.
///
/// The key hashed, because a cache key is a digest of a digest and a caller's
/// parameters, and only the first of those is guaranteed to be a path segment.
#[must_use]
pub fn cache_entry_key(prefix: &str, cache_key: &str) -> String {
    format!(
        "{prefix}/cache/{}.json",
        aiwatcher_jobs::digest(cache_key.as_bytes())
    )
}

/// One execution's own lineage prefix, trailing slash included.
///
/// The id is hashed for the reason every other key here is: it is a caller's
/// string, and a key is a path.
#[must_use]
pub fn lineage_prefix(prefix: &str, execution_id: &str) -> String {
    format!(
        "{prefix}/lineage/{}/",
        aiwatcher_jobs::digest(execution_id.as_bytes())
    )
}

/// One pointer under that prefix, named by the digest it points at.
#[must_use]
pub fn lineage_key(prefix: &str, execution_id: &str, digest: &str) -> String {
    format!("{}{digest}.json", lineage_prefix(prefix, execution_id))
}

/// Whether a string is a content address: a lowercase hex `sha256`.
///
/// [`ArtifactRef::has_digest`]'s question, asked of a bare string — the digest
/// read back out of a lineage pointer, or the one a lookup was given. Asked by
/// borrowing core's own answer rather than by writing a second one, because a
/// content address that two modules disagree about is a key one of them can
/// build and the other cannot open.
#[must_use]
pub fn is_content_address(digest: &str) -> bool {
    ArtifactRef::new("", "", digest).has_digest()
}

/// Whether this reference is exactly the pointer `prefix` would have emitted.
///
/// An exact canonical key rather than a prefix test: traversal, an encoded
/// separator, a doubled slash, a receipt, another registry and another
/// project's namespace all have to fail, and each of them survives a
/// `starts_with`. The digest, the kind and the URI are checked together, so a
/// reference whose parts disagree is refused before anything reads it —
/// knowing a URI is not access to what is under it.
#[must_use]
pub fn addresses(prefix: &str, artifact: &ArtifactRef) -> bool {
    artifact.has_digest() && artifact.uri == data_uri(prefix, artifact.kind, &artifact.digest)
}

/// The area below [`PREFIX`] that everything belonging to a project sits in.
#[must_use]
pub fn scoped_area() -> String {
    format!("{PREFIX}/{SCOPES}/")
}

/// Whether a key or a URI reaches into the area projects keep their bytes in.
///
/// The deny half, for the deployment-wide namespace: it accepts references this
/// crate did not mint — a `file://` a step handed on, an `s3://` from an older
/// build — and must still never write one down that points at a project.
///
/// A key of this deployment's own is decided by [`owner_of`]. Anything else is
/// decided by whether the scoped area is named in it at all, which is blunt and
/// deliberately so: an `s3://` or a `file://` naming the same bucket or the
/// same directory is not a pointer this deployment resolves, and the cost of
/// refusing one that merely reads like a project's is a row not written, while
/// the cost of taking one is a project's address recorded as everybody's.
#[must_use]
pub fn reaches_a_project(uri_or_key: &str) -> bool {
    let key = uri_or_key.strip_prefix(SCHEME).unwrap_or(uri_or_key);
    matches!(owner_of(key), KeyOwner::Project(_) | KeyOwner::Unattributed)
        || key.contains(&scoped_area())
}

/// Whose key this is.
///
/// A measurement's question and a refusal's, answered once. The last two
/// variants are the honest ones: a key under the scoped area naming no scope
/// and a key that is not under the artifact prefix at all are both things a
/// walk can find, and counting either as deployment-wide would be the
/// measurement telling a project's bytes as everybody's.
///
/// `ProjectScope` is neither `Ord` nor `Hash`, so this is not either: a caller
/// that wants to group by project keys on the two identifiers, which are both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyOwner {
    /// Under [`PREFIX`] and outside every project: what this deployment held
    /// before projects existed, and what an unscoped writer still writes.
    Global,
    /// One project's own.
    Project(ProjectScope),
    /// Under the scoped area and naming no resolvable project. Never written
    /// by this code; reported rather than guessed at.
    Unattributed,
    /// Not under [`PREFIX`]. A listing that answered outside the prefix it was
    /// asked about — a store's bug, and never a row to add to a total.
    Elsewhere,
}

/// Which namespace a key belongs to.
///
/// Parsed exactly: the two identifiers have to be spelled the way this code
/// spells them, so a uuid written braced or without hyphens is `Unattributed`
/// rather than quietly folded into the project whose keys it is not under.
#[must_use]
pub fn owner_of(key: &str) -> KeyOwner {
    let Some(rest) = key
        .strip_prefix(PREFIX)
        .and_then(|rest| rest.strip_prefix('/'))
    else {
        return if key == PREFIX {
            KeyOwner::Global
        } else {
            KeyOwner::Elsewhere
        };
    };
    let Some(scoped) = rest
        .strip_prefix(SCOPES)
        .and_then(|rest| rest.strip_prefix('/'))
    else {
        return KeyOwner::Global;
    };
    let parts: Vec<&str> = scoped.splitn(4, '/').collect();
    let [organization, project, registry, tail] = parts.as_slice() else {
        return KeyOwner::Unattributed;
    };
    if *registry != REGISTRY || tail.is_empty() {
        return KeyOwner::Unattributed;
    }
    match (exactly(organization), exactly(project)) {
        (Some(organization), Some(project)) => KeyOwner::Project(ProjectScope {
            organization: OrganizationId(organization),
            project: ProjectId(project),
        }),
        _ => KeyOwner::Unattributed,
    }
}

/// A uuid spelled the one way this code spells it.
fn exactly(segment: &str) -> Option<uuid::Uuid> {
    uuid::Uuid::parse_str(segment)
        .ok()
        .filter(|parsed| parsed.to_string() == segment)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> ProjectScope {
        ProjectScope {
            organization: OrganizationId::new(),
            project: ProjectId::new(),
        }
    }

    fn digest() -> String {
        "ab".repeat(32)
    }

    #[test]
    fn a_project_moves_the_prefix_and_never_the_content_address() {
        // The whole promise: the same bytes are one digest and two URIs, so a
        // reference carries its scope in the key rather than in a field
        // beside it that a reader could forget to look at.
        let scope = scope();
        let global = data_uri(&prefix(None), ArtifactKind::Rows, &digest());
        let scoped = data_uri(&prefix(Some(scope)), ArtifactKind::Rows, &digest());
        assert_ne!(global, scoped);
        assert!(global.contains(&digest()) && scoped.contains(&digest()));
        assert_eq!(
            prefix(Some(scope)),
            format!(
                "artifacts/scopes/{}/{}/registry",
                scope.organization.0, scope.project.0
            )
        );
    }

    #[test]
    fn every_kind_and_family_stays_out_of_the_scoped_area() {
        // `scopes` is not a kind, so the separation is the alphabet rather
        // than a check somebody has to remember to make.
        for kind in [
            ArtifactKind::Rows,
            ArtifactKind::Code,
            ArtifactKind::Preview,
            ArtifactKind::Log,
            ArtifactKind::Model,
            ArtifactKind::Report,
            ArtifactKind::Blob,
        ] {
            assert_ne!(kind.as_str(), SCOPES);
            assert_eq!(
                owner_of(&data_key(&prefix(None), kind, &digest())),
                KeyOwner::Global
            );
        }
        for family in ["cache", "lineage", "receipts"] {
            assert_ne!(family, SCOPES);
        }
    }

    #[test]
    fn a_key_says_whose_it_is_and_an_unspellable_one_is_not_everybodys() {
        let scope = scope();
        assert_eq!(
            owner_of(&data_key(
                &prefix(Some(scope)),
                ArtifactKind::Log,
                &digest()
            )),
            KeyOwner::Project(scope)
        );
        assert_eq!(owner_of("artifacts/cache/aa.json"), KeyOwner::Global);
        assert_eq!(owner_of("prompts/heads/aa.json"), KeyOwner::Elsewhere);
        assert_eq!(owner_of("artifactsx/rows/aa/x/data"), KeyOwner::Elsewhere);
        for unattributed in [
            "artifacts/scopes/".to_owned(),
            "artifacts/scopes/not-a-uuid/also-not/registry/rows".to_owned(),
            format!(
                "artifacts/scopes/{}/{}/store/rows",
                scope.organization.0, scope.project.0
            ),
            // Spelled another way: the same identifier, a key nothing here
            // would write, and a project it must not be counted into.
            format!(
                "artifacts/scopes/{}/{}/registry/rows",
                scope.organization.0.simple(),
                scope.project.0
            ),
            format!(
                "artifacts/scopes/{}/{}/registry/",
                scope.organization.0, scope.project.0
            ),
        ] {
            assert_eq!(
                owner_of(&unattributed),
                KeyOwner::Unattributed,
                "{unattributed}"
            );
        }
    }

    #[test]
    fn only_the_exact_pointer_a_prefix_emits_addresses_it() {
        let scope = scope();
        let mine = prefix(Some(scope));
        let canonical = ArtifactRef::new(
            "rows",
            data_uri(&mine, ArtifactKind::Rows, &digest()),
            digest(),
        )
        .of_kind(ArtifactKind::Rows);
        assert!(addresses(&mine, &canonical));
        assert!(!addresses(&prefix(None), &canonical));
        assert!(reaches_a_project(&canonical.uri));
        // And in another scheme, which this deployment resolves for nobody and
        // must still not write down as one of its own artifacts' addresses.
        assert!(reaches_a_project(&format!(
            "s3://a-bucket/{}",
            data_key(&mine, ArtifactKind::Rows, &digest())
        )));
        assert!(!reaches_a_project(&data_uri(
            &prefix(None),
            ArtifactKind::Rows,
            &digest()
        )));
        assert!(!reaches_a_project("s3://a-bucket/rows.json"));

        for forged in [
            ArtifactRef {
                uri: canonical.uri.replace("/rows/", "/rows/../rows/"),
                ..canonical.clone()
            },
            ArtifactRef {
                uri: canonical.uri.replace("/rows/", "/rows//"),
                ..canonical.clone()
            },
            ArtifactRef {
                uri: canonical.uri.replace("/rows/", "/rows/%2e%2e/rows/"),
                ..canonical.clone()
            },
            ArtifactRef {
                uri: format!("{}?project=other", canonical.uri),
                ..canonical.clone()
            },
            ArtifactRef {
                kind: ArtifactKind::Log,
                ..canonical.clone()
            },
            ArtifactRef {
                digest: "ab".to_owned(),
                ..canonical.clone()
            },
            ArtifactRef {
                digest: "AB".repeat(32),
                ..canonical.clone()
            },
            ArtifactRef {
                uri: "file:///tmp/rows.json".to_owned(),
                ..canonical.clone()
            },
        ] {
            assert!(!addresses(&mine, &forged), "{}", forged.uri);
        }
    }

    #[test]
    fn a_content_address_is_core_s_own_answer_rather_than_a_second_one() {
        assert!(is_content_address(&digest()));
        for not in ["", "ab", &"AB".repeat(32), &"zz".repeat(32), "../secret"] {
            assert!(!is_content_address(not), "{not}");
        }
    }
}
