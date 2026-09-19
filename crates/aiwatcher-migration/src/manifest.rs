//! The manifest, what it is identified by, and what a dry run observed.
//!
//! Two documents rather than one, and the split is the whole reason resume
//! works. A **manifest** describes the source and the mapping: the snapshot it
//! was taken from, the families it covers, every object's key pair and digest,
//! and every reason a cutover cannot yet be declared. It is a pure function of
//! the snapshot and the configuration, so re-planning an unchanged snapshot
//! yields the same bytes and the same [`Manifest::manifest_id`].
//!
//! A **survey** is what the target looked like at one moment: absent,
//! identical, in conflict. That is deliberately *not* part of the manifest,
//! because a half-finished copy changes it — and a manifest whose identity
//! moved when the first object landed would be a manifest no resume could bind
//! to.

use std::collections::BTreeMap;

use aiwatcher_core::migration::{ContentKind, Reference, WriteOrder, sha256_hex};
use aiwatcher_iam::{ProjectRole, ProjectScope};
use serde::{Deserialize, Serialize};

/// Bumped when a field changes meaning. A tool reading an older manifest
/// refuses it rather than guessing what a missing field meant.
pub const MANIFEST_SCHEMA: u32 = 1;
pub const CHECKPOINT_SCHEMA: u32 = 1;

