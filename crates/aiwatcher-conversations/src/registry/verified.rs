//! Bounded verification of frozen prompt/response rows and their live policies.
use super::Registry;
use crate::{
    ArchivedTurn, Error, ExportFormat, ExportManifest, Result, ReviewState, Role, SealedObject,
    TrainingScope, TurnContent, digest, turn_id, validate_digest, validate_name,
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use time::OffsetDateTime;

const MAX_ROWS: usize = 1_000;
const MAX_BYTES: usize = 100 * 1024 * 1024;

/// Decrypted only for a caller that already passed the archive content role.
/// No storage keys or key material escape the owner facade.
#[derive(Debug)]
pub struct VerifiedEvaluationRows {
    pub rows: Vec<Value>,
    pub expires_at: OffsetDateTime,
}

struct Budget(usize);
impl Budget {
    async fn bytes(
        &mut self,
        owner: &Registry,
        key: &str,
        limit: usize,
        sealed: bool,
    ) -> Result<Vec<u8>> {
        let bytes = owner.backend.bounded_bytes(key, limit.min(self.0)).await?;
        self.0 -= bytes.len();
        if !sealed {
            return Ok(bytes);
        }
        let envelope: SealedObject = decode(key, &bytes)?;
        let plain = owner.backend.keyring().open(key, &envelope)?;
        if plain.len() > self.0 {
            return Err(Error::TooLarge {
                what: "verified conversation bytes",
                size: plain.len(),
                limit: self.0,
            });
        }
        self.0 -= plain.len();
        Ok(plain)
    }
    async fn read<T: DeserializeOwned>(
        &mut self,
        owner: &Registry,
        key: &str,
        limit: usize,
        sealed: bool,
    ) -> Result<T> {
        decode(key, &self.bytes(owner, key, limit, sealed).await?)
    }
}
fn corrupt() -> Error {
    Error::Corrupt {
        key: "verified conversation export".into(),
        message: "identity or row mismatch".into(),
    }
}
fn decode<T: DeserializeOwned>(key: &str, bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|_| Error::Corrupt {
        key: key.into(),
        message: "invalid document".into(),
    })
}
fn field<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    value[name].as_str().ok_or_else(corrupt)
}

