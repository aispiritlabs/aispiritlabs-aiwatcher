//! Cohort derivation binds native owners without retaining global source access.
use super::*;
use aiwatcher_iam::ProjectScope;

#[derive(Debug, Clone)]
struct ProjectCohorts {
    scope: ProjectScope,
    source: Arc<LocalSource>,
}

pub(super) fn bind(root: &LocalSource, scope: ProjectScope) -> Result<Arc<dyn SourceAuthority>> {
    // Build a new adapter. Host directories, staged bundles, conversations,
    // prompts and model packages confer no capability on a cohort derivation.
    let mut source = LocalSource::new(None);
    if let Some(datasets) = &root.datasets {
        source = source.with_curation(Arc::new(
            datasets
                .for_project(scope)
                .map_err(|_| unavailable(EvidenceState::Forbidden))?,
        ));
    }
    if let Some(annotations) = &root.annotations {
        source = source.with_annotations(Arc::new(
            annotations
                .for_project(scope)
                .map_err(|_| unavailable(EvidenceState::Forbidden))?,
        ));
    }
    Ok(Arc::new(ProjectCohorts {
        scope,
        source: Arc::new(source),
    }))
}

#[async_trait]
impl SourceAuthority for ProjectCohorts {
    fn for_project_cohorts(&self, scope: ProjectScope) -> Result<Arc<dyn SourceAuthority>> {
        if scope != self.scope {
            return Err(unavailable(EvidenceState::Forbidden));
        }
        Ok(Arc::new(self.clone()))
    }

    async fn resolve(&self, _: &EvaluationManifest, _: &str) -> Result<SourceEvidence> {
        Err(unavailable(EvidenceState::Forbidden))
    }

    async fn derive_cohort(&self, request: &CohortRequest, subject: &str) -> Result<CohortFiles> {
        if !matches!(
            request.dataset.kind,
            DatasetKind::Curation | DatasetKind::Annotations
        ) {
            return Err(unavailable(EvidenceState::Forbidden));
        }
        self.source.derive_cohort(request, subject).await
    }
}
