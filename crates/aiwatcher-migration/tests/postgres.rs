//! The authority the CLI's `--iam postgres:` reaches, against a real database.
//!
//! Opt-in and named, the way this workspace's other database suites are:
//!
//! ```sh
//! AIWATCHER_MIGRATION_TEST_POSTGRES_URL=postgres://… \
//!   cargo test -p aiwatcher-migration --features postgres --test postgres -- --ignored
//! ```
//!
//! Ignored rather than skipped, so a run with no database says it did not run
//! this instead of reporting it as passed. It creates its own organization and
//! project in a disposable database; it is never pointed at one a deployment
//! uses.
#![cfg(feature = "postgres")]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::sync::Arc;

use aiwatcher_core::prompts::ObjectStore;
use aiwatcher_iam::postgres::PostgresIamStore;
use aiwatcher_iam::{Change, Command, IamStore, Principal, ProjectRole, ProjectScope};
use aiwatcher_migration::MigrationError;
use aiwatcher_migration::authority::{IamAuthority, TargetAuthority};
use aiwatcher_migration::execute::{Destination, Options, execute};
use aiwatcher_migration::manifest::IamCheck;
use aiwatcher_migration::plan::{Source, plan};
use support::seeded;

fn url() -> String {
    std::env::var("AIWATCHER_MIGRATION_TEST_POSTGRES_URL")
        .expect("set AIWATCHER_MIGRATION_TEST_POSTGRES_URL to a disposable database")
}