impl Registry {
    /// Verify a complete prompt/response export, including every source turn's
    /// present consent, review, content digest and earliest retention deadline.
    /// The caller must enforce the same content role as `export_rows`.
    ///
    /// # Errors
    ///
    /// Refuses other formats or non-evaluation consent. Missing/erased source
    /// data, corrupt identities, crypto failures and bounded reads stay distinct.
    pub async fn verified_evaluation_rows(
        &self,
        name: &str,
        version: &str,
    ) -> Result<VerifiedEvaluationRows> {
        validate_name(name, "export name")?;
        validate_digest(version, "export version")?;
        let mut budget = Budget(MAX_BYTES);
        let manifest: ExportManifest = budget
            .read(
                self,
                &self.backend.manifest_key(name, version),
                4 * 1024 * 1024,
                false,
            )
            .await?;
        manifest.request.validate()?;
        validate_digest(&manifest.job_id, "export job")?;
        if manifest.name != name
            || manifest.version != version
            || manifest.request.name != name
            || manifest.request.digest() != manifest.request_digest
            || aiwatcher_jobs::version_of(&manifest.request_digest, &manifest.shards) != version
        {
            return Err(corrupt());
        }
        if let Some(withdrawal) = &manifest.withdrawn {
            return Err(Error::Erased(name.into(), withdrawal.at.to_string()));
        }
        if manifest.request.format != ExportFormat::PromptResponse
            || manifest.request.required_scope != TrainingScope::Evaluate
            || !manifest.request.require_human_review
        {
            return Err(Error::Refused(
                "evaluation requires a reviewed prompt/response export with evaluate scope".into(),
            ));
        }
        if manifest.counts.rows == 0
            || manifest.counts.rows > MAX_ROWS
            || manifest.shards.len() > MAX_ROWS
            || manifest.conversations.len() > MAX_ROWS
        {
            return Err(Error::TooLarge {
                what: "verified conversation rows/selection",
                size: manifest
                    .counts
                    .rows
                    .max(manifest.conversations.len())
                    .max(manifest.shards.len()),
                limit: MAX_ROWS,
            });
        }
        let mut rows = Vec::new();
        let mut expiry: Option<OffsetDateTime> = None;
        for (index, shard) in manifest.shards.iter().enumerate() {
            validate_digest(&shard.digest, "export shard")?;
            if shard.index != index || shard.rows > MAX_ROWS {
                return Err(corrupt());
            }
            let bytes = budget
                .bytes(
                    self,
                    &self.backend.shard_key(&manifest.job_id, index),
                    MAX_BYTES,
                    true,
                )
                .await?;
            if digest(&bytes) != shard.digest {
                return Err(corrupt());
            }
            let mut count = 0;
            for line in bytes.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
                if rows.len() >= MAX_ROWS {
                    return Err(corrupt());
                }
                let row: Value = decode("export row", line)?;
                let eligibility = row["eligibility"]
                    .as_array()
                    .filter(|items| items.len() == 2)
                    .ok_or_else(corrupt)?;
                let conversation = field(&eligibility[0], "conversation_id")?;
                if !manifest.conversations.iter().any(|id| id == conversation)
                    || field(&eligibility[1], "conversation_id")? != conversation
                {
                    return Err(corrupt());
                }
                for (entry, role, text) in [
                    (&eligibility[0], Role::User, "prompt"),
                    (&eligibility[1], Role::Assistant, "response"),
                ] {
                    let expires = self
                        .verify_evaluation_turn(
                            &mut budget,
                            &manifest,
                            entry,
                            role,
                            field(&row, text)?,
                        )
                        .await?;
                    expiry = Some(expiry.map_or(expires, |old| old.min(expires)));
                }
                rows.push(row);
                count += 1;
            }
            if count != shard.rows {
                return Err(corrupt());
            }
        }
        if rows.len() != manifest.counts.rows {
            return Err(corrupt());
        }
        Ok(VerifiedEvaluationRows {
            rows,
            expires_at: expiry.ok_or_else(corrupt)?,
        })
    }

    async fn verify_evaluation_turn(
        &self,
        budget: &mut Budget,
        manifest: &ExportManifest,
        entry: &Value,
        role: Role,
        text: &str,
    ) -> Result<OffsetDateTime> {
        let conversation = field(entry, "conversation_id")?;
        let id = field(entry, "turn_id")?;
        validate_name(conversation, "conversation")?;
        validate_digest(id, "turn")?;
        let turn: ArchivedTurn = budget
            .read(
                self,
                &self.backend.turn_key(conversation, id),
                2 * 1024 * 1024,
                false,
            )
            .await?;
        if turn.turn_id != id
            || turn.conversation_id != conversation
            || turn_id(conversation, &turn.message_id) != id
            || turn.content_digest != field(entry, "content_digest")?
            || turn.message_id != field(entry, "message_id")?
            || turn.role != role
        {
            return Err(corrupt());
        }
        if !turn.is_readable() {
            return Err(Error::Erased(id.into(), "source turn erased".into()));
        }
        if !turn.policy.consent.problems().is_empty()
            || !self.policy.check(&turn.policy).is_empty()
            || !turn.permits(TrainingScope::Evaluate)
            || turn.review.state != ReviewState::Approved
            || crate::export::excluded_for(&turn, &manifest.request).is_some()
        {
            return Err(Error::Refused(
                "source consent or review no longer permits evaluation".into(),
            ));
        }
        // Apply both the recorded deadline and the deployment's current cap.
        // No expiry is an unknown policy, never permission for indefinite copies.
        let recorded = turn
            .expires_at
            .ok_or_else(|| Error::Refused("source has no retention deadline".into()))?;
        let (retention, _) = self.policy.clamp(&turn.policy.retention);
        let expires = recorded.min(retention.expires_at(turn.received_at));
        let content: TurnContent = budget
            .read(self, &self.backend.content_key(id), 2 * 1024 * 1024, true)
            .await?;
        if content.digest() != turn.content_digest
            || (crate::export::format::Selected {
                turn: &turn,
                content: &content,
            })
            .text()
                != text
        {
            return Err(corrupt());
        }
        Ok(expires)
    }
}
