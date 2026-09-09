//! Import authored seed data before the API accepts writes.
//!
//! A seed contains ordinary save requests. Validation, revisions and storage
//! are owned by the registry, exactly as for an API save. Existing names belong
//! to the user; a restart or an updated seed never moves their heads backwards.

use std::{collections::BTreeSet, path::Path};

use aiwatcher_datasets::{Registry, SaveBlockTemplateRequest, SavePipelineRequest};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;

const MAX_SEED_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Seed {
    version: u32,
    pipelines: Vec<SavePipelineRequest>,
    #[serde(default)]
    templates: Vec<SaveBlockTemplateRequest>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Imported {
    pub created: usize,
    pub skipped: usize,
    pub templates_created: usize,
    pub templates_skipped: usize,
}

/// An explicit missing or invalid seed is a startup error, not an empty service.
pub async fn import_file(registry: &Registry, path: &Path) -> Result<Imported> {
    let size = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("reading curation seed {}", path.display()))?
        .len();
    ensure!(size <= MAX_SEED_BYTES, "curation seed exceeds 8 MiB");
    let bytes = tokio::fs::read(path).await?;
    import(registry, &bytes)
        .await
        .with_context(|| format!("importing curation seed {}", path.display()))
}

pub async fn import(registry: &Registry, bytes: &[u8]) -> Result<Imported> {
    ensure!(
        bytes.len() as u64 <= MAX_SEED_BYTES,
        "curation seed exceeds 8 MiB"
    );
    let seed: Seed = serde_json::from_slice(bytes).context("parsing curation seed")?;
    ensure!(
        seed.version == 1,
        "unsupported curation seed version {}",
        seed.version
    );
    ensure!(
        seed.pipelines.len() <= 1000,
        "curation seed exceeds 1000 pipelines"
    );

    // Reject the entire document before writing its first pipeline. A store
    // failure can still interrupt an import; the next start skips completed names.
    let mut names = BTreeSet::new();
    for pipeline in &seed.pipelines {
        ensure!(
            names.insert(&pipeline.name),
            "duplicate seed pipeline {}",
            pipeline.name
        );
        pipeline
            .validate()
            .with_context(|| format!("validating {}", pipeline.name))?;
    }
    ensure!(
        seed.templates.len() <= 1000,
        "curation seed exceeds 1000 solutions"
    );
    let mut ids = BTreeSet::new();
    for template in &seed.templates {
        ensure!(
            ids.insert(&template.id),
            "duplicate seed solution {}",
            template.id
        );
        template
            .validate()
            .with_context(|| format!("validating solution {}", template.id))?;
    }
    let mut imported = Imported::default();
    for pipeline in seed.pipelines {
        if registry.pipeline(&pipeline.name, None).await?.is_some() {
            imported.skipped += 1;
            continue;
        }
        registry.save_pipeline(pipeline).await?;
        imported.created += 1;
    }
    for template in seed.templates {
        if registry.block_template(&template.id).await?.is_some() {
            imported.templates_skipped += 1;
            continue;
        }
        registry.save_block_template(template).await?;
        imported.templates_created += 1;
    }
    Ok(imported)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use aiwatcher_prompts::adapters::memory::MemoryObjectStore;
    use std::sync::Arc;

    fn registry() -> Registry {
        Registry::new(Arc::new(MemoryObjectStore::new()), "datasets")
    }

    fn seed() -> Vec<u8> {
        std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/seed.json"))
            .expect("the packaged seed exists")
    }

    #[tokio::test]
    async fn imports_real_examples_then_preserves_authored_edits_and_revisions() {
        let registry = registry();
        let bytes = seed();
        let expected: Seed = serde_json::from_slice(&bytes).unwrap();
        let count = expected.pipelines.len();
        assert!(count > 0);
        assert_eq!(
            import(&registry, &bytes).await.unwrap(),
            Imported {
                created: count,
                skipped: 0,
                templates_created: expected.templates.len(),
                templates_skipped: 0
            }
        );
        let mut edited = expected.pipelines[0].clone();
        edited.description = "User changes must survive a restart".into();
        let saved = registry.save_pipeline(edited).await.unwrap();
        let mut edited_template = expected.templates[0].clone();
        edited_template.description = "An author's library changes".into();
        let saved_template = registry.save_block_template(edited_template).await.unwrap();
        assert_eq!(
            import(&registry, &bytes).await.unwrap(),
            Imported {
                created: 0,
                skipped: count,
                templates_created: 0,
                templates_skipped: expected.templates.len()
            }
        );
        let head = registry
            .pipeline(&saved.pipeline.name, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(head.revision, saved.pipeline.revision);
        assert_eq!(head.saved_at, saved.pipeline.saved_at);
        let head_template = registry
            .block_template(&saved_template.definition.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(head_template.revision, saved_template.revision);
        assert_eq!(head_template.saved_at, saved_template.saved_at);
        assert_eq!(registry.pipelines().await.unwrap().pipelines.len(), count);
        // An API save of the same authored request has the very same revision.
        let pipeline = expected.pipelines.last().unwrap().clone();
        assert!(!registry.save_pipeline(pipeline).await.unwrap().created);
    }

    #[tokio::test]
    async fn validates_every_entry_before_writing_and_rejects_duplicates_and_versions() {
        for fault in ["chain", "duplicate", "version", "name", "template"] {
            let registry = registry();
            let mut seed: serde_json::Value = serde_json::from_slice(&seed()).unwrap();
            match fault {
                "chain" => seed["pipelines"][1]["edges"] = serde_json::json!([]),
                "name" => seed["pipelines"][1]["name"] = serde_json::json!("../invalid"),
                "duplicate" => seed["pipelines"][1] = seed["pipelines"][0].clone(),
                "template" => seed["templates"][0]["title"] = serde_json::json!(""),
                _ => seed["version"] = serde_json::json!(2),
            }
            assert!(
                import(&registry, &serde_json::to_vec(&seed).unwrap())
                    .await
                    .is_err(),
                "{fault}"
            );
            assert!(
                registry.pipelines().await.unwrap().pipelines.is_empty(),
                "{fault}"
            );
        }
    }

    #[tokio::test]
    async fn a_missing_seed_is_an_actionable_error() {
        let error = import_file(&registry(), Path::new("/missing-curation-seed.json"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("missing-curation-seed.json"));
    }

    #[tokio::test]
    async fn building_the_service_imports_the_configured_seed_without_a_notebook_runtime() {
        let data_dir = std::env::temp_dir().join(format!(
            "aiwatcher-seed-test-{}-{}",
            std::process::id(),
            time::OffsetDateTime::now_utc().unix_timestamp_nanos()
        ));
        let config = crate::Config {
            data_dir: data_dir.to_string_lossy().into_owned(),
            seed_file: Some(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../examples/seed.json")
                    .to_string_lossy()
                    .into_owned(),
            ),
            prompt_store: crate::config::PromptStoreKind::Memory,
            workflow_store: crate::config::WorkflowStoreKind::Memory,
            ..crate::Config::default()
        };
        let runtime = crate::build(config).await.unwrap();
        let registry = runtime.state.datasets.as_ref().unwrap();
        let expected: Seed = serde_json::from_slice(&seed()).unwrap();
        assert_eq!(
            registry.pipelines().await.unwrap().pipelines.len(),
            expected.pipelines.len()
        );
        assert_eq!(
            registry.block_templates("", 0, 100).await.unwrap().total,
            expected.templates.len()
        );
        drop(runtime);
        tokio::fs::remove_dir_all(data_dir).await.unwrap();
    }
}
