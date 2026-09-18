//! Which project an execution belongs to, who started it, and the one rule
//! every workflow store applies about both.
//!
//! An execution used to be one thing: a stream, a projection, a claim table row
//! and an outbox row, all reachable by id from whichever process held the
//! store. That is correct while there is one tenant and wrong the moment there
//! are two — a reactor polling for work, a launcher reading claimable rows, a
//! timer tick and the outbox publisher would each pick a project's execution up
//! and carry it out under no authority at all.
//!
//! So an execution now has an **owner**, and the owner is durable:
//!
//! * [`ExecutionOwnership`] is written **in the transaction that creates the
//!   execution** and never again ([`WorkflowStore::append`]'s own rule). Not an
//!   object beside the run, not a row written before the start and not one
//!   written after it: either the run and its owner both exist or neither does.
//! * It is **immutable**. A later command, a replay, a retry and a repeated
//!   start all assert it and none of them may repoint it. A start naming a
//!   different owner is [`StoreError::OwnershipConflict`], not a second owner.
//! * It does not come from the plan, its parameters, `requested_by`, the
//!   declaration's author, a worker's name or anything a claimant says about
//!   itself. It comes from [`ProjectStart`], which only trusted server wiring
//!   constructs from an authenticated principal and a resolved scope.
//!
//! A **global** execution has no record at all. That is deliberate: every
//! stream this build has ever written is one, and inventing an owner for them
//! would be a migration this stage explicitly does not perform. Absence *is*
//! the unscoped side, and [`ScopeBinding`] reads it that way.
//!
//! The binding goes on the **store**, not on each call, for the reason
//! `Artifacts::for_project` binds a byte store and `DefinitionRegistry::for_project`
//! binds a registry: one door, checked once, and the unscoped reader enforcing
//! the same rule from the other side. A store bound to a project sees that
//! project's executions and nothing else; the unscoped store sees the ones
//! nobody owns and refuses the rest, so the reactor, the worker, the launcher,
//! the timer tick, the outbox publisher and the retention sweep this binary
//! already runs cannot reach a project's work by accident.
//!
//! What this is **not**: an authorization decision. Nothing here asks IAM
//! anything. It says which executions a store may touch at all, which is the
//! floor a dispatcher stands on when it later asks whether this principal still
//! holds a grant.

use aiwatcher_iam::{Principal, ProjectScope};
use serde::{Deserialize, Serialize};

use crate::error::{Result, StoreError};
use crate::plan::{DefinitionKind, DefinitionRevision, ExecutionPlan, PlanId};
use crate::state::ExecutionId;

/// Which side of the project boundary something is on.
///
/// Two arms and no default: a caller has to *choose*, the way
/// [`ExpectedVersion`](crate::store::ExpectedVersion) makes one choose an
/// expectation rather than pass a number it guessed. A store that defaulted to
/// [`Self::Global`] would be a store somebody forgot to bind, which is the one
/// mistake this type exists to make impossible to write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionScope {
    /// Every execution this build has started so far, and every one a
    /// deployment with no IAM starts. Nothing owns them.
    Global,
    /// One project of one organization.
    Project(ProjectScope),
}

impl ExecutionScope {
    /// The project, when there is one.
    #[must_use]
    pub const fn project(self) -> Option<ProjectScope> {
        match self {
            Self::Global => None,
            Self::Project(scope) => Some(scope),
        }
    }

    /// Whether this is the unscoped side.
    #[must_use]
    pub const fn is_global(self) -> bool {
        matches!(self, Self::Global)
    }

    /// What a message, a log line and a SQL column call this scope.
    ///
    /// The empty string for [`Self::Global`], so the column an adapter stores
    /// it in has no nullable case to get wrong, and `<organization>/<project>`
    /// otherwise. Two uuids, which is what makes it safe to build by
    /// concatenation: neither half can contain the separator.
    #[must_use]
    pub fn key(self) -> String {
        match self {
            Self::Global => String::new(),
            Self::Project(scope) => format!("{}/{}", scope.organization.0, scope.project.0),
        }
    }

