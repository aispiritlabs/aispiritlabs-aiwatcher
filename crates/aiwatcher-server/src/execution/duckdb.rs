//! One DuckDB query, as a managed step (AW-3).
//!
//! Everything a query step shares with the other engines — the request, the
//! answer, the receipt, the lookup, the address and which engine it is wired
//! to — is in [`super::query`]. What is DuckDB's is the kind of step it claims,
//! the binding it reads, and where its routes are: `/query`, which is all a
//! Python engine has ever served.
//!
//! Nothing here runs DuckDB. A query is Python calling DuckDB's relational API,
//! and it runs in the query service's own children (AW-3's first decision):
//! this process keeps the read model's memory contract and none of a query's.
//! The module shares its name with the `duckdb` crate the workflow store links
//! behind its feature, so code in this crate that means the crate writes it
//! `::duckdb`.

use aiwatcher_execution::{
    ActivityCommand, ActivityContext, ActivityError, ActivityExecutor, ActivityResult,
    PriorAttempt, RuntimeBinding, RuntimeKind,
};
use async_trait::async_trait;

use super::artifacts::Artifacts;
use super::query::QueryClient;

/// Where a Python engine's routes are.
const ROUTES: &str = "/query";

#[derive(Debug)]
pub struct DuckDbExecutor {
    client: QueryClient,
}

impl DuckDbExecutor {
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
impl ActivityExecutor for DuckDbExecutor {
    fn runtime(&self) -> RuntimeKind {
        RuntimeKind::DuckDb
    }

    async fn execute(
        &self,
        command: &ActivityCommand,
        context: &ActivityContext,
    ) -> Result<ActivityResult, ActivityError> {
        let RuntimeBinding::DuckDb(spec) = &command.step.runtime else {
            // Unreachable through the registry, which routes by runtime. A
            // refusal rather than a guess: another engine's text sent here is a
            // name DuckDB's namespace does not hold, and a notebook is neither.
            return Err(ActivityError::user_code("this step is not a DuckDB query"));
        };
        self.client.execute(spec, command, context).await
    }

    async fn lookup(&self, command: &ActivityCommand) -> Result<PriorAttempt, ActivityError> {
        self.client.lookup(command).await
    }
}
