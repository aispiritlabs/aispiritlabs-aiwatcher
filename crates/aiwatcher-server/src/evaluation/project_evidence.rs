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
        // Admission grants no execution authority.
        //
        // A judge and a card's calibrated framework metrics are admitted here
        // too, and only for a measurement this deployment makes over a native
        // cohort: a producer's judge still has no adapter anywhere (ADR_0030),
        // and the registry has already held the rubric, the card, the frozen
        // human judgements and the pinned settings to this project.
        let measured_here = manifest.context.scored_here()
            && matches!(
                manifest.context.dataset.kind,
                DatasetKind::Curation | DatasetKind::Annotations
            );
        let asks_a_model =
            manifest.context.judge.is_some() || manifest.context.external_calibration.is_some();
        // What a provider is sent leaves this deployment, so the archive's own
        // seal, retention and erasure end at it. Refused here as well as in the
        // registry: this adapter is what a hand-written context reaches.
        let reads_archive = manifest
            .context
            .judge
            .as_ref()
            .is_some_and(|judge| judge.reads_archive)
            || manifest
                .context
                .external_calibration
                .as_ref()
                .is_some_and(|pin| pin.reads_archive);
        if (asks_a_model && !measured_here)
            || reads_archive
            || (manifest.context.scored_here() && !measured_here)
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
