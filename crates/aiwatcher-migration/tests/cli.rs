//! The command line, against a real directory on this machine.
//!
//! The store here is a [`FileObjectStore`] over a temporary directory rather
//! than the in-memory one the other suites use: the executor's guarantees are
//! about a store that survives a process ending, and the checkpoint is a file
//! on the same disk.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use aiwatcher_core::prompts::ObjectStore;
use aiwatcher_iam::ProjectScope;
use aiwatcher_prompts::adapters::fs::FileObjectStore;
use serde_json::{Value, json};
use support::scope;

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "aiwatcher-migrate-cli-{}",
            aiwatcher_iam::ProjectId::new().0
        ));
        std::fs::create_dir_all(path.join("store")).unwrap();
        Self(path)
    }
    fn store(&self) -> PathBuf {
        self.0.join("store")
    }
    fn at(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The binary this crate builds, beside the test executable.
fn binary() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("aiwatcher-migrate")
}

struct Output {
    ok: bool,
    stdout: String,
    stderr: String,
}

fn migrate(arguments: &[&str]) -> Output {
    let output = Command::new(binary())
        .args(arguments)
        .output()
        .expect("the migration binary is built beside this test");
    Output {
        ok: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// A fixture admitting one operator on one project. Never an authority: every
/// receipt it admits is marked as a rehearsal.
fn fixture(path: &Path, target: ProjectScope) {
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&json!({
            "admits": [{
                "organization": target.organization.0.to_string(),
                "project": target.project.0.to_string(),
                "provider": "test", "subject": "operator", "role": "admin"
            }]
        }))
        .unwrap(),
    )
    .unwrap();
}

