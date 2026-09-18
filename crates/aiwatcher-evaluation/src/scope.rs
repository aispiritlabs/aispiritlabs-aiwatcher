//! Authored evaluation storage. Immutable documents keep their original identity.
use aiwatcher_core::{
    ports::{PortError, PortResult},
    storage::{ObjectEntry, ObjectStore},
};
use aiwatcher_iam::ProjectScope;
use async_trait::async_trait;
use std::sync::Arc;

pub(crate) fn component(value: &str, field: &str) -> crate::Result<()> {
    crate::text(value, field)?;
    crate::require(
        !matches!(value, "." | "..") && !value.contains(['/', '\\']),
        field,
        "must be a single object key component",
    )
}

fn refused() -> PortError {
    PortError::Rejected {
        target: "evaluation-store",
        message: "object key is outside the authored evaluation registry".into(),
    }
}

/// What an authored project registry opens: the families people write.
const AUTHORED: [&str; 7] = [
    crate::store::RUBRICS,
    crate::store::ASSESSMENTS,
    crate::store::SCORECARDS,
    crate::store::REVIEWS,
    crate::store::REVIEW_TARGETS,
    crate::store::COHORTS,
    crate::store::RECORDINGS,
];

/// What an evidence registry opens on top of them: a measurement's own
/// families. A judge's pinned settings and the replies it gave belong to the
/// declaration that asked, so they are the project's and never the instance's
/// — a reply kept beside one project's declaration must not answer another's.
const MEASURED: [&str; 6] = [
    "evaluations/",
    crate::store::CALIBRATIONS,
    crate::store::SCORING_RUNS,
    crate::store::JUDGE_SETTINGS,
    crate::store::JUDGE_REPLIES,
    crate::store::EXTERNAL_REPLIES,
];

/// The one key a scoped registry reads outside its own prefix, and only reads.
///
/// The scorer service's catalog is the deployment's description of what it
/// runs — adapter names, framework releases, the model a graded metric asks,
/// and each metric's unit and direction. It holds no project's data, no
/// address and no credential, and a card published in a project pins what it
/// said. A project may never write, list or delete it: what the service says
/// is the work role's to record, once, for the whole deployment.
const DEPLOYMENT_READS: [&str; 1] = [crate::store::SCORER_CATALOG];

#[derive(Debug)]
pub(crate) struct ProjectStore {
    inner: Arc<dyn ObjectStore>,
    prefix: Option<String>,
    evidence: bool,
}
impl ProjectStore {
    pub(crate) fn legacy(inner: Arc<dyn ObjectStore>) -> Self {
        Self {
            inner,
            prefix: None,
            evidence: false,
        }
    }
    pub(crate) fn scoped(inner: Arc<dyn ObjectStore>, scope: ProjectScope) -> Self {
        Self {
            inner,
            evidence: false,
            prefix: Some(format!(
                "evaluation-scopes/{}/{}/registry/",
                scope.organization.0, scope.project.0
            )),
        }
    }
    pub(crate) fn evidence(inner: Arc<dyn ObjectStore>, scope: ProjectScope) -> Self {
        Self {
            evidence: true,
            ..Self::scoped(inner, scope)
        }
    }
    fn key(&self, relative: &str, listing: bool) -> PortResult<String> {
        let checked = if listing {
            relative.strip_suffix('/').unwrap_or(relative)
        } else {
            relative
        };
        // Empty listings are used by legacy maintenance. Scoped callers can
        // only list one of the authored families, never the backing store.
        if !(checked.is_empty() && listing && self.prefix.is_none())
            && checked
                .split('/')
                .any(|part| matches!(part, "" | "." | "..") || part.contains(['\\', '\0']))
        {
            return Err(refused());
        }
        if let Some(prefix) = &self.prefix {
            let evidence_key =
                self.evidence && MEASURED.iter().any(|family| relative.starts_with(family));
            if !evidence_key && !AUTHORED.iter().any(|family| relative.starts_with(family)) {
                return Err(refused());
            }
            Ok(format!("{prefix}{relative}"))
        } else {
            Ok(relative.to_owned())
        }
    }

    /// The deployment's own key, when this read is one of the few a project
    /// makes outside its prefix. Reads only: every other operation goes
    /// through [`Self::key`], so nothing here can write or erase one.
    fn read_key(&self, relative: &str) -> PortResult<String> {
        if self.prefix.is_some() && DEPLOYMENT_READS.contains(&relative) {
            return Ok(relative.to_owned());
        }
        self.key(relative, false)
    }
}

#[async_trait]
impl ObjectStore for ProjectStore {
    async fn put(&self, key: &str, body: Vec<u8>) -> PortResult<()> {
        self.inner.put(&self.key(key, false)?, body).await
    }
    async fn create(&self, key: &str, body: Vec<u8>) -> PortResult<bool> {
        self.inner.create(&self.key(key, false)?, body).await
    }
    async fn get(&self, key: &str) -> PortResult<Option<Vec<u8>>> {
        self.inner.get(&self.read_key(key)?).await
    }
    async fn list(&self, prefix: &str) -> PortResult<Vec<ObjectEntry>> {
        let full = self.key(prefix, true)?;
        let mut entries = self.inner.list(&full).await?;
        for entry in &mut entries {
            if !entry.key.starts_with(&full) {
                return Err(refused());
            }
            let relative = match &self.prefix {
                Some(root) => entry.key.strip_prefix(root).ok_or_else(refused)?,
                None => &entry.key,
            };
            self.key(relative, false)?;
            entry.key = relative.to_owned();
        }
        Ok(entries)
    }
    async fn delete(&self, key: &str) -> PortResult<()> {
        self.inner.delete(&self.key(key, false)?).await
    }
}

/// A project binding must never retain the instance's source resolver.
#[derive(Debug)]
pub(crate) struct NoSources;
#[async_trait]
impl crate::SourceAuthority for NoSources {
    async fn resolve(
        &self,
        _: &crate::EvaluationManifest,
        _: &str,
    ) -> crate::Result<crate::SourceEvidence> {
        Err(crate::EvaluationError::Unavailable(
            crate::EvidenceState::Forbidden,
        ))
    }
}