    /// The sentence a refusal names this scope with.
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Global => "the unscoped path".to_owned(),
            Self::Project(scope) => format!(
                "project {} of organization {}",
                scope.project.0, scope.organization.0
            ),
        }
    }
}

impl std::fmt::Display for ExecutionScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label())
    }
}

/// Which project a run belongs to, and who asked for it.
///
/// The trusted half of a start, and the only way ownership is ever created.
/// Server wiring builds it from a verified session and a resolved scope; it is
/// not a request body, not a plan field, not a worker token and not a
/// declaration's author. Nothing deserializes it, deliberately: a type that
/// could arrive over the wire is a type somebody can send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectStart {
    pub scope: ProjectScope,
    pub principal: Principal,
}

impl ProjectStart {
    #[must_use]
    pub const fn new(scope: ProjectScope, principal: Principal) -> Self {
        Self { scope, principal }
    }
}

/// What an execution was started on: the compiled plan, and the definition
/// behind it.
///
/// Kept beside the owner rather than read from the stream, because the whole
/// point of the record is to be readable *before* the stream is: a dispatcher
/// deciding whether it may load an execution at all cannot first load it.
/// Content addresses on both halves, so this names one immutable thing.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct OwnedDefinition {
    pub kind: DefinitionKind,
    pub name: String,
    pub revision: DefinitionRevision,
    pub plan_id: PlanId,
}

impl OwnedDefinition {
    #[must_use]
    pub fn of(plan: &ExecutionPlan) -> Self {
        Self {
            kind: plan.definition_kind,
            name: plan.definition_name.clone(),
            revision: plan.revision.clone(),
            plan_id: plan.plan_id.clone(),
        }
    }
}

/// The durable record: who owns one execution, and what they started.
///
/// Written once, with the execution, in one transaction. Read by a dispatcher
/// before it touches anything else about the run, which is why it carries the
/// plan binding as well as the principal: "may this process run this" and "what
/// was it asked to run" are one lookup rather than a lookup and a stream read.
///
/// It is not a capability. It says who the owner *is*; whether that principal
/// still holds a grant is IAM's answer, asked fresh every time.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ExecutionOwnership {
    pub scope: ProjectScope,
    pub principal: Principal,
    pub definition: OwnedDefinition,
}

impl ExecutionOwnership {
    /// The record a start establishes.
    #[must_use]
    pub fn of(start: &ProjectStart, plan: &ExecutionPlan) -> Self {
        Self {
            scope: start.scope,
            principal: start.principal.clone(),
            definition: OwnedDefinition::of(plan),
        }
    }

    /// The side of the boundary this record puts its execution on.
    #[must_use]
    pub const fn execution_scope(&self) -> ExecutionScope {
        ExecutionScope::Project(self.scope)
    }

    /// What a refusal names this owner by.
    ///
    /// The scope and the principal's provider namespace, never the subject: a
    /// refusal is read by whoever asked, and whoever asked is on the other side
    /// of the boundary the refusal is about.
    #[must_use]
    pub fn label(&self) -> String {
        self.execution_scope().label()
    }
}

/// What one store is bound to, and every rule it applies about scope.
///
/// Written once here rather than four times, for the reason
/// [`prunable`](crate::store::prunable) and [`settles`](crate::store::settles)
/// are: four adapters answering "is this execution mine" four ways would be
/// four boundaries, and the one that disagreed would be the one that handed a
/// project's attempt to a global reactor. The adapters differ in *where they
/// read the record from* — a map, a file, an indexed table — and in nothing
/// else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopeBinding {
    scope: ExecutionScope,
}

impl Default for ScopeBinding {
    fn default() -> Self {
        Self::global()
    }
}

impl ScopeBinding {
    /// The unscoped store: the executions nobody owns, and no others.
    #[must_use]
    pub const fn global() -> Self {
        Self {
            scope: ExecutionScope::Global,
        }
    }

