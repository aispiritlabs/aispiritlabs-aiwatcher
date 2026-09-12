//! Immutable authored versions and their latest head, outside run retention.

use std::sync::Arc;

use aiwatcher_core::prompts::ObjectStore;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

use super::WorkflowSpec;
use crate::{DefinitionError, DefinitionRevision, digest};

/// This registry's own refusals, never the workflow store's: an object store
/// that is briefly unreachable and a stored definition that will not read back
/// are different answers to the only question a scheduler asks about a start.
type Result<T> = std::result::Result<T, DefinitionError>;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, ToSchema)]
pub struct SavedWorkflow {
    pub definition: WorkflowSpec,
    pub revision: DefinitionRevision,
    pub registered_by: String,
    #[serde(with = "time::serde::rfc3339")]
    pub registered_at: OffsetDateTime,
}

#[derive(Debug)]
pub struct DefinitionRegistry {
    store: Arc<dyn ObjectStore>,
}

impl DefinitionRegistry {
    #[must_use]
    pub fn new(store: Arc<dyn ObjectStore>) -> Self {
        Self { store }
    }

    /// Save a validated definition, version before head. Repeated content is one revision.
    ///
    /// # Errors
    /// A definition that does not compile, a store that refused, or bytes that
    /// will not read back.
    pub async fn save(
        &self,
        definition: WorkflowSpec,
        registered_by: String,
        registered_at: OffsetDateTime,
    ) -> Result<SavedWorkflow> {
        // Keep the public store door safe too: API and SDK are not its only possible callers.
        definition
            .compile()
            .map_err(|error| DefinitionError::Refused(error.problems().to_vec()))?;
        let revision = definition.revision();
        let saved = match self.get(&definition.name, Some(&revision.0)).await? {
            Some(saved) => saved,
            None => {
                let saved = SavedWorkflow {
                    definition,
                    revision,
                    registered_by,
                    registered_at,
                };
                self.put(
                    &version_key(&saved.definition.name, &saved.revision.0),
                    &saved,
                )
                .await?;
                saved
            }
        };
        self.put(&head_key(&saved.definition.name), &saved).await?;
        Ok(saved)
    }

    /// # Errors
    /// A store that refused, or a stored record that is not this definition.
    pub async fn get(&self, name: &str, revision: Option<&str>) -> Result<Option<SavedWorkflow>> {
        let key = match revision {
            Some(revision) => {
                if revision.len() != 64
                    || !revision
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                {
                    return Ok(None);
                }
                version_key(name, revision)
            }
            None => head_key(name),
        };
        let Some(body) = self.store.get(&key).await? else {
            return Ok(None);
        };
        let saved: SavedWorkflow = read(&key, &body)?;
        if saved.definition.name != name
            || saved.revision != saved.definition.revision()
            || revision.is_some_and(|revision| revision != saved.revision.0)
        {
            return Err(DefinitionError::Corrupt {
                key,
                message: "it describes a different definition or revision".to_owned(),
            });
        }
        Ok(Some(saved))
    }

    /// Authored heads only. The number grows with definitions, never with executions.
    /// # Errors
    /// A store that refused, or a stored record that will not read back.
    pub async fn list(&self) -> Result<Vec<SavedWorkflow>> {
        let mut definitions = Vec::new();
        for entry in self.store.list("workflows/heads/").await? {
            if let Some(body) = self.store.get(&entry.key).await? {
                definitions.push(read(&entry.key, &body)?);
            }
        }
        definitions.sort_by(|left, right| left.definition.name.cmp(&right.definition.name));
        Ok(definitions)
    }

    async fn put(&self, key: &str, saved: &SavedWorkflow) -> Result<()> {
        let body = serde_json::to_vec(saved).map_err(|error| DefinitionError::Corrupt {
            key: key.to_owned(),
            message: error.to_string(),
        })?;
        self.store.put(key, body).await?;
        Ok(())
    }
}

fn read(key: &str, body: &[u8]) -> Result<SavedWorkflow> {
    serde_json::from_slice(body).map_err(|error| DefinitionError::Corrupt {
        key: key.to_owned(),
        message: error.to_string(),
    })
}

fn head_key(name: &str) -> String {
    format!("workflows/heads/{}.json", digest(name.as_bytes()))
}
fn version_key(name: &str, revision: &str) -> String {
    format!(
        "workflows/versions/{}/{revision}.json",
        digest(name.as_bytes())
    )
}
