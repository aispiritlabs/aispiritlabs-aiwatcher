//! One Flow PHP query, as a managed step.
//!
//! Everything a query step shares with the other engines — the request, the
//! answer, the receipt, the lookup, the address and which engine it is wired
//! to — is in [`super::query`]. What is Flow's is the kind of step it claims,
//! the binding it reads, and where its routes are.
//!
//! **Its routes stay under `/flow` for the release that renames them.** The
//! service answers `/query/*` beside `/flow/*` from this release on, but an
//! older image serves only the second, and a server rolled out ahead of it
//! would be asking a path that answers 404. [`super::query`] refuses that
//! rather than reading it as a table with no rows, so the step would fail
//! loudly instead of completing over nothing — and it still should not happen
//! in an ordinary rollout. Moving to `/query` belongs to the release that drops
//! the alias.

use aiwatcher_execution::{
    ActivityCommand, ActivityContext, ActivityError, ActivityExecutor, ActivityResult,
    PriorAttempt, RuntimeBinding, RuntimeKind,
};
use async_trait::async_trait;

use super::artifacts::Artifacts;
use super::query::QueryClient;

/// Where the Flow service's routes are, for the release that renames them.
const ROUTES: &str = "/flow";

#[derive(Debug)]
pub struct FlowExecutor {
    client: QueryClient,
}

impl FlowExecutor {
    /// # Errors
    ///
    /// When the HTTP client cannot be built at all.
    pub fn new(endpoint: String, artifacts: Artifacts) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: QueryClient::new(endpoint, ROUTES, artifacts)?,
        })
    }
}

#[async_trait]
impl ActivityExecutor for FlowExecutor {
    fn runtime(&self) -> RuntimeKind {
        RuntimeKind::FlowPhp
    }

    async fn execute(
        &self,
        command: &ActivityCommand,
        context: &ActivityContext,
    ) -> Result<ActivityResult, ActivityError> {
        let RuntimeBinding::FlowPhp(spec) = &command.step.runtime else {
            // Unreachable through the registry, which routes by runtime. A
            // refusal rather than a guess: a Flow client handed another step
            // would send Python to a PHP parser, or a notebook to neither.
            return Err(ActivityError::user_code(
                "this step is not a Flow PHP query",
            ));
        };
        self.client.execute(spec, command, context).await
    }

    async fn lookup(&self, command: &ActivityCommand) -> Result<PriorAttempt, ActivityError> {
        self.client.lookup(command).await
    }
}