    #[must_use]
    pub const fn scope(self) -> ExecutionScope {
        self.scope
    }

    /// Bind to one project.
    ///
    /// Binding a store that is already bound to the same project is the same
    /// store, so wiring that resolves a scope twice costs nothing. Binding it
    /// to a *second* project is refused rather than replacing the first —
    /// `Artifacts::for_project`'s rule, and for the sharper reason here: a
    /// rebinding that silently won would move every live claim, timer and
    /// outbox row of one project under another's name.
    ///
    /// # Errors
    ///
    /// [`StoreError::NotInThisScope`] naming both scopes.
    pub fn bind(self, scope: ProjectScope) -> Result<Self> {
        match self.scope {
            ExecutionScope::Global => Ok(Self {
                scope: ExecutionScope::Project(scope),
            }),
            ExecutionScope::Project(held) if held == scope => Ok(self),
            ExecutionScope::Project(held) => Err(StoreError::NotInThisScope {
                what: format!(
                    "bind to {}: it is already bound to {}",
                    ExecutionScope::Project(scope).label(),
                    ExecutionScope::Project(held).label()
                ),
            }),
        }
    }

    /// Whether an execution with this recorded ownership is this store's.
    ///
    /// `exists` is whether the store holds any history for that id, and it is
    /// the difference between the two things "no owner" can mean. An id nobody
    /// has used is nobody's *yet*: a project store may look at it, find
    /// nothing, and start a run under it. An execution that **exists** with no
    /// owner is a global run — every stream written before this stage, and
    /// every one a deployment with no IAM writes — and a project store adopting
    /// one is exactly the repointing this record prevents, so that is refused
    /// however empty the project's own side is.
    ///
    /// The unscoped store needs no such distinction: an id nobody has used is
    /// unowned, and so is every stream written before ownership existed.
    #[must_use]
    pub fn admits(self, recorded: Option<&ExecutionOwnership>, exists: bool) -> bool {
        match (self.scope, recorded) {
            (ExecutionScope::Global, None) => true,
            (ExecutionScope::Global, Some(_)) => false,
            (ExecutionScope::Project(bound), Some(owner)) => owner.scope == bound,
            (ExecutionScope::Project(_), None) => !exists,
        }
    }

    /// Refuse an execution this store is not bound to, before touching it.
    ///
    /// # Errors
    ///
    /// Always. The `Ok` arm is the caller's: this is what it returns when
    /// [`Self::admits`] said no.
    pub fn refuse<T>(
        self,
        execution: &ExecutionId,
        recorded: Option<&ExecutionOwnership>,
    ) -> Result<T> {
        Err(StoreError::OutOfScope {
            execution: execution.to_string(),
            bound: self.scope.label(),
            holder: recorded
                .map_or_else(|| ExecutionScope::Global.label(), ExecutionOwnership::label),
        })
    }

    /// The check every per-execution operation makes first.
    ///
    /// # Errors
    ///
    /// [`StoreError::OutOfScope`] naming the execution, this store's scope and
    /// the one that holds it.
    pub fn check(
        self,
        execution: &ExecutionId,
        recorded: Option<&ExecutionOwnership>,
        exists: bool,
    ) -> Result<()> {
        if self.admits(recorded, exists) {
            Ok(())
        } else {
            self.refuse(execution, recorded)
        }
    }

