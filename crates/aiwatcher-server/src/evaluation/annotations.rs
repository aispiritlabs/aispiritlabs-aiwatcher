//! Bind approved cases to verified COCO images and their complete expectations.
use super::{LocalSource, unavailable, verified};
use aiwatcher_annotations::{Error, Split};
use aiwatcher_evaluation::{
    EvaluationContext, EvaluationError, EvidenceState, Result, SourceEvidence,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

#[derive(Deserialize, PartialEq)]
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

impl LocalSource {
    pub(super) async fn annotation_cases(
        &self,
        root: &Path,
        context: &EvaluationContext,
    ) -> Result<SourceEvidence> {
        let owner = self
            .annotations
            .as_ref()
            .ok_or_else(|| unavailable(EvidenceState::Forbidden))?;
        let split = match context.split.as_str() {
            "train" => Split::Train,
            "validation" => Split::Validation,
            "test" => Split::Test,
            _ => return Err(unavailable(EvidenceState::Forbidden)),
        };
        let coco = owner
            .verified_coco(&context.dataset.name, &context.dataset.version, split)
            .await
            .map_err(owner_error)?;
        let artifact = &context.case_manifest;
        let approved: Cases = serde_json::from_slice(
            &verified(root, &artifact.name, &artifact.digest, artifact.size_bytes).await?,
        )
        .map_err(|_| unavailable(EvidenceState::CorruptArtifact))?;
        let images = coco["images"]
            .as_array()
            .ok_or_else(|| unavailable(EvidenceState::CorruptArtifact))?;
        let annotations = coco["annotations"]
            .as_array()
            .ok_or_else(|| unavailable(EvidenceState::CorruptArtifact))?;
        let cases: Vec<Case> = images.iter().map(|image| Case {
            case_id: image["file_name"].as_str().unwrap_or_default().into(),
            input: image.clone(),
            expected: json!({"categories": coco["categories"], "annotations": annotations.iter().filter(|a| a["image_id"] == image["id"]).collect::<Vec<_>>()}),
        }).collect();
        if approved.schema_version != 1
            || cases.len() as u64 != context.case_count
            || approved.cases != cases
        {
            return Err(unavailable(EvidenceState::CorruptArtifact));
        }
        Ok(SourceEvidence {
            expected: cases
                .into_iter()
                .map(|case| (case.case_id, case.expected))
                .collect::<BTreeMap<_, _>>(),
            expires_at: None,
            bundle_digest: None,
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
