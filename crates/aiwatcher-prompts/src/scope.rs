//! Storage ownership is outside prompt bytes, version IDs and optimization IDs.
//! This binds storage, not authority: callers must check a fresh IAM grant.
use crate::{Registry, RegistryError, Result};
use aiwatcher_iam::ProjectScope;

impl Registry {
    /// Bind every prompt operation to a project without changing content hashes.
    /// A bound registry may be reopened in its own scope, never rebound.
    pub fn for_project(&self, scope: ProjectScope) -> Result<Self> {
        if let Some(current) = self.scope {
            return if current == scope {
                Ok(self.clone())
            } else {
                Err(RegistryError::InvalidScope(
                    "registry is already bound to another project",
                ))
            };
        }
        let mut config = self.config.clone();
        config.prefix = format!(
            "{}/scopes/{}/{}/registry",
            config.prefix.trim_end_matches('/'),
            scope.organization.0,
            scope.project.0
        );
        let registry = Self {
            store: self.store.clone(),
            config,
            scope: Some(scope),
        };
        registry.check_key(&format!("{}/scope-check", registry.config.prefix))?;
        Ok(registry)
    }

    pub(crate) fn check_key(&self, key: &str) -> Result<()> {
        if self.scope.is_some()
            && (!key.starts_with(&format!("{}/", self.config.prefix))
                || key
                    .split('/')
                    .any(|part| matches!(part, "" | "." | "..") || part.contains('\\')))
        {
            return Err(RegistryError::InvalidScope(
                "object key escapes the project registry",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::{PromptFilter, PublishRequest, RegistryConfig, adapters::fs::FileObjectStore};
    use aiwatcher_core::ObjectStore;
    use aiwatcher_iam::{OrganizationId, ProjectId};
    use serde_json::json;
    use std::sync::Arc;

    fn request(text: &str) -> PublishRequest {
        serde_json::from_value(json!({"name":"shared.prompt","text":text,"label":"production","author":"original author","notes":"original note"})).unwrap()
    }
    fn scope() -> ProjectScope {
        ProjectScope {
            organization: OrganizationId::new(),
            project: ProjectId::new(),
        }
    }

    #[tokio::test]
    async fn project_storage_survives_reopen_without_changing_hashes_or_legacy_visibility() {
        let a_scope = scope();
        let dir =
            std::env::temp_dir().join(format!("aiwatcher-scoped-prompts-{}", a_scope.project.0));
        let store = Arc::new(FileObjectStore::open(&dir).await.unwrap());
        let legacy = Registry::new(store.clone(), RegistryConfig::default());
        let a = legacy.for_project(a_scope).unwrap();
        let b_scope = ProjectScope {
            project: ProjectId::new(),
            ..a_scope
        };
        let b = legacy.for_project(b_scope).unwrap();
        let other_org = legacy
            .for_project(ProjectScope {
                organization: OrganizationId::new(),
                ..a_scope
            })
            .unwrap();
        let original = a
            .publish(request("Answer {{question}} privately."))
            .await
            .unwrap();
        let name = &original.version.name;
        let pin = &original.version.version_id;
        for registry in [&b, &other_org, &legacy] {
            assert!(
                registry
                    .list(&PromptFilter::default())
                    .await
                    .unwrap()
                    .prompts
                    .is_empty()
            );
            assert!(registry.head(name).await.unwrap().is_none());
            assert!(
                registry
                    .verified_version(name, pin)
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(registry.set_label(name, "production", pin).await.is_err());
            assert!(registry.rebuild(name).await.is_err());
        }
        let same = b.publish(request(&original.version.text)).await.unwrap();
        assert!(same.created);
        assert_eq!(same.version.version_id, *pin);
        let different = b
            .publish(request("Other project current prompt"))
            .await
            .unwrap();
        assert_eq!(a.resolve(name, None).await.unwrap(), original.version);
        assert_eq!(b.resolve(name, None).await.unwrap(), different.version);
        let old = legacy.publish(request("Global prompt")).await.unwrap();
        // Even the legitimate legacy name `scopes` cannot enumerate project keys.
        let mut named_scopes = request("A legacy prompt named scopes");
        named_scopes.name = aiwatcher_core::prompts::PromptName::parse("scopes").unwrap();
        legacy.publish(named_scopes).await.unwrap();
        let page = legacy.list(&PromptFilter::default()).await.unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(page.prompts.len(), 2);
        assert_eq!(legacy.resolve(name, None).await.unwrap(), old.version);
        let optimization = a
            .record_optimization(
                name,
                serde_json::from_value(json!({
                    "optimization_id":"scoped-optimization", "algorithm":"test", "baseline":pin,
                    "candidate_text":"Candidate {{question}}", "primary_metric":"quality",
                    "test":[{"metric":"quality","baseline":0.5,"candidate":0.7}],
                    "dataset":"unchanged@dataset", "evaluation_id":"unchanged-report",
                    "report":{"private":"optimization evidence"}
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        for registry in [&b, &other_org, &legacy] {
            assert!(
                registry
                    .optimization(name, "scoped-optimization")
                    .await
                    .unwrap()
                    .is_none()
            );
        }
        let reopened = Registry::new(
            Arc::new(FileObjectStore::open(&dir).await.unwrap()),
            RegistryConfig::default(),
        )
        .for_project(a_scope)
        .unwrap();
        assert_eq!(
            reopened.verified_version(name, pin).await.unwrap(),
            Some(original.version.clone())
        );
        assert_eq!(reopened.rebuild(name).await.unwrap().current(), Some(pin));
        assert_eq!(
            reopened
                .optimization(name, "scoped-optimization")
                .await
                .unwrap(),
            Some(optimization)
        );
        let again = reopened
            .publish(request(&original.version.text))
            .await
            .unwrap();
        assert!(!again.created);
        assert_eq!(again.version, original.version);
        assert!(a.for_project(b_scope).is_err());
        assert!(a.for_project(a_scope).is_ok());
        assert!(
            store
                .list(&format!(
                    "prompts/scopes/{}/{}/registry/",
                    a_scope.organization.0, a_scope.project.0
                ))
                .await
                .unwrap()
                .len()
                >= 2
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn scope_rejects_escape_keys_and_unsafe_root_configuration() {
        let root = Registry::new(
            Arc::new(crate::adapters::memory::MemoryObjectStore::new()),
            RegistryConfig::default(),
        );
        let scoped = root.for_project(scope()).unwrap();
        for key in [
            "prompts/shared.prompt/head.json".to_owned(),
            format!("{}/../secret", scoped.config.prefix),
            format!("{}/x\\secret", scoped.config.prefix),
        ] {
            assert!(scoped.read_json::<serde_json::Value>(&key).await.is_err());
            assert!(scoped.write_json(&key, &json!({})).await.is_err());
        }
        assert!(
            scoped
                .optimization(&request("text").name, "../secret")
                .await
                .is_err()
        );
        let unsafe_root = Registry::new(
            root.store.clone(),
            RegistryConfig {
                prefix: "prompts/../escape".into(),
                ..RegistryConfig::default()
            },
        );
        assert!(unsafe_root.for_project(scope()).is_err());
        assert!(root.store.list("").await.unwrap().is_empty());
    }
}