    /// What one append may do, given what the store already holds.
    ///
    /// `Ok(Some(record))` is "write this ownership in this transaction"; it is
    /// returned exactly once per execution, for the append that creates its
    /// stream. `Ok(None)` is "the ownership is already what it should be, or
    /// there is none to write". Everything else is a refusal, and each of them
    /// is one of the ways ownership could otherwise move:
    ///
    /// * an unscoped append to an owned execution — the global reactor, worker,
    ///   launcher, timer or publisher reaching a project's run;
    /// * a project append to an execution nobody owns — a project route
    ///   adopting a global run;
    /// * a project append to *another* project's run;
    /// * a start that names a different owner for an execution that has one;
    /// * an unscoped append that carries an ownership record at all.
    ///
    /// # Errors
    ///
    /// [`StoreError::OutOfScope`] or [`StoreError::OwnershipConflict`].
    pub fn appending(
        self,
        execution: &ExecutionId,
        establishing: Option<&ExecutionOwnership>,
        recorded: Option<&ExecutionOwnership>,
        stream_is_empty: bool,
    ) -> Result<Option<ExecutionOwnership>> {
        self.check(execution, recorded, !stream_is_empty)?;
        match (self.scope, establishing) {
            // The ordinary append, on either side of the boundary: the record
            // is already what it is, and nothing this decision does touches it.
            (_, None) => Ok(None),
            // A global store may not mint an owner. Only a project-bound one
            // can, which is what keeps every unscoped path — including one that
            // was handed a record by mistake — from creating project data.
            (ExecutionScope::Global, Some(_)) => Err(StoreError::NotInThisScope {
                what: "establish project ownership: it is bound to the unscoped path".to_owned(),
            }),
            (ExecutionScope::Project(bound), Some(wanted)) => {
                if wanted.scope != bound {
                    return self.refuse(execution, Some(wanted));
                }
                match recorded {
                    // A repeated start of a run this store already owns. Equal
                    // is the redelivery the inbox is about to recognise;
                    // different is somebody starting *another* run under an id
                    // that is taken, and ownership is written once.
                    Some(held) if held == wanted => Ok(None),
                    Some(held) => Err(StoreError::OwnershipConflict {
                        execution: execution.to_string(),
                        holder: held.label(),
                    }),
                    // The first append of a project execution. The stream has
                    // to be empty: an owner arriving after a run has a history
                    // would be adopting it, which is the repointing this record
                    // exists to prevent.
                    None if stream_is_empty => Ok(Some(wanted.clone())),
                    None => self.refuse(execution, None),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aiwatcher_iam::{OrganizationId, ProjectId};

    fn scope() -> ProjectScope {
        ProjectScope {
            organization: OrganizationId::new(),
            project: ProjectId::new(),
        }
    }

    fn plan() -> ExecutionPlan {
        ExecutionPlan::seal(
            DefinitionKind::Workflow,
            "import".to_owned(),
            DefinitionRevision("ab".repeat(32)),
            Vec::new(),
            Vec::new(),
        )
    }

    fn owner(scope: ProjectScope, subject: &str) -> ExecutionOwnership {
        ExecutionOwnership::of(
            &ProjectStart::new(
                scope,
                Principal::new("https://id.example", subject).expect("a principal"),
            ),
            &plan(),
        )
    }

    fn execution() -> ExecutionId {
        ExecutionId::new("run-1")
    }

    #[test]
    fn an_unowned_execution_is_the_unscoped_store_s_and_nobody_else_s() {
        let one = scope();
        assert!(ScopeBinding::global().admits(None, true));
        assert!(
            !ScopeBinding::global()
                .bind(one)
                .expect("a binding")
                .admits(None, true),
            "a project store may not adopt a run nobody owns"
        );
        assert!(
            ScopeBinding::global()
                .bind(one)
                .expect("a binding")
                .admits(None, false),
            "an id nobody has used is nobody's yet"
        );
    }

    #[test]
    fn an_owned_execution_is_its_own_project_s_and_never_the_global_path_s() {
        let (one, other) = (scope(), scope());
        let owned = owner(one, "alice");
        assert!(!ScopeBinding::global().admits(Some(&owned), true));
        assert!(
            ScopeBinding::global()
                .bind(one)
                .expect("a binding")
                .admits(Some(&owned), true)
        );
        assert!(
            !ScopeBinding::global()
                .bind(other)
                .expect("a binding")
                .admits(Some(&owned), true)
        );
    }

    #[test]
    fn a_store_binds_to_one_project_and_refuses_a_second() {
        let (one, other) = (scope(), scope());
        let bound = ScopeBinding::global().bind(one).expect("a binding");
        assert_eq!(bound.bind(one).expect("the same project"), bound);
        let refused = bound.bind(other).expect_err("a second project");
        assert!(
            matches!(refused, StoreError::NotInThisScope { .. }),
            "{refused}"
        );
        assert!(refused.says_the_same_next_time());
    }

    #[test]
    fn the_first_append_of_a_project_execution_writes_its_owner_and_the_next_does_not() {
        let one = scope();
        let bound = ScopeBinding::global().bind(one).expect("a binding");
        let owned = owner(one, "alice");

        let written = bound
            .appending(&execution(), Some(&owned), None, true)
            .expect("the first append");
        assert_eq!(written.as_ref(), Some(&owned));

        // The same start again, once the record exists: a redelivery, not a
        // second owner.
        assert_eq!(
            bound
                .appending(&execution(), Some(&owned), Some(&owned), false)
                .expect("a redelivered start"),
            None
        );
        // And every ordinary append afterwards carries none at all.
        assert_eq!(
            bound
                .appending(&execution(), None, Some(&owned), false)
                .expect("a command"),
            None
        );
    }

    #[test]
    fn a_start_naming_a_different_owner_does_not_repoint_the_execution() {
        let one = scope();
        let bound = ScopeBinding::global().bind(one).expect("a binding");
        let held = owner(one, "alice");
        let wanted = owner(one, "mallory");

        let refused = bound
            .appending(&execution(), Some(&wanted), Some(&held), false)
            .expect_err("a second owner");
        assert!(
            matches!(refused, StoreError::OwnershipConflict { .. }),
            "{refused}"
        );
        assert!(refused.says_the_same_next_time());
    }

    #[test]
    fn an_owner_may_not_arrive_after_the_run_has_a_history() {
        // Adopting a started stream is the repointing this record prevents,
        // reached from the other side: the ownership would be written by
        // somebody who did not start the run.
        let one = scope();
        let bound = ScopeBinding::global().bind(one).expect("a binding");
        let refused = bound
            .appending(&execution(), Some(&owner(one, "alice")), None, false)
            .expect_err("an adoption");
        assert!(
            matches!(refused, StoreError::OutOfScope { .. }),
            "{refused}"
        );
    }

    #[test]
    fn the_unscoped_path_neither_appends_to_an_owned_run_nor_mints_an_owner() {
        let one = scope();
        let owned = owner(one, "alice");
        let global = ScopeBinding::global();

        let refused = global
            .appending(&execution(), None, Some(&owned), false)
            .expect_err("a global append to an owned run");
        assert!(
            matches!(refused, StoreError::OutOfScope { .. }),
            "{refused}"
        );

        let refused = global
            .appending(&execution(), Some(&owned), None, true)
            .expect_err("a global store minting an owner");
        assert!(
            matches!(refused, StoreError::NotInThisScope { .. }),
            "{refused}"
        );
    }

    #[test]
    fn a_project_store_refuses_another_project_s_record_even_on_a_first_append() {
        let (one, other) = (scope(), scope());
        let bound = ScopeBinding::global().bind(one).expect("a binding");
        let refused = bound
            .appending(&execution(), Some(&owner(other, "alice")), None, true)
            .expect_err("somebody else's scope");
        assert!(
            matches!(refused, StoreError::OutOfScope { .. }),
            "{refused}"
        );
    }

    #[test]
    fn a_scope_key_tells_two_projects_apart_and_says_nothing_for_the_global_one() {
        let one = scope();
        assert_eq!(ExecutionScope::Global.key(), "");
        assert_eq!(
            ExecutionScope::Project(one).key(),
            format!("{}/{}", one.organization.0, one.project.0)
        );
        assert_ne!(
            ExecutionScope::Project(one).key(),
            ExecutionScope::Project(scope()).key()
        );
    }
}
