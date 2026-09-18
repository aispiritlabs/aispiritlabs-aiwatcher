//! Trusted, per-execution authority for a recorded project measurement.
//!
//! Not a wire type or a worker credential. Only server wiring may construct
//! this from authenticated, persisted execution ownership; a plan, parameter,
//! declaration author or approval is not such ownership. No production start
//! path creates these yet: history, artifacts and live reads still need scope.
use std::sync::Arc;

use aiwatcher_evaluation::Registry;
use aiwatcher_execution::{
    ActivityCommand, ActivityError, ExecutionId, FailureClass, RuntimeBinding,
};
use aiwatcher_iam::{IamStore, Principal, ProjectRole, ProjectScope};

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

    pub(super) async fn authorize(
        &self,
        registry: &Registry,
        command: &ActivityCommand,
        declaration: &str,
    ) -> Result<(), ActivityError> {
        if registry.project_scope() != Some(self.scope)
            || command.key.execution_id != self.execution
            || command.key.step_id != command.step.id
            || declaration != self.declaration
            || !matches!(&command.step.runtime, RuntimeBinding::ScoreEvaluation(spec) if spec.declaration == self.declaration)
        {
            return Err(denied());
        }
        let access =
            self.iam
                .access(self.scope, &self.principal)
                .await
                .map_err(|error| match error {
                    aiwatcher_iam::Error::Backend(_) => {
                        ActivityError::transient("project execution authorization is unavailable")
                    }
                    _ => denied(),
                })?;
        if access.project.scope != self.scope || access.role < ProjectRole::Editor {
            return Err(denied());
        }
        Ok(())
    }
}

fn denied() -> ActivityError {
    ActivityError::new(
        FailureClass::Policy,
        "project execution is not authorized for this principal and measurement",
    )
}