#[tokio::test]
#[ignore = "needs a disposable PostgreSQL; see this file's header"]
async fn the_deployments_own_iam_admits_a_real_project_and_refuses_everything_else() {
    let iam = Arc::new(PostgresIamStore::connect(&url(), 4).await.unwrap());
    let operator = Principal::new(
        "oidc",
        format!("operator-{}", aiwatcher_iam::ProjectId::new().0),
    )
    .unwrap();
    let organization = iam
        .create_organization(&operator, "Stream D migration test")
        .await
        .unwrap();
    let Change::ProjectCreated(project) = iam
        .apply(
            organization.id,
            &operator,
            Command::CreateProject {
                name: "plans".to_owned(),
            },
        )
        .await
        .unwrap()
    else {
        panic!("creating a project answers with the project");
    };

    let store: Arc<dyn IamStore> = iam.clone();
    let authority = IamAuthority::new(store, operator.clone(), "postgres", true);
    let IamCheck::Verified {
        authoritative,
        role,
        authority: named,
        ..
    } = authority.admit(project.scope).await.unwrap()
    else {
        panic!("a live admin grant is admitted");
    };
    assert!(
        authoritative,
        "this is the store a deployment authorizes from"
    );
    assert_eq!(named, "postgres");
    assert_eq!(role, ProjectRole::Admin);

    // A project id nobody created, in an organization that exists.
    let nowhere = authority
        .admit(ProjectScope {
            organization: organization.id,
            project: aiwatcher_iam::ProjectId::new(),
        })
        .await
        .expect_err("there is no such project");
    assert!(matches!(nowhere, MigrationError::Authority(_)));

    // An organization nobody created.
    let elsewhere = authority
        .admit(ProjectScope {
            organization: aiwatcher_iam::OrganizationId::new(),
            project: project.scope.project,
        })
        .await
        .expect_err("there is no such organization");
    assert!(matches!(elsewhere, MigrationError::Authority(_)));

    // A real copy, admitted by the real control plane: the one run in this
    // suite that is a rehearsal of a cutover rather than of a refusal.
    let memory = seeded().await;
    let objects: Arc<dyn ObjectStore> = memory.clone();
    let directory = std::env::temp_dir().join(format!(
        "aiwatcher-migration-postgres-{}",
        aiwatcher_iam::ProjectId::new().0
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let manifest = plan(&objects, &Source::new("fs:/snapshot"), project.scope)
        .await
        .unwrap();
    let receipt = execute(
        &objects,
        &manifest,
        &Destination {
            label: "fs:/snapshot".to_owned(),
            checkpoint: directory.join("checkpoint.json"),
        },
        &authority,
        Options::default(),
    )
    .await
    .unwrap();
    assert!(receipt.complete);
    assert!(matches!(
        receipt.iam,
        IamCheck::Verified {
            authoritative: true,
            ..
        }
    ));
    assert!(
        !receipt.cutover_ready,
        "the families with no adapter still stand between this and a cutover"
    );
    std::fs::remove_dir_all(&directory).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a disposable PostgreSQL; see this file's header"]
async fn the_command_line_reaches_that_same_store_and_writes_a_cutover_grade_receipt() {
    let iam = PostgresIamStore::connect(&url(), 4).await.unwrap();
    let operator = Principal::new(
        "oidc",
        format!("cli-operator-{}", aiwatcher_iam::ProjectId::new().0),
    )
    .unwrap();
    let organization = iam
        .create_organization(&operator, "Stream D command line test")
        .await
        .unwrap();
    let Change::ProjectCreated(project) = iam
        .apply(
            organization.id,
            &operator,
            Command::CreateProject {
                name: "plans".to_owned(),
            },
        )
        .await
        .unwrap()
    else {
        panic!("creating a project answers with the project");
    };

    let root = std::env::temp_dir().join(format!(
        "aiwatcher-migrate-postgres-cli-{}",
        aiwatcher_iam::ProjectId::new().0
    ));
    let snapshot = root.join("store");
    std::fs::create_dir_all(&snapshot).unwrap();
    let objects: Arc<dyn ObjectStore> = Arc::new(
        aiwatcher_prompts::adapters::fs::FileObjectStore::open(&snapshot)
            .await
            .unwrap(),
    );
    aiwatcher_prompts::Registry::new(
        objects.clone(),
        aiwatcher_prompts::RegistryConfig::default(),
    )
    .publish(
        serde_json::from_value(serde_json::json!({
            "name": "house.extract", "text": "Read {{ page }}.", "label": "production"
        }))
        .unwrap(),
    )
    .await
    .unwrap();

    let binary = {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        if path.ends_with("deps") {
            path.pop();
        }
        path.join("aiwatcher-migrate")
    };
    let store = format!("fs:{}", snapshot.display());
    let manifest = root.join("manifest.json");
    let run = |arguments: Vec<String>| {
        let output = std::process::Command::new(&binary)
            .args(&arguments)
            .output()
            .unwrap();
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    };
    let owned = |values: &[&str]| values.iter().map(ToString::to_string).collect::<Vec<_>>();

    let (ok, _, stderr) = run(owned(&[
        "plan",
        "--store",
        &store,
        "--org",
        &organization.id.0.to_string(),
        "--project",
        &project.scope.project.0.to_string(),
        "--out",
        manifest.to_str().unwrap(),
    ]));
    assert!(ok, "{stderr}");

    let (ok, stdout, stderr) = run(owned(&[
        "apply",
        "--store",
        &store,
        "--manifest",
        manifest.to_str().unwrap(),
        "--checkpoint",
        root.join("checkpoint.json").to_str().unwrap(),
        "--iam",
        &url(),
        "--principal",
        &format!("{}:{}", operator.provider, operator.subject),
        "--confirm",
    ]));
    assert!(ok, "{stderr}");
    let receipt: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(receipt["iam"]["authority"], "postgres");
    assert_eq!(receipt["iam"]["authoritative"], true);
    assert_eq!(receipt["iam"]["role"], "admin");
    assert_eq!(receipt["complete"], true);
    assert!(receipt["written"].as_u64().unwrap() > 0);

    // A principal the control plane does not admit for this project is refused
    // by the command line too, and writes nothing.
    let (ok, _, stderr) = run(owned(&[
        "apply",
        "--store",
        &store,
        "--manifest",
        manifest.to_str().unwrap(),
        "--checkpoint",
        root.join("stranger.json").to_str().unwrap(),
        "--iam",
        &url(),
        "--principal",
        "oidc:somebody-else",
        "--confirm",
    ]));
    assert!(!ok);
    assert!(stderr.contains("no live grant"), "{stderr}");
    assert!(!root.join("stranger.json").exists());

    std::fs::remove_dir_all(&root).unwrap();
}
