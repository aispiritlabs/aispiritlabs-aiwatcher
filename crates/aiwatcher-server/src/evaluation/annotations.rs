//! Bind approved cases to verified COCO images and their complete expectations.
use super::{LocalSource, unavailable};
use aiwatcher_annotations::{Error, Split};
use aiwatcher_evaluation::{
    CohortFiles, CohortRequest, EvaluationContext, EvaluationError, EvidenceState, Result,
    SourceEvidence,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Deserialize, serde::Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Case {
    case_id: String,
    input: Value,
    expected: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cases {
    schema_version: u32,
    cases: Vec<Case>,
}

fn split_of(name: &str) -> Result<Split> {
    match name {
        "train" => Ok(Split::Train),
        "validation" => Ok(Split::Validation),
        "test" => Ok(Split::Test),
        _ => Err(unavailable(EvidenceState::Forbidden)),
    }
}

impl LocalSource {
    /// Every image an export deals to one split, as a case: the image record
    /// it asks about and every annotation of it, under the export's categories.
    async fn annotation_rows(&self, project: &str, export: &str, split: &str) -> Result<Vec<Case>> {
        let owner = self
            .annotations
            .as_ref()
            .ok_or_else(|| unavailable(EvidenceState::Forbidden))?;
        let coco = owner
            .verified_coco(project, export, split_of(split)?)
            .await
            .map_err(owner_error)?;
        let images = coco["images"]
            .as_array()
            .ok_or_else(|| unavailable(EvidenceState::CorruptArtifact))?;
        let annotations = coco["annotations"]
            .as_array()
            .ok_or_else(|| unavailable(EvidenceState::CorruptArtifact))?;
        Ok(images.iter().map(|image| Case {
            case_id: image["file_name"].as_str().unwrap_or_default().into(),
            input: image.clone(),
            expected: json!({"categories": coco["categories"], "annotations": annotations.iter().filter(|a| a["image_id"] == image["id"]).collect::<Vec<_>>()}),
        }).collect())
    }

    /// The first `limit` images of an export's split, as the files a cohort pins.
    pub(super) async fn annotation_cohort(&self, request: &CohortRequest) -> Result<CohortFiles> {
        let cases = self
            .annotation_rows(
                &request.dataset.name,
                &request.dataset.version,
                &request.split,
            )
            .await?;
        super::cohort_files(
            &cases,
            request.limit,
            &json!({"type": "object", "required": ["file_name"]}),
            &json!({"type": "object", "required": ["categories", "annotations"]}),
        )
    }

    pub(super) async fn annotation_cases(
        &self,
        pinned: &[u8],
        context: &EvaluationContext,
    ) -> Result<SourceEvidence> {
        let mut cases = self
            .annotation_rows(
                &context.dataset.name,
                &context.dataset.version,
                &context.split,
            )
            .await?;
        let approved: Cases = serde_json::from_slice(pinned)
            .map_err(|_| unavailable(EvidenceState::CorruptArtifact))?;
        // The export's first images, as many as the cohort declares.
        let selected = usize::try_from(context.case_count)
            .map_err(|_| unavailable(EvidenceState::CorruptArtifact))?;
        if approved.schema_version != 1
            || approved.cases.len() != selected
            || cases.get(..selected) != Some(approved.cases.as_slice())
        {
            return Err(unavailable(EvidenceState::CorruptArtifact));
        }
        cases.truncate(selected);
        let inputs = cases
            .iter()
            .map(|case| (case.case_id.clone(), case.input.clone()))
            .collect();
        Ok(SourceEvidence {
            expected: cases
                .into_iter()
                .map(|case| (case.case_id, case.expected))
                .collect::<BTreeMap<_, _>>(),
            inputs,
            expires_at: None,
            bundle_digest: None,
            earlier_bundle_digest: None,
        })
    }
}
fn owner_error(error: Error) -> EvaluationError {
    match error {
        Error::NotFound(_) => unavailable(EvidenceState::DeletedSource),
        Error::Corrupt { .. } | Error::TooLarge { .. } => {
            unavailable(EvidenceState::CorruptArtifact)
        }
        Error::Store(error) => EvaluationError::Storage(error),
        Error::Invalid(_) | Error::Rejected(_) => unavailable(EvidenceState::Forbidden),
    }
}
