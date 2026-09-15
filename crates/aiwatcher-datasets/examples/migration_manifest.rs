//! Read-only operator tool over an existing filesystem object-store snapshot.
use aiwatcher_datasets::Registry;
use aiwatcher_iam::{OrganizationId, ProjectId, ProjectScope};
use aiwatcher_prompts::adapters::fs::FileObjectStore;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 5 {
        return Err("usage: migration_manifest STORE_DIRECTORY REGISTRY_PREFIX ORGANIZATION_UUID PROJECT_UUID".into());
    }
    if !std::path::Path::new(&args[1]).is_dir() {
        return Err("STORE_DIRECTORY must be an existing snapshot directory".into());
    }
    let scope = ProjectScope {
        organization: OrganizationId(args[3].parse()?),
        project: ProjectId(args[4].parse()?),
    };
    let registry = Registry::new(Arc::new(FileObjectStore::open(&args[1]).await?), &args[2]);
    let manifest = registry.migration_manifest(scope).await?;
    println!("{}", serde_json::to_string_pretty(&manifest)?);
    Ok(())
}
