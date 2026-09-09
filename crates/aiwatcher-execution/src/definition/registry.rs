//! Immutable authored versions and their latest head, outside run retention.

use std::sync::Arc;

use aiwatcher_core::prompts::ObjectStore;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

use super::WorkflowSpec;
use crate::{DefinitionRevision, StoreError, digest};

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
    /// Storage failures; callers must compile the definition before calling this method.
    pub async fn save(
        &self,
        definition: WorkflowSpec,
        registered_by: String,
        registered_at: OffsetDateTime,
    ) -> crate::Result<SavedWorkflow> {
        // Keep the public store door safe too: API and SDK are not its only possible callers.
        definition
            .compile()
            .map_err(|error| StoreError::Backend(error.to_string()))?;
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
    /// Storage failures or a corrupt saved record.
    pub async fn get(
        &self,
        name: &str,
        revision: Option<&str>,
    ) -> crate::Result<Option<SavedWorkflow>> {
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
        let Some(body) = self.store.get(&key).await.map_err(backend)? else {
            return Ok(None);
        };
        let saved: SavedWorkflow = serde_json::from_slice(&body).map_err(backend)?;
        if saved.definition.name != name
            || saved.revision != saved.definition.revision()
            || revision.is_some_and(|revision| revision != saved.revision.0)
        {
            return Err(StoreError::Backend(
                "workflow definition integrity check failed".to_owned(),
            ));
        }
        Ok(Some(saved))
    }

    /// Authored heads only. The number grows with definitions, never with executions.
    /// # Errors
    /// Storage failures or a corrupt saved record.
    pub async fn list(&self) -> crate::Result<Vec<SavedWorkflow>> {
        let mut definitions = Vec::new();
        for entry in self.store.list("workflows/heads/").await.map_err(backend)? {
            if let Some(body) = self.store.get(&entry.key).await.map_err(backend)? {
                definitions.push(serde_json::from_slice::<SavedWorkflow>(&body).map_err(backend)?);
            }
        }
        definitions.sort_by(|left, right| left.definition.name.cmp(&right.definition.name));
        Ok(definitions)
    }

    async fn put(&self, key: &str, saved: &SavedWorkflow) -> crate::Result<()> {
        self.store
            .put(key, serde_json::to_vec(saved).map_err(backend)?)
            .await
            .map_err(backend)
    }
}

fn backend(error: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(error.to_string())
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