async fn seed(directory: &Path) {
    let store: Arc<dyn ObjectStore> = Arc::new(FileObjectStore::open(directory).await.unwrap());
    let registry = aiwatcher_prompts::Registry::new(
        store.clone(),
        aiwatcher_prompts::RegistryConfig::default(),
    );
    registry
        .publish(
            serde_json::from_value(json!({
                "name": "house.extract", "text": "Read {{ page }}.", "label": "production"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let training = aiwatcher_training::Registry::new(store, "training");
    training
        .start(
            serde_json::from_value(json!({
                "run_id": "run-1", "model": "walls", "dataset": "plans@sha256abc"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn plan_then_apply_then_resume_over_a_real_directory() {
    let scratch = Scratch::new();
    seed(&scratch.store()).await;
    let target = scope();
    let organization = target.organization.0.to_string();
    let project = target.project.0.to_string();
    let store = format!("fs:{}", scratch.store().display());
    let manifest = scratch.at("manifest.json");
    let checkpoint = scratch.at("checkpoint.json");
    let fixture_path = scratch.at("iam.json");
    fixture(&fixture_path, target);
    let iam = format!("fixture:{}", fixture_path.display());

    let planned = migrate(&[
        "plan",
        "--store",
        &store,
        "--org",
        &organization,
        "--project",
        &project,
        "--out",
        manifest.to_str().unwrap(),
    ]);
    assert!(planned.ok, "{}", planned.stderr);
    let document: Value = serde_json::from_str(&planned.stdout).unwrap();
    assert!(document["manifest"]["manifest_id"].is_string());
    assert_eq!(document["survey"]["identical"], 0);
    assert_eq!(document["survey"]["conflicts"].as_array().unwrap().len(), 0);
    assert_eq!(
        document["survey"]["iam"]["checked"], "not_checked",
        "a plan with no --iam confirmed nothing, and says so"
    );
    assert!(
        document["manifest"]["body"]["families"]["conversations"]["state"] == "blocked",
        "the archive is blocked: {}",
        document["manifest"]["body"]["families"]["conversations"]
    );

    // The dry run wrote only the manifest it was asked for.
    assert!(manifest.exists());
    assert!(!checkpoint.exists());

    // Writing needs --confirm.
    let unconfirmed = migrate(&[
        "apply",
        "--store",
        &store,
        "--manifest",
        manifest.to_str().unwrap(),
        "--checkpoint",
        checkpoint.to_str().unwrap(),
        "--iam",
        &iam,
        "--principal",
        "test:operator",
    ]);
    assert!(!unconfirmed.ok);
    assert!(
        unconfirmed.stderr.contains("--confirm"),
        "{}",
        unconfirmed.stderr
    );
    assert!(!checkpoint.exists());

    let applied = migrate(&[
        "apply",
        "--store",
        &store,
        "--manifest",
        manifest.to_str().unwrap(),
        "--checkpoint",
        checkpoint.to_str().unwrap(),
        "--iam",
        &iam,
        "--principal",
        "test:operator",
        "--confirm",
    ]);
    assert!(applied.ok, "{}", applied.stderr);
    let receipt: Value = serde_json::from_str(&applied.stdout).unwrap();
    assert!(receipt["written"].as_u64().unwrap() > 0);
    assert_eq!(receipt["complete"], true);
    assert_eq!(
        receipt["cutover_ready"], false,
        "a fixture admitted it and families with no adapter remain"
    );
    assert_eq!(receipt["iam"]["authoritative"], false);
    assert!(checkpoint.exists());
    assert!(
        applied.stderr.contains("not a cutover"),
        "{}",
        applied.stderr
    );

    // A second `apply` finds the checkpoint and says which word to use.
    let again = migrate(&[
        "apply",
        "--store",
        &store,
        "--manifest",
        manifest.to_str().unwrap(),
        "--checkpoint",
        checkpoint.to_str().unwrap(),
        "--iam",
        &iam,
        "--principal",
        "test:operator",
        "--confirm",
    ]);
    assert!(!again.ok);
    assert!(again.stderr.contains("resume"), "{}", again.stderr);

    let resumed = migrate(&[
        "resume",
        "--store",
        &store,
        "--manifest",
        manifest.to_str().unwrap(),
        "--checkpoint",
        checkpoint.to_str().unwrap(),
        "--iam",
        &iam,
        "--principal",
        "test:operator",
        "--confirm",
    ]);
    assert!(resumed.ok, "{}", resumed.stderr);
    let receipt: Value = serde_json::from_str(&resumed.stdout).unwrap();
    assert_eq!(receipt["written"], 0, "everything was already done");
    assert!(receipt["resumed"].as_u64().unwrap() > 0);

    // And the objects really are on disk, in the project's namespace, reopened
    // by a registry that was never told a migration happened.
    let store_handle: Arc<dyn ObjectStore> =
        Arc::new(FileObjectStore::open(scratch.store()).await.unwrap());
    let prompts = aiwatcher_prompts::Registry::new(
        store_handle,
        aiwatcher_prompts::RegistryConfig::default(),
    )
    .for_project(target)
    .unwrap();
    let head = prompts
        .head(&aiwatcher_core::prompts::PromptName::parse("house.extract").unwrap())
        .await
        .unwrap()
        .expect("the prompt is in the project");
    assert!(head.labels.contains_key("production"));
}

#[tokio::test(flavor = "multi_thread")]
async fn resume_with_no_checkpoint_and_a_store_that_is_not_a_directory_are_refused_by_name() {
    let scratch = Scratch::new();
    seed(&scratch.store()).await;
    let target = scope();
    let store = format!("fs:{}", scratch.store().display());
    let manifest = scratch.at("manifest.json");
    let fixture_path = scratch.at("iam.json");
    fixture(&fixture_path, target);
    let iam = format!("fixture:{}", fixture_path.display());

    let planned = migrate(&[
        "plan",
        "--store",
        &store,
        "--org",
        &target.organization.0.to_string(),
        "--project",
        &target.project.0.to_string(),
        "--out",
        manifest.to_str().unwrap(),
    ]);
    assert!(planned.ok, "{}", planned.stderr);

    let resumed = migrate(&[
        "resume",
        "--store",
        &store,
        "--manifest",
        manifest.to_str().unwrap(),
        "--checkpoint",
        scratch.at("nothing.json").to_str().unwrap(),
        "--iam",
        &iam,
        "--principal",
        "test:operator",
        "--confirm",
    ]);
    assert!(!resumed.ok);
    assert!(resumed.stderr.contains("apply"), "{}", resumed.stderr);

    let missing = migrate(&[
        "plan",
        "--store",
        "fs:/definitely/not/here",
        "--org",
        &target.organization.0.to_string(),
        "--project",
        &target.project.0.to_string(),
    ]);
    assert!(!missing.ok);
    assert!(
        missing.stderr.contains("existing snapshot directory"),
        "{}",
        missing.stderr
    );

    let unknown_store = migrate(&[
        "plan",
        "--store",
        "s3://a-bucket",
        "--org",
        &target.organization.0.to_string(),
        "--project",
        &target.project.0.to_string(),
    ]);
    assert!(!unknown_store.ok);
    assert!(
        unknown_store.stderr.contains("fs:DIRECTORY"),
        "{}",
        unknown_store.stderr
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_edited_manifest_is_refused_at_the_command_line() {
    let scratch = Scratch::new();
    seed(&scratch.store()).await;
    let target = scope();
    let store = format!("fs:{}", scratch.store().display());
    let manifest = scratch.at("manifest.json");
    let fixture_path = scratch.at("iam.json");
    fixture(&fixture_path, target);

    let planned = migrate(&[
        "plan",
        "--store",
        &store,
        "--org",
        &target.organization.0.to_string(),
        "--project",
        &target.project.0.to_string(),
        "--out",
        manifest.to_str().unwrap(),
    ]);
    assert!(planned.ok, "{}", planned.stderr);

    let mut document: Value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
    document["body"]["objects"][0]["target_key"] =
        Value::String("prompts/../escaped.json".to_owned());
    std::fs::write(&manifest, serde_json::to_vec_pretty(&document).unwrap()).unwrap();

    let applied = migrate(&[
        "apply",
        "--store",
        &store,
        "--manifest",
        manifest.to_str().unwrap(),
        "--checkpoint",
        scratch.at("checkpoint.json").to_str().unwrap(),
        "--iam",
        &format!("fixture:{}", fixture_path.display()),
        "--principal",
        "test:operator",
        "--confirm",
    ]);
    assert!(!applied.ok);
    assert!(applied.stderr.contains("re-plan"), "{}", applied.stderr);
    assert!(!scratch.store().join("escaped.json").exists());
    assert!(!scratch.at("checkpoint.json").exists());
}
