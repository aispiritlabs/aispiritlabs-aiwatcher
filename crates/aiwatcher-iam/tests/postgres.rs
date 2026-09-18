#![cfg(feature = "postgres")]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use aiwatcher_iam::{postgres::PostgresIamStore, *};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{sync::Arc, time::Duration};
mod support;
use support::*;

async fn open(clock: Arc<TestClock>) -> (PostgresIamStore, PgPool) {
    let url = std::env::var("AIWATCHER_IAM_TEST_POSTGRES_URL")
        .expect("set AIWATCHER_IAM_TEST_POSTGRES_URL to a disposable test database");
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("test PostgreSQL");
    let store = PostgresIamStore::from_pool(pool.clone(), clock)
        .await
        .expect("IAM schema");
    (store, pool)
}

macro_rules! contract {
    ($($name:ident),+ $(,)?) => { $(
        #[tokio::test]
        #[ignore = "needs a disposable PostgreSQL via AIWATCHER_IAM_TEST_POSTGRES_URL"]
        async fn $name() {
            let clock = Arc::new(TestClock::default());
            let (store, _) = open(clock.clone()).await;
            support::$name(&store, &clock).await;
        }
    )+ };
}
contract!(
    audit_is_atomic_paginated_and_administrator_only,
    provider_subject_boundary,
    membership_is_not_project_access,
    a_roster_answers_administrators_and_a_project_s_grants_answer_its_admin,
    cross_organization_references_are_refused,
    timed_and_permanent_grants_are_unioned,
    timed_access_expires_without_a_new_session,
    revocation_cannot_be_undone_by_rejoining,
    role_administration_and_last_owner,
    invalid_commands_have_no_effect,
);

#[tokio::test]
#[ignore = "needs a disposable PostgreSQL via AIWATCHER_IAM_TEST_POSTGRES_URL"]
async fn migrations_and_metadata_survive_reconnection() {
    let clock = Arc::new(TestClock::default());
    let (store, pool) = open(clock.clone()).await;
    let owner = user("owner");
    let org = store
        .create_organization(&owner, "Persistent")
        .await
        .unwrap();
    let p = project(&store, org.id, &owner).await;
    drop(store);
    pool.close().await;
    let (reopened, pool) = open(clock).await;
    assert_eq!(reopened.organizations(&owner).await.unwrap(), vec![org]);
    assert_eq!(
        reopened.access(p.scope, &owner).await.unwrap().role,
        ProjectRole::Admin
    );
    let migrations: i64 = sqlx::query_scalar("SELECT count(*) FROM iam_schema_migrations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(migrations, 2);
}

#[tokio::test]
#[ignore = "needs a disposable PostgreSQL via AIWATCHER_IAM_TEST_POSTGRES_URL"]
async fn concurrent_owner_removals_cannot_remove_the_last_owner() {
    let clock = Arc::new(TestClock::default());
    let ((a, _), (b, _)) = tokio::join!(open(clock.clone()), open(clock));
    let (alice, bob) = (user("alice"), user("bob"));
    let org = a.create_organization(&alice, "Owner race").await.unwrap();
    member(&a, org.id, &alice, &bob, OrganizationRole::Owner).await;
    let (first, second) = tokio::join!(
        a.apply(
            org.id,
            &alice,
            Command::RemoveMember {
                principal: bob.clone()
            }
        ),
        b.apply(
            org.id,
            &bob,
            Command::RemoveMember {
                principal: alice.clone()
            }
        ),
    );
    assert_ne!(
        first.is_ok(),
        second.is_ok(),
        "exactly one owner may remove the other"
    );
    let remaining = if first.is_ok() { &alice } else { &bob };
    assert!(matches!(
        a.apply(
            org.id,
            remaining,
            Command::RemoveMember {
                principal: remaining.clone()
            }
        )
        .await,
        Err(Error::LastOwner)
    ));
    assert_eq!(
        a.organizations(&alice).await.unwrap().len() + b.organizations(&bob).await.unwrap().len(),
        1
    );
}

#[tokio::test]
#[ignore = "needs a disposable PostgreSQL via AIWATCHER_IAM_TEST_POSTGRES_URL"]
async fn concurrent_grant_and_member_removal_cannot_leave_a_revived_grant() {
    let clock = Arc::new(TestClock::default());
    let ((a, _), (b, _)) = tokio::join!(open(clock.clone()), open(clock));
    let (owner, learner) = (user("owner"), user("learner"));
    let org = a.create_organization(&owner, "Removal race").await.unwrap();
    member(&a, org.id, &owner, &learner, OrganizationRole::Member).await;
    let p = project(&a, org.id, &owner).await;
    let (created, removed) = tokio::join!(
        a.apply(
            org.id,
            &owner,
            Command::Grant {
                project: p.scope.project,
                grantee: Grantee::User(learner.clone()),
                role: ProjectRole::Admin,
                window: GrantWindow::permanent(1_000)
            }
        ),
        b.apply(
            org.id,
            &owner,
            Command::RemoveMember {
                principal: learner.clone()
            }
        ),
    );
    removed.unwrap();
    assert!(created.is_ok() || matches!(created, Err(Error::NotFound)));
    member(&a, org.id, &owner, &learner, OrganizationRole::Member).await;
    assert!(matches!(
        b.access(p.scope, &learner).await,
        Err(Error::NotFound)
    ));
}

#[tokio::test]
#[ignore = "needs a disposable PostgreSQL via AIWATCHER_IAM_TEST_POSTGRES_URL"]
async fn a_grant_that_expires_while_waiting_for_the_lock_cannot_authorize_a_write() {
    let clock = Arc::new(TestClock::default());
    let (store, pool) = open(clock.clone()).await;
    let (second, _) = open(clock.clone()).await;
    let (owner, admin) = (user("owner"), user("temporary-admin"));
    let org = store
        .create_organization(&owner, "Time at commit")
        .await
        .unwrap();
    member(&store, org.id, &owner, &admin, OrganizationRole::Member).await;
    let p = project(&store, org.id, &owner).await;
    grant(
        &store,
        &p,
        &owner,
        Grantee::User(admin.clone()),
        ProjectRole::Admin,
        GrantWindow {
            valid_from: 1_000,
            edit_until: Some(1_010),
            read_until: None,
        },
    )
    .await;
    let mut locked = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM iam_organizations WHERE id = $1 FOR UPDATE")
        .bind(org.id.0)
        .fetch_one(&mut *locked)
        .await
        .unwrap();
    let command = second.apply(
        org.id,
        &admin,
        Command::Grant {
            project: p.scope.project,
            grantee: Grantee::User(admin.clone()),
            role: ProjectRole::Admin,
            window: GrantWindow::permanent(1_000),
        },
    );
    tokio::pin!(command);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), command.as_mut())
            .await
            .is_err(),
        "write must wait for the row lock"
    );
    clock.set(1_010);
    locked.commit().await.unwrap();
    assert!(matches!(command.await, Err(Error::Forbidden)));
    let access = store.access(p.scope, &admin).await.unwrap();
    assert_eq!(access.role, ProjectRole::Viewer);
    assert_eq!(
        access.grants.len(),
        1,
        "no new permanent grant was committed"
    );
}