/// What a migration cannot do yet, and how badly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Nothing is written while this stands.
    RefusesExecution,
    /// The copy may run and is faithful; the *cutover* is not complete, so no
    /// route may be switched and no source may be removed on the strength of
    /// this run.
    RefusesCutover,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockerKind {
    /// A registry in the store that this build has no adapter for.
    UnsupportedFamily,
    /// A registry whose bytes must not be copied at all.
    BlockedFamily,
    /// A key under a known family's prefix that its own adapter did not
    /// recognise: a schema this tool must not walk past.
    ForeignObject,
    /// An object at a key the owner does recognise, holding something that is
    /// not that owner's document.
    DamagedObject,
    /// A top-level prefix in the store that belongs to no known family.
    UnknownPrefix,
    /// An object names another object in the same registry that is not there.
    /// Copied as it stands — the source was already inconsistent, and
    /// rewriting a reference to make the copy look tidy is changing history.
    DanglingReference,
    /// The target project was admitted by something that is not the
    /// deployment's authoritative IAM.
    NonAuthoritativeIam,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Blocker {
    pub kind: BlockerKind,
    pub severity: Severity,
    /// The family, prefix or key this is about.
    pub subject: String,
    pub detail: String,
}

/// Where the bytes were read from, and what they were when they were read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// The operator's own words for the store this was planned against. Part
    /// of the identity, so a manifest cannot be resumed against a store it was
    /// never planned for without saying so.
    pub origin: String,
    /// Family to the prefix this run used, which is configuration rather than
    /// a constant: the prompt registry's prefix is a deployment setting.
    pub prefixes: BTreeMap<String, String>,
    /// SHA-256 over `source_key \t bytes \t sha256` of every inventoried
    /// object, in key order. Content, never size and modification time: two
    /// edits of one object are the same length more often than anybody plans
    /// for.
    pub inventory_digest: String,
    /// SHA-256 over `key \t size` of every key under every known prefix that
    /// the migration does not itself write, plus every key under an unknown
    /// prefix. It is what notices an object appearing or disappearing in a
    /// family this run does not read.
    pub listing_digest: String,
    pub objects: u64,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum FamilyState {
    /// Read, hashed and mapped. `categories` is the owner's own vocabulary.
    Inventoried {
        objects: u64,
        bytes: u64,
        categories: BTreeMap<String, u64>,
    },
    /// Supported, and there is nothing under its prefix. A real answer, and
    /// not the same answer as the next two.
    Empty,
    /// No adapter in this build. Not a count, and not a success.
    Unsupported {
        reason: String,
        objects: u64,
        bytes: u64,
    },
    /// Must not be copied. The reason says what would break.
    Blocked {
        reason: String,
        objects: u64,
        bytes: u64,
    },
    /// An object under this prefix is not the registry's own document.
    Damaged { key: String, message: String },
    /// Keys under this prefix its own adapter did not recognise. Neither
    /// migrated nor ignored.
    Foreign { keys: Vec<String>, total: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FamilyReport {
    pub prefix: String,
    pub owner: String,
    /// Where this registry's objects go in the bound scope, as the registry
    /// itself computed it. The executor checks every target key against this
    /// rather than against a prefix it assembled, so an edited manifest cannot
    /// name a key outside the project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_prefix: Option<String>,
    #[serde(flatten)]
    pub state: FamilyState,
}

/// One object, and where it goes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedObject {
    pub family: String,
    pub category: String,
    pub source_key: String,
    pub target_key: String,
    pub order: WriteOrder,
    pub content: ContentKind,
    pub bytes: u64,
    pub sha256: String,
    pub references: BTreeMap<String, Reference>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissingReference {
    pub source_key: String,
    pub pointer: String,
    pub names: String,
}

/// What the objects point at, counted rather than resolved.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct References {
    pub internal: u64,
    /// Named, every one. An internal reference with nothing behind it is a
    /// source that was already inconsistent.
    pub internal_missing: Vec<MissingReference>,
    /// Owner to how many references name it.
    pub foreign: BTreeMap<String, u64>,
    /// Reason to how many references carry it.
    pub opaque: BTreeMap<String, u64>,
    pub note: String,
}

/// What this tool does and does not do to identity, written into every
/// manifest so nobody has to take it on trust from a runbook.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityPolicy {
    /// Always false. Mapping data into a project is not granting anybody
    /// access to it.
    pub creates_grants: bool,
    /// Always false. An identity provider's groups are not this system's
    /// teams, and a name is not a membership.
    pub reads_identity_provider_groups: bool,
    /// Always false. Content, version ids and references are copied byte for
    /// byte.
    pub rewrites_content: bool,
    pub note: String,
}

impl Default for IdentityPolicy {
    fn default() -> Self {
        Self {
            creates_grants: false,
            reads_identity_provider_groups: false,
            rewrites_content: false,
            note: "This manifest maps storage keys into one named project. It creates no \
                   organization, team, membership or grant, reads no identity-provider group, \
                   and rewrites no stored byte, version id or reference. Who may read the \
                   target project after the copy is exactly who could before it."
                .to_owned(),
        }
    }
}

/// Everything the manifest's identity covers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Body {
    pub schema: u32,
    pub tool: String,
    pub target: ProjectScope,
    pub snapshot: Snapshot,
    /// Every known family, with an answer for each.
    pub families: BTreeMap<String, FamilyReport>,
    /// Sorted by write order, then by target key. This is the copy order.
    pub objects: Vec<PlannedObject>,
    pub references: References,
    pub identity: IdentityPolicy,
    /// Sorted. Every reason this is not a finished cutover.
    pub blockers: Vec<Blocker>,
}

/// A reviewed plan, named by what it says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// SHA-256 of `body`'s canonical JSON. What a checkpoint binds to, and
    /// what an execution re-derives from the live snapshot before it writes.
    pub manifest_id: String,
    pub body: Body,
}

impl Manifest {
    /// Seal a body with the digest of its own bytes.
    ///
    /// # Errors
    ///
    /// Only if the body cannot be serialised, which means a type in it grew a
    /// map with non-string keys.
    pub fn seal(body: Body) -> Result<Self, serde_json::Error> {
        let manifest_id = sha256_hex(&serde_json::to_vec(&body)?);
        Ok(Self { manifest_id, body })
    }

    /// Whether the id still describes the body.
    ///
    /// # Errors
    ///
    /// As [`Manifest::seal`].
    pub fn identity_holds(&self) -> Result<bool, serde_json::Error> {
        Ok(sha256_hex(&serde_json::to_vec(&self.body)?) == self.manifest_id)
    }

