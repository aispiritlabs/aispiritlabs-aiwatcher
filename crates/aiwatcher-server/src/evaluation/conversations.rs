//! Governed source rows and encryption for Evaluation's own retained objects.
use super::{LocalSource, unavailable};
use aiwatcher_conversations::{Error, Keyring, KeyringError};
use aiwatcher_evaluation::{
    CohortFiles, CohortRequest, EvaluationContext, EvaluationError, EvidenceCipher, EvidenceState,
    Result, SourceEvidence,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

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
#[derive(Deserialize, serde::Serialize, PartialEq, Eq)]
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

/// One corpus's cases, in the owner's order, and when the owner stops keeping
/// them.
struct Owned {
    cases: Vec<Case>,
    inputs: Vec<Value>,
    answers: Vec<Value>,
    expires_at: i64,
}

impl LocalSource {
    /// Every case a conversation corpus holds, as a cohort names them: by the
    /// digests of what was asked and answered, never by the words.
    async fn conversation_rows(&self, name: &str, version: &str, split: &str) -> Result<Owned> {
        // A corpus is exported to be measured on, whole: it deals no split.
        if split != "test" {
            return Err(unavailable(EvidenceState::Forbidden));
        }
        let owner = self
            .conversations
            .as_ref()
            .ok_or_else(|| unavailable(EvidenceState::Forbidden))?;
        let source = owner
            .verified_evaluation_rows(name, version)
            .await
            .map_err(source_error)?;
        let mut owned = Owned {
            cases: Vec::new(),
            inputs: Vec::new(),
            answers: Vec::new(),
            expires_at: source.expires_at.unix_timestamp(),
        };
        for row in source.rows {
            let id = row["eligibility"][1]["turn_id"]
                .as_str()
                .ok_or_else(|| unavailable(EvidenceState::CorruptArtifact))?
                .to_owned();
            let input = json!({"question": row["prompt"]});
            let answer = json!({"answer": row["response"]});
            owned.cases.push(Case {
                case_id: id,
                input_digest: digest(&input)?,
                expected_digest: digest(&answer)?,
            });
            owned.inputs.push(input);
            owned.answers.push(answer);
        }
        Ok(owned)
    }

    /// The first `limit` cases of a corpus, as the three files a cohort pins.
    pub(super) async fn conversation_cohort(&self, request: &CohortRequest) -> Result<CohortFiles> {
        let owned = self
            .conversation_rows(
                &request.dataset.name,
                &request.dataset.version,
                &request.split,
            )
            .await?;
        super::cohort_files(
            &owned.cases,
            request.limit,
            &json!({"type": "object", "required": ["question"]}),
            &json!({"type": "object", "required": ["answer"]}),
        )
    }

    pub(super) async fn conversation_cases(
        &self,
        pinned: &[u8],
        context: &EvaluationContext,
    ) -> Result<SourceEvidence> {
        let owned = self
            .conversation_rows(
                &context.dataset.name,
                &context.dataset.version,
                &context.split,
            )
            .await?;
        let approved: Cases = serde_json::from_slice(pinned)
            .map_err(|_| unavailable(EvidenceState::CorruptArtifact))?;
        // The owner's first cases, as many as the cohort declares: a cohort
        // taken with a limit selects a prefix, and one without selects them all.
        let selected = usize::try_from(context.case_count)
            .map_err(|_| unavailable(EvidenceState::CorruptArtifact))?;
        if approved.schema_version != 1
            || approved.cases.len() != selected
            || owned.cases.get(..selected) != Some(approved.cases.as_slice())
        {
            return Err(unavailable(EvidenceState::CorruptArtifact));
        }
        let mut expected = BTreeMap::new();
        let mut inputs = BTreeMap::new();
        for ((case, input), answer) in owned
            .cases
            .into_iter()
            .zip(owned.inputs)
            .zip(owned.answers)
            .take(selected)
        {
            if expected.insert(case.case_id.clone(), answer).is_some() {
                return Err(unavailable(EvidenceState::CorruptArtifact));
            }
            inputs.insert(case.case_id, input);
        }
        Ok(SourceEvidence {
            expected,
            // What somebody said to the assistant, for a judge the card shows
            // it to — whose context then says it reads the archive.
            inputs,
            expires_at: Some(owned.expires_at),
            bundle_digest: None,
            earlier_bundle_digest: None,
        })
    }
}
