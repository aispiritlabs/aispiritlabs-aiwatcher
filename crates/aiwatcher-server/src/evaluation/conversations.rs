//! Governed source rows and encryption for Evaluation's own retained objects.
use super::{LocalSource, unavailable, verified};
use aiwatcher_conversations::{Error, Keyring, KeyringError};
use aiwatcher_evaluation::{
    EvaluationContext, EvaluationError, EvidenceCipher, EvidenceState, Result, SourceEvidence,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};

/// Uses the archive's established authenticated envelope, with Evaluation's
/// full object path as associated data. It owns no archive storage keys.
#[derive(Debug)]
pub struct ConversationCipher(pub Keyring);
impl EvidenceCipher for ConversationCipher {
    fn seal(&self, path: &str, plaintext: &[u8]) -> Result<Value> {
        Ok(serde_json::to_value(
            self.0.seal(path, plaintext).map_err(crypto_error)?,
        )?)
    }
    fn open(&self, path: &str, envelope: &Value) -> Result<Vec<u8>> {
        let sealed = serde_json::from_value(envelope.clone())
            .map_err(|_| unavailable(EvidenceState::CorruptArtifact))?;
        self.0.open(path, &sealed).map_err(crypto_error)
    }
}
fn crypto_error(error: KeyringError) -> EvaluationError {
    unavailable(match error {
        KeyringError::UnknownKey { .. } => EvidenceState::Forbidden,
        _ => EvidenceState::CorruptArtifact,
    })
}
fn source_error(error: Error) -> EvaluationError {
    match error {
        Error::NotFound(_) | Error::Erased(..) => unavailable(EvidenceState::DeletedSource),
        Error::Corrupt { .. } | Error::TooLarge { .. } => {
            unavailable(EvidenceState::CorruptArtifact)
        }
        Error::Crypto(error) => crypto_error(error),
        Error::Store(error) => EvaluationError::Storage(error),
        Error::Invalid(_) | Error::Rejected(_) | Error::Refused(_) => {
            unavailable(EvidenceState::Forbidden)
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cases {
    schema_version: u32,
    cases: Vec<Case>,
}
#[derive(Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Case {
    case_id: String,
    input_digest: String,
    expected_digest: String,
}
fn digest(value: &Value) -> Result<String> {
    // Both objects below have exactly one key; no producer-specific map ordering.
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(value)?)))
}
impl LocalSource {
    pub(super) async fn conversation_cases(
        &self,
        root: &Path,
        context: &EvaluationContext,
    ) -> Result<SourceEvidence> {
        if context.split != "test" {
            return Err(unavailable(EvidenceState::Forbidden));
        }
        let owner = self
            .conversations
            .as_ref()
            .ok_or_else(|| unavailable(EvidenceState::Forbidden))?;
        let source = owner
            .verified_evaluation_rows(&context.dataset.name, &context.dataset.version)
            .await
            .map_err(source_error)?;
        let approved: Cases = serde_json::from_slice(
            &verified(
                root,
                &context.case_manifest.name,
                &context.case_manifest.digest,
                context.case_manifest.size_bytes,
            )
            .await?,
        )
        .map_err(|_| unavailable(EvidenceState::CorruptArtifact))?;
        let mut cases = Vec::new();
        let mut expected = BTreeMap::new();
        for row in source.rows {
            let id = row["eligibility"][1]["turn_id"]
                .as_str()
                .ok_or_else(|| unavailable(EvidenceState::CorruptArtifact))?
                .to_owned();
            let input = json!({"question": row["prompt"]});
            let answer = json!({"answer": row["response"]});
            cases.push(Case {
                case_id: id.clone(),
                input_digest: digest(&input)?,
                expected_digest: digest(&answer)?,
            });
            if expected.insert(id, answer).is_some() {
                return Err(unavailable(EvidenceState::CorruptArtifact));
            }
        }
        if approved.schema_version != 1
            || approved.cases != cases
            || cases.len() as u64 != context.case_count
        {
            return Err(unavailable(EvidenceState::CorruptArtifact));
        }
        Ok(SourceEvidence {
            expected,
            expires_at: Some(source.expires_at.unix_timestamp()),
            bundle_digest: None,
        })
    }
}