    #[must_use]
    pub fn refuses_execution(&self) -> Vec<&Blocker> {
        self.body
            .blockers
            .iter()
            .filter(|blocker| blocker.severity == Severity::RefusesExecution)
            .collect()
    }

    /// Whether every family in the store was inventoried, so a completed copy
    /// would be the whole application's data rather than part of it.
    #[must_use]
    pub fn covers_every_family(&self) -> bool {
        self.body.blockers.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetState {
    Absent,
    Identical,
    Conflict,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    pub source_key: String,
    pub target_key: String,
    pub source_sha256: String,
    pub target_sha256: String,
}

/// Who admitted the target project, and whether they are allowed to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "checked", rename_all = "snake_case")]
pub enum IamCheck {
    /// The deployment's IAM answered for this exact scope.
    Verified {
        /// The store that answered: `postgres`, or a fixture.
        authority: String,
        /// Whether that store is one a deployment actually authorizes from.
        authoritative: bool,
        provider: String,
        subject: String,
        role: ProjectRole,
        evaluated_at: i64,
    },
    /// Offline. A dry run may say this; an execution may not.
    NotChecked { reason: String },
}

/// Who will reach what this run copied.
///
/// **A copy is not a share.** This tool creates no organization, team,
/// membership or grant, and that does not change; what it does is *report*.
/// Mapping authored data into a project makes it reachable by whoever holds a
/// live grant on that project and by nobody else, so a cutover declared
/// without knowing who that is was declared without reading its consequence.
///
/// The precedent is `GET /api/v1/system`: a deployment's own facts are worth
/// having in one place, and a grant is not a secret from the operator who
/// already holds admin on the project.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Audience {
    /// The live grants on the target, as IAM answered them.
    Read {
        /// Principals and teams with a live grant at the moment this was read,
        /// each with the role the window leaves them. Sorted, so two runs of
        /// one plan read the same way.
        holders: Vec<Holder>,
        /// When IAM was asked. Not a capability and not durable: a grant
        /// revoked a minute later is not reflected here, which is why the
        /// receipt says *when*.
        evaluated_at: i64,
    },
    /// Nobody was asked. A dry run with no IAM says this, and so does a run
    /// whose authority is a fixture, which is not an authority.
    NotRead { reason: String },
}

/// One live grant on the target project, as it is reported.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Holder {
    /// `oidc:subject` for a person, `team:<id>` for a team. The exact pair the
    /// control plane compares, never an email address and never a group name.
    pub grantee: String,
    pub role: ProjectRole,
}

/// What the target held when somebody looked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Survey {
    pub manifest_id: String,
    pub target: ProjectScope,
    pub iam: IamCheck,
    /// Who holds a live grant on the target, read when this survey ran — so
    /// the question "and who will be able to open this afterwards" has an
    /// answer **before** anything is copied. See [`Audience`].
    pub audience: Audience,
    pub absent: u64,
    pub identical: u64,
    /// Named, every one. Nothing is overwritten.
    pub conflicts: Vec<Conflict>,
    pub blockers: Vec<Blocker>,
}

/// How an object is published into the target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Publication {
    /// The store publishes a complete object only when the key is absent, so
    /// a second writer racing this one loses rather than interleaving.
    CreateOnly,
    /// The store has no such guarantee, and the operator stated that this run
    /// has the store to itself. Read-then-write is only safe under that
    /// statement, and the receipt keeps it so nobody reads this run as proof
    /// of something it never had.
    ExclusiveAccess,
}