#[tokio::test]
#[ignore = "needs a disposable PostgreSQL via AIWATCHER_IAM_TEST_POSTGRES_URL"]
async fn unknown_or_inconsistent_documents_fail_closed_and_are_not_overwritten() {
    let clock = Arc::new(TestClock::default());
    let (store, pool) = open(clock).await;
    let owner = user("owner");
    let org = store
        .create_organization(&owner, "Future format")
        .await
        .unwrap();
    let p = project(&store, org.id, &owner).await;
    let original: serde_json::Value =
        sqlx::query_scalar("SELECT document FROM iam_organizations WHERE id = $1")
            .bind(org.id.0)
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut future = original.clone();
    future["schema_version"] = serde_json::json!(99);
    let mut cross_scope = original;
    cross_scope["grants"][0]["scope"]["organization"] = serde_json::json!(OrganizationId::new());
    for document in [future, cross_scope] {
        sqlx::query("UPDATE iam_organizations SET document = $2 WHERE id = $1")
            .bind(org.id.0)
            .bind(&document)
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            store.access(p.scope, &owner).await,
            Err(Error::Incompatible(_))
        ));
        assert!(matches!(
            store.organizations(&owner).await,
            Err(Error::Incompatible(_))
        ));
        assert!(matches!(
            store
                .apply(
                    org.id,
                    &owner,
                    Command::CreateTeam {
                        name: "No overwrite".into()
                    }
                )
                .await,
            Err(Error::Incompatible(_))
        ));
        let retained: serde_json::Value =
            sqlx::query_scalar("SELECT document FROM iam_organizations WHERE id = $1")
                .bind(org.id.0)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(retained, document);
    }
}

