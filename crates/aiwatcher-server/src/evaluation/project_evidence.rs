//! Producer evidence: every native owner and staged member belongs to one project.
use super::*;
use aiwatcher_iam::ProjectScope;

#[derive(Debug, Clone)]
struct ProjectEvidence {
    scope: ProjectScope,
    source: Arc<LocalSource>,
}

pub(super) fn bind(root: &LocalSource, scope: ProjectScope) -> Result<Arc<dyn SourceAuthority>> {
    if root.bundle_scope.is_some_and(|current| current != scope) {
        return Err(unavailable(EvidenceState::Forbidden));
    }
    let store = root
        .bundles
        .as_ref()
        .ok_or_else(|| unavailable(EvidenceState::Forbidden))?;
    // Construct, never clone: neither the host directory nor the conversation
    // archive may become a fallback when a project lacks a source.
    let mut source = LocalSource::new(None).with_bundles(store.clone());
    source.bundle_scope = Some(scope);
    if let Some(owner) = &root.datasets {
        source = source.with_curation(Arc::new(
            owner
                .for_project(scope)
                .map_err(|_| unavailable(EvidenceState::Forbidden))?,
        ));
    }
    if let Some(owner) = &root.annotations {
        source = source.with_annotations(Arc::new(
            owner
                .for_project(scope)
                .map_err(|_| unavailable(EvidenceState::Forbidden))?,
        ));
    }
    if let Some(owner) = &root.prompts {
        source = source.with_prompts(Arc::new(
            owner
                .for_project(scope)
                .map_err(|_| unavailable(EvidenceState::Forbidden))?,
        ));
    }
    if let Some(owner) = &root.training {
        source = source.with_training(Arc::new(
            owner
                .for_project(scope)
                .map_err(|_| unavailable(EvidenceState::Forbidden))?,
        ));
    }
    Ok(Arc::new(ProjectEvidence {
        scope,
        source: Arc::new(source),
    }))
}

#[async_trait]
impl SourceAuthority for ProjectEvidence {
    fn for_project_evidence(&self, scope: ProjectScope) -> Result<Arc<dyn SourceAuthority>> {
        if scope != self.scope {
            return Err(unavailable(EvidenceState::Forbidden));
        }
        Ok(Arc::new(self.clone()))
    }

    fn for_project_cohorts(&self, scope: ProjectScope) -> Result<Arc<dyn SourceAuthority>> {
        if scope != self.scope {
            return Err(unavailable(EvidenceState::Forbidden));
        }
        self.source.for_project_cohorts(scope)
    }

    async fn resolve(
        &self,
        manifest: &EvaluationManifest,
        subject: &str,
    ) -> Result<SourceEvidence> {
        // Built-in scoring is admitted against the project's card by Registry;
        // the variant's bytes still pass every ordinary owner/bundle check.
        // Admission grants no execution authority. Judges and external scoring
        // remain closed until their own project execution boundary exists.
        if manifest.context.judge.is_some()
            || manifest.context.external_calibration.is_some()
            || (manifest.context.scored_here()
                && !matches!(
                    manifest.context.dataset.kind,
                    DatasetKind::Curation | DatasetKind::Annotations
                ))
            || !matches!(
                manifest.context.dataset.kind,
                DatasetKind::External | DatasetKind::Curation | DatasetKind::Annotations
            )
        {
            return Err(unavailable(EvidenceState::Forbidden));
        }
        self.source.resolve(manifest, subject).await
    }

    async fn derive_cohort(&self, request: &CohortRequest, subject: &str) -> Result<CohortFiles> {
        self.for_project_cohorts(self.scope)?
            .derive_cohort(request, subject)
            .await
    }
}
