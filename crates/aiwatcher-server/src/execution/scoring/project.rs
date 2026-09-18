//! Trusted, per-execution authority for a recorded project measurement.
//!
//! Not a wire type or a worker credential. Only server wiring may construct
//! this from authenticated, persisted execution ownership; a plan, parameter,
//! declaration author or approval is not such ownership. What builds one is
//! [`ProjectDispatcher`](crate::execution::project::ProjectDispatcher), from
//! the record on the bound store — and no production start path creates a
//! project execution for it to read: history, live reads and retention still
//! need scope (`docs/iam-01-kickoff.md`).
use std::sync::Arc;

use aiwatcher_evaluation::Registry;
use aiwatcher_execution::{
    ActivityCommand, ActivityError, ExecutionId, FailureClass, RuntimeBinding,
};
use aiwatcher_iam::{IamStore, Principal, ProjectScope};

#[derive(Debug)]
pub struct ProjectAuthority {
    iam: Arc<dyn IamStore>,
    scope: ProjectScope,
    principal: Principal,
    execution: ExecutionId,
    declaration: String,
}

impl ProjectAuthority {
    /// Bind trusted execution ownership, never fields received in a plan/body.
    /// This is not a cached grant: every use asks IAM for the current decision.
    #[must_use]
    pub fn new(
        iam: Arc<dyn IamStore>,
        scope: ProjectScope,
        principal: Principal,
        execution: ExecutionId,
        declaration: String,
    ) -> Self {
        Self {
            iam,
            scope,
            principal,
            execution,
            declaration,
        }
    }

    pub(super) const fn scope(&self) -> ProjectScope {
        self.scope
    }

    /// All three measuring runtimes, and the declaration compared in each.
    ///
    /// A card that asks a judge compiles to `judge_evaluation` and one that
    /// asks a scorer service to `external_evaluation`
    /// (`aiwatcher_api::scoring`'s `plan_for`), and those are the same
    /// measurement in the role that holds the client. What the binding may not
    /// change is *which* declaration: the pinned one is what this authority
    /// names, whichever variant carries it.
    pub(super) async fn authorize(
        &self,
        registry: &Registry,
        command: &ActivityCommand,
        declaration: &str,
    ) -> Result<(), ActivityError> {
        let pinned = matches!(
            &command.step.runtime,
            RuntimeBinding::ScoreEvaluation(spec)
                | RuntimeBinding::JudgeEvaluation(spec)
                | RuntimeBinding::ExternalEvaluation(spec)
                if spec.declaration == self.declaration
        );
        if registry.project_scope() != Some(self.scope)
            || command.key.execution_id != self.execution
            || command.key.step_id != command.step.id
            || declaration != self.declaration
            || !pinned
        {
            return Err(denied());
        }
        // The same question the dispatcher's own authority asks before the
        // work and before publication, in one place so a revocation reaches
        // all three.
        super::super::project::editor_grant(&self.iam, self.scope, &self.principal).await
    }
}

fn denied() -> ActivityError {
    ActivityError::new(
        FailureClass::Policy,
        "project execution is not authorized for this principal and measurement",
    )
}