#[tokio::test]
#[ignore = "needs a disposable PostgreSQL via AIWATCHER_IAM_TEST_POSTGRES_URL"]
async fn audit_failure_rolls_back_both_creation_and_mutation() {
    let (store, pool) = open(Arc::new(TestClock::default())).await;
    let owner = user("audit-rollback");
    let org = store.create_organization(&owner, "Before").await.unwrap();
    let suffix = uuid::Uuid::now_v7().simple().to_string();
    let name = format!("reject-audit-{suffix}");
    let constraint = format!("test_audit_{suffix}");
    // Reject only this test's entries; other parallel tests can continue.
    sqlx::raw_sql(&format!("ALTER TABLE iam_audit ADD CONSTRAINT {constraint} CHECK (organization_id <> '{}' AND entry #>> '{{action,organization,name}}' IS DISTINCT FROM '{name}') NOT VALID", org.id.0))
        .execute(&pool).await.unwrap();
    let creation = store.create_organization(&owner, &name).await;
    let mutation = store
        .apply(
            org.id,
            &owner,
            Command::CreateProject {
                name: "must rollback".into(),
            },
        )
        .await;
    sqlx::raw_sql(&format!(
        "ALTER TABLE iam_audit DROP CONSTRAINT {constraint}"
    ))
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(creation, Err(Error::Backend(_))));
    assert!(matches!(mutation, Err(Error::Backend(_))));
    assert_eq!(
        store.organizations(&owner).await.unwrap(),
        vec![org.clone()]
    );
    assert!(store.projects(org.id, &owner).await.unwrap().is_empty());
    assert_eq!(store.audit(org.id, &owner, 0, 100).await.unwrap().len(), 1);
    drop(store);
    let reopened = PostgresIamStore::from_pool(pool, Arc::new(TestClock::default()))
        .await
        .unwrap();
    assert_eq!(
        reopened.audit(org.id, &owner, 0, 100).await.unwrap().len(),
        1
    );
}

#[tokio::test]
#[ignore = "needs a disposable PostgreSQL via AIWATCHER_IAM_TEST_POSTGRES_URL"]
async fn concurrent_changes_have_ordered_complete_audit_entries() {
    let clock = Arc::new(TestClock::default());
    let ((a, _), (b, _)) = tokio::join!(open(clock.clone()), open(clock));
    let owner = user("audit-concurrent");
    let org = a
        .create_organization(&owner, "Concurrent audit")
        .await
        .unwrap();
    let (first, second) = tokio::join!(
        a.apply(org.id, &owner, Command::CreateProject { name: "A".into() }),
        b.apply(org.id, &owner, Command::CreateProject { name: "B".into() })
    );
    first.unwrap();
    second.unwrap();
    let entries = a.audit(org.id, &owner, 0, 100).await.unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.sequence)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    let projects = a.projects(org.id, &owner).await.unwrap();
    for entry in &entries[1..] {
        let AuditAction::CommandApplied {
            change: Change::ProjectCreated(project),
            ..
        } = &entry.action
        else {
            panic!("project change")
        };
        assert!(projects.iter().any(|access| access.project == *project));
    }
}

#[tokio::test]
#[ignore = "needs a disposable PostgreSQL via AIWATCHER_IAM_TEST_POSTGRES_URL"]
async fn version_one_upgrades_without_inventing_historical_audit() {
    let url = std::env::var("AIWATCHER_IAM_TEST_POSTGRES_URL").unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let schema = format!("iam_upgrade_{}", uuid::Uuid::now_v7().simple());
    sqlx::raw_sql(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let search_path = format!("SET search_path TO {schema}");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .after_connect(move |connection, _| {
            let statement = search_path.clone();
            Box::pin(async move {
                sqlx::query(&statement).execute(connection).await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../migrations/0001_organizations.sql"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE TABLE iam_schema_migrations (version bigint PRIMARY KEY); INSERT INTO iam_schema_migrations VALUES (1)")
        .execute(&pool).await.unwrap();
    let owner = user("pre-audit-owner");
    let id = OrganizationId::new();
    let document = serde_json::json!({"schema_version":1, "organization":{"id":id,"name":"Existing"},
        "members":[{"principal":owner,"role":"owner"}],"teams":[],"projects":[],"grants":[]});
    sqlx::query("INSERT INTO iam_organizations (id, document) VALUES ($1,$2)")
        .bind(id.0)
        .bind(&document)
        .execute(&pool)
        .await
        .unwrap();
    let store = PostgresIamStore::from_pool(pool.clone(), Arc::new(TestClock::default()))
        .await
        .unwrap();
    assert!(store.audit(id, &owner, 0, 100).await.unwrap().is_empty());
    let preserved: serde_json::Value =
        sqlx::query_scalar("SELECT document FROM iam_organizations WHERE id=$1")
            .bind(id.0)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(preserved, document);
    store
        .apply(
            id,
            &owner,
            Command::CreateTeam {
                name: "After migration".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(store.audit(id, &owner, 0, 100).await.unwrap().len(), 1);
    drop(store);
    pool.close().await;
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
}