/// What one execution did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub manifest_id: String,
    pub target: ProjectScope,
    pub iam: IamCheck,
    /// Who holds a live grant on the target, read when this run finished.
    /// See [`Audience`]: reported, never created.
    pub audience: Audience,
    pub publication: Publication,
    /// Objects this run copied.
    pub written: u64,
    /// Objects already byte-identical in the target when this run reached
    /// them. A safe repetition, not a conflict.
    pub already_identical: u64,
    /// Objects a checkpoint said an earlier run had finished.
    pub resumed: u64,
    pub bytes_written: u64,
    /// Whether every object in the manifest is now in the target.
    pub complete: bool,
    /// Whether the whole application's authored data is now in the project.
    /// False whenever any blocker stands, including every family this build
    /// does not support.
    pub cutover_ready: bool,
    pub blockers: Vec<Blocker>,
}

/// Durable progress, bound to one manifest, one target and one store.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub schema: u32,
    pub manifest_id: String,
    pub target: ProjectScope,
    /// The operator's words for the destination store, as the manifest's
    /// snapshot records them for the source.
    pub destination: String,
    /// Target keys written *and read back*. A lagging checkpoint costs a
    /// repeated copy of identical bytes; one that ran ahead would lose an
    /// object, so it is written only after the read-back.
    pub done: Vec<String>,
    pub updated_at: i64,
}

impl Checkpoint {
    #[must_use]
    pub fn new(manifest: &Manifest, destination: &str) -> Self {
        Self {
            schema: CHECKPOINT_SCHEMA,
            manifest_id: manifest.manifest_id.clone(),
            target: manifest.body.target,
            destination: destination.to_owned(),
            done: Vec::new(),
            updated_at: 0,
        }
    }

    /// Whether this progress belongs to this exact plan against this exact
    /// destination.
    #[must_use]
    pub fn binds_to(&self, manifest: &Manifest, destination: &str) -> bool {
        self.schema == CHECKPOINT_SCHEMA
            && self.manifest_id == manifest.manifest_id
            && self.target == manifest.body.target
            && self.destination == destination
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> Body {
        Body {
            schema: MANIFEST_SCHEMA,
            tool: "aiwatcher-migration".to_owned(),
            target: ProjectScope {
                organization: aiwatcher_iam::OrganizationId::new(),
                project: aiwatcher_iam::ProjectId::new(),
            },
            snapshot: Snapshot {
                origin: "fs:/snapshot".to_owned(),
                prefixes: BTreeMap::from([("prompts".to_owned(), "prompts".to_owned())]),
                inventory_digest: "a".repeat(64),
                listing_digest: "b".repeat(64),
                objects: 0,
                bytes: 0,
            },
            families: BTreeMap::new(),
            objects: Vec::new(),
            references: References::default(),
            identity: IdentityPolicy::default(),
            blockers: Vec::new(),
        }
    }

    #[test]
    fn a_manifest_is_named_by_its_own_body_and_notices_an_edited_one() {
        let manifest = Manifest::seal(body()).expect("a body serialises");
        assert!(manifest.identity_holds().expect("re-serialises"));
        assert_eq!(
            manifest.manifest_id,
            Manifest::seal(manifest.body.clone())
                .expect("re-seals")
                .manifest_id,
            "sealing the same body twice is the same id"
        );
        let mut edited = manifest.clone();
        edited.body.snapshot.origin = "fs:/somewhere-else".to_owned();
        assert!(
            !edited.identity_holds().expect("re-serialises"),
            "an edited body must not keep the id it was reviewed under"
        );
    }

    #[test]
    fn a_checkpoint_binds_to_one_plan_target_and_destination() {
        let manifest = Manifest::seal(body()).expect("a body serialises");
        let checkpoint = Checkpoint::new(&manifest, "fs:/target");
        assert!(checkpoint.binds_to(&manifest, "fs:/target"));
        assert!(!checkpoint.binds_to(&manifest, "fs:/another-store"));
        let mut other = manifest.clone();
        other.manifest_id = "c".repeat(64);
        assert!(!checkpoint.binds_to(&other, "fs:/target"));
        let mut scope = manifest;
        scope.body.target.project = aiwatcher_iam::ProjectId::new();
        let resealed = Manifest::seal(scope.body).expect("re-seals");
        assert!(!checkpoint.binds_to(&resealed, "fs:/target"));
    }
}
