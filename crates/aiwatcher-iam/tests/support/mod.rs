#![allow(dead_code, clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use aiwatcher_iam::*;
use std::sync::atomic::{AtomicI64, Ordering};
use uuid::Uuid;

#[derive(Debug)]
pub struct TestClock(pub AtomicI64);
impl Default for TestClock {
    fn default() -> Self {
        Self(AtomicI64::new(1_000))
    }
}
impl Clock for TestClock {
    fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}
impl TestClock {
    pub fn set(&self, now: i64) {
        self.0.store(now, Ordering::SeqCst);
    }
}

pub fn user(label: &str) -> Principal {
    Principal::new(
        "https://identity.test/oidc",
        format!("{label}-{}", Uuid::now_v7()),
    )
    .unwrap()
}
pub async fn member(
    store: &dyn IamStore,
    org: OrganizationId,
    owner: &Principal,
    user: &Principal,
    role: OrganizationRole,
) {
    store
        .apply(
            org,
            owner,
            Command::SetMember {
                principal: user.clone(),
                role,
            },
        )
        .await
        .unwrap();
}
pub async fn project(store: &dyn IamStore, org: OrganizationId, owner: &Principal) -> Project {
    match store
        .apply(
            org,
            owner,
            Command::CreateProject {
                name: "Project".into(),
            },
        )
        .await
        .unwrap()
    {
        Change::ProjectCreated(project) => project,
        other => panic!("unexpected {other:?}"),
    }
}
pub async fn team(store: &dyn IamStore, org: OrganizationId, owner: &Principal) -> Team {
    match store
        .apply(
            org,
            owner,
            Command::CreateTeam {
                name: "Team".into(),
            },
        )
        .await
        .unwrap()
    {
        Change::TeamCreated(team) => team,
        other => panic!("unexpected {other:?}"),
    }
}
pub async fn grant(
    store: &dyn IamStore,
    project: &Project,
    actor: &Principal,
    grantee: Grantee,
    role: ProjectRole,
    window: GrantWindow,
) -> Grant {
    match store
        .apply(
            project.scope.organization,
            actor,
            Command::Grant {
                project: project.scope.project,
                grantee,
                role,
                window,
            },
        )
        .await
        .unwrap()
    {
        Change::GrantCreated(grant) => grant,
        other => panic!("unexpected {other:?}"),
    }
}
pub async fn join_team(
    store: &dyn IamStore,
    team: &Team,
    actor: &Principal,
    user: &Principal,
    present: bool,
) {
    store
        .apply(
            team.organization,
            actor,
            Command::SetTeamMember {
                team: team.id,
                principal: user.clone(),
                present,
            },
        )
        .await
        .unwrap();
}

pub async fn provider_subject_boundary(store: &dyn IamStore, _: &TestClock) {
    let alice = user("alice");
    let impostor = Principal::new("https://another-identity.test/oidc", &alice.subject).unwrap();
    let org = store
        .create_organization(&alice, "  Organization  ")
        .await
        .unwrap();
    assert_eq!(org.name, "Organization");
    let p = project(store, org.id, &alice).await;
    assert_eq!(
        store.organizations(&alice).await.unwrap(),
        vec![org.clone()]
    );
    assert!(store.organizations(&impostor).await.unwrap().is_empty());
    assert!(matches!(
        store.projects(org.id, &impostor).await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        store.access(p.scope, &impostor).await,
        Err(Error::NotFound)
    ));
    let changed_case = Principal::new(alice.provider.to_uppercase(), &alice.subject).unwrap();
    assert!(store.organizations(&changed_case).await.unwrap().is_empty());
    let same_identity: Principal =
        serde_json::from_value(serde_json::to_value(&alice).unwrap()).unwrap();
    assert_eq!(
        store.organizations(&same_identity).await.unwrap(),
        vec![org]
    );
}

pub async fn a_roster_answers_administrators_and_a_project_s_grants_answer_its_admin(
    store: &dyn IamStore,
    clock: &TestClock,
) {
    let (alice, bob, carol, dave) = (
        user("owner"),
        user("admin"),
        user("member"),
        user("stranger"),
    );
    let org = store.create_organization(&alice, "Roster").await.unwrap();
    member(store, org.id, &alice, &bob, OrganizationRole::Admin).await;
    member(store, org.id, &alice, &carol, OrganizationRole::Member).await;
    let group = team(store, org.id, &alice).await;
    join_team(store, &group, &alice, &carol, true).await;
    let lesson = project(store, org.id, &alice).await;
    let other = project(store, org.id, &bob).await;

    // An administrator sees everybody and every project, including the ones no
    // grant of theirs reaches — which is the reason this read exists.
    let roster = store.roster(org.id, &bob).await.unwrap();
    assert_eq!(roster.organization, org);
    assert_eq!(roster.members.len(), 3);
    assert_eq!(roster.projects.len(), 2);
    assert!(store.projects(org.id, &bob).await.unwrap().len() < roster.projects.len());
    assert_eq!(roster.teams.len(), 1);
    assert_eq!(roster.teams[0].members, vec![carol.clone()]);
    assert_eq!(
        roster
            .members
            .iter()
            .find(|entry| entry.principal == carol)
            .map(|entry| entry.role),
        Some(OrganizationRole::Member),
    );

    // A member is not an administrator, and a stranger is not even that.
    assert!(matches!(
        store.roster(org.id, &carol).await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        store.roster(org.id, &dave).await,
        Err(Error::NotFound)
    ));

    // A project's grants answer whoever may issue one there: its own admin, or
    // an administrator of the organization. Carol holds a lapsed grant and a
    // live one, and both come back — a list filtered by the clock would hide
    // the row somebody opened this to find.
    clock.set(2_000);
    grant(
        store,
        &lesson,
        &alice,
        Grantee::User(carol.clone()),
        ProjectRole::Editor,
        GrantWindow {
            valid_from: 1_000,
            edit_until: Some(1_500),
            read_until: Some(1_600),
        },
    )
    .await;
    grant(
        store,
        &lesson,
        &alice,
        Grantee::Team(group.id),
        ProjectRole::Viewer,
        GrantWindow::permanent(1_000),
    )
    .await;
    let grants = store.project_grants(lesson.scope, &alice).await.unwrap();
    assert_eq!(
        grants.len(),
        3,
        "the creator's own grant, and the two above"
    );
    assert!(grants.iter().all(|grant| grant.scope == lesson.scope));
    assert_eq!(
        store
            .project_grants(lesson.scope, &bob)
            .await
            .unwrap()
            .len(),
        3,
        "an organization admin may read a project no grant of theirs reaches",
    );
    assert!(matches!(
        store.project_grants(lesson.scope, &carol).await,
        Err(Error::Forbidden),
    ));
    assert!(matches!(
        store.project_grants(lesson.scope, &dave).await,
        Err(Error::NotFound)
    ));

    // The scope is the pair, so a project id from this organization pointed at
    // another one is absent rather than readable.
    let elsewhere = store.create_organization(&dave, "Elsewhere").await.unwrap();
    assert!(matches!(
        store
            .project_grants(
                ProjectScope {
                    organization: elsewhere.id,
                    project: other.scope.project,
                },
                &dave,
            )
            .await,
        Err(Error::NotFound),
    ));
}

pub async fn an_invitation_is_one_use_expiring_and_learns_who_took_it(
    store: &dyn IamStore,
    clock: &TestClock,
) {
    let (alice, stranger, other) = (user("owner"), user("stranger"), user("someone-else"));
    let org = store.create_organization(&alice, "Workshop").await.unwrap();
    let lesson = project(store, org.id, &alice).await;

    let issued = store
        .invite(
            lesson.scope,
            &alice,
            InvitationOffer {
                role: ProjectRole::Editor,
                window: GrantWindow {
                    valid_from: 1_000,
                    edit_until: Some(3_000),
                    read_until: None,
                },
                expires_at: 2_000,
                label: Some("somebody@example.test".into()),
            },
        )
        .await
        .unwrap();
    // The one moment the token exists, and it is not in the record.
    assert_eq!(issued.token.len(), 64);
    assert!(issued.invitation.redeemed.is_none());
    let listed = store.invitations(org.id, &alice).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(
        !serde_json::to_string(&listed)
            .unwrap()
            .contains(&issued.token),
        "a stored invitation must not carry its own token",
    );

    // A stranger is not a member and reaches nothing — until they redeem.
    assert!(matches!(
        store.access(lesson.scope, &stranger).await,
        Err(Error::NotFound)
    ));
    let redeemed = store.redeem(&issued.token, &stranger).await.unwrap();
    assert_eq!(redeemed.project, lesson);
    assert_eq!(redeemed.role, ProjectRole::Editor);
    let access = store.access(lesson.scope, &stranger).await.unwrap();
    assert_eq!(access.role, ProjectRole::Editor);
    assert_eq!(access.grants.len(), 1);
    assert_eq!(access.grants[0].grant.id, redeemed.grant);

    // Once, by anybody. The second attempt is refused whoever makes it, and
    // the first redeemer is remembered by the pair their session proved.
    assert!(matches!(
        store.redeem(&issued.token, &stranger).await,
        Err(Error::Redeemed)
    ));
    assert!(matches!(
        store.redeem(&issued.token, &other).await,
        Err(Error::Redeemed)
    ));
    let spent = store.invitations(org.id, &alice).await.unwrap();
    assert_eq!(
        spent[0].redeemed.as_ref().map(|entry| &entry.principal),
        Some(&stranger),
    );
    // Withdrawing a spent offer is refused rather than silently undoing it:
    // what it produced is a grant, and that is revoked on its own.
    assert!(matches!(
        store
            .revoke_invitation(org.id, &alice, issued.invitation.id)
            .await,
        Err(Error::Redeemed),
    ));

    // An unknown token is absent, and an expired one says so — the holder is
    // the only person who can ask, and "it lapsed" is what they need.
    assert!(matches!(
        store.redeem("f".repeat(64).as_str(), &other).await,
        Err(Error::NotFound)
    ));
    let lapsing = store
        .invite(
            lesson.scope,
            &alice,
            InvitationOffer {
                role: ProjectRole::Viewer,
                window: GrantWindow::permanent(1_000),
                expires_at: 1_500,
                label: None,
            },
        )
        .await
        .unwrap();
    clock.set(1_600);
    assert!(matches!(
        store.redeem(&lapsing.token, &other).await,
        Err(Error::Expired)
    ));
    // …and an offer nobody took can still be withdrawn.
    store
        .revoke_invitation(org.id, &alice, lapsing.invitation.id)
        .await
        .unwrap();

    // Only the authority that may grant here may offer one, and a member who
    // may not administer the project sees none of its offers.
    assert!(matches!(
        store
            .invite(
                lesson.scope,
                &stranger,
                InvitationOffer {
                    role: ProjectRole::Admin,
                    window: GrantWindow::permanent(1_600),
                    expires_at: 9_000,
                    label: None,
                },
            )
            .await,
        Err(Error::Forbidden),
    ));
    assert!(
        store
            .invitations(org.id, &stranger)
            .await
            .unwrap()
            .is_empty()
    );
}

pub async fn membership_is_not_project_access(store: &dyn IamStore, _: &TestClock) {
    let (alice, bob, carol) = (user("owner"), user("second-owner"), user("admin"));
    let org = store
        .create_organization(&alice, "No implicit grants")
        .await
        .unwrap();
    member(store, org.id, &alice, &bob, OrganizationRole::Owner).await;
    member(store, org.id, &alice, &carol, OrganizationRole::Admin).await;
    let a = project(store, org.id, &alice).await;
    let b = project(store, org.id, &bob).await;
    assert!(matches!(
        store.access(a.scope, &bob).await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        store.access(b.scope, &alice).await,
        Err(Error::NotFound)
    ));
    assert!(store.projects(org.id, &carol).await.unwrap().is_empty());
    let access = store.projects(org.id, &alice).await.unwrap();
    assert_eq!(access.len(), 1);
    assert_eq!(access[0].project, a);
    assert_eq!(
        access[0].grants.len(),
        1,
        "creator has an explicit removable admin grant"
    );
    assert_eq!(access[0].role, ProjectRole::Admin);
    store
        .apply(
            org.id,
            &alice,
            Command::RevokeGrant {
                project: a.scope.project,
                grant: access[0].grants[0].grant.id,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        store.access(a.scope, &alice).await,
        Err(Error::NotFound)
    ));
    // Organizational administration can recover an explicit project grant.
    grant(
        store,
        &a,
        &alice,
        Grantee::User(alice.clone()),
        ProjectRole::Viewer,
        GrantWindow::permanent(1_000),
    )
    .await;
    assert_eq!(
        store.access(a.scope, &alice).await.unwrap().role,
        ProjectRole::Viewer
    );
}

pub async fn cross_organization_references_are_refused(store: &dyn IamStore, _: &TestClock) {
    let (alice, bob) = (user("a"), user("b"));
    let a = store.create_organization(&alice, "A").await.unwrap();
    let b = store.create_organization(&bob, "B").await.unwrap();
    let pa = project(store, a.id, &alice).await;
    let pb = project(store, b.id, &bob).await;
    let tb = team(store, b.id, &bob).await;
    assert!(matches!(
        store
            .access(
                ProjectScope {
                    organization: b.id,
                    project: pa.scope.project
                },
                &bob
            )
            .await,
        Err(Error::NotFound)
    ));
    for command in [
        Command::CreateProject {
            name: "intrusion".into(),
        },
        Command::SetMember {
            principal: alice.clone(),
            role: OrganizationRole::Owner,
        },
        Command::SetTeamMember {
            team: tb.id,
            principal: bob.clone(),
            present: false,
        },
    ] {
        assert!(matches!(
            store.apply(b.id, &alice, command).await,
            Err(Error::NotFound)
        ));
    }
    for (project, grantee) in [
        (pa.scope.project, Grantee::Team(tb.id)),
        (pa.scope.project, Grantee::User(bob.clone())),
        (pb.scope.project, Grantee::User(alice.clone())),
    ] {
        assert!(matches!(
            store
                .apply(
                    a.id,
                    &alice,
                    Command::Grant {
                        project,
                        grantee,
                        role: ProjectRole::Admin,
                        window: GrantWindow::permanent(1_000)
                    }
                )
                .await,
            Err(Error::NotFound)
        ));
    }
    assert_eq!(store.projects(a.id, &alice).await.unwrap().len(), 1);
    assert_eq!(store.projects(b.id, &bob).await.unwrap().len(), 1);
}

pub async fn timed_and_permanent_grants_are_unioned(store: &dyn IamStore, clock: &TestClock) {
    let (owner, learner) = (user("owner"), user("learner"));
    let org = store.create_organization(&owner, "Union").await.unwrap();
    member(store, org.id, &owner, &learner, OrganizationRole::Member).await;
    let p = project(store, org.id, &owner).await;
    let t = team(store, org.id, &owner).await;
    join_team(store, &t, &owner, &learner, true).await;
    join_team(store, &t, &owner, &learner, true).await;
    let permanent = grant(
        store,
        &p,
        &owner,
        Grantee::User(learner.clone()),
        ProjectRole::Viewer,
        GrantWindow::permanent(1_000),
    )
    .await;
    let timed = grant(
        store,
        &p,
        &owner,
        Grantee::Team(t.id),
        ProjectRole::Editor,
        GrantWindow {
            valid_from: 1_010,
            edit_until: Some(1_020),
            read_until: Some(1_030),
        },
    )
    .await;
    for (now, role, count) in [
        (1_009, ProjectRole::Viewer, 1),
        (1_010, ProjectRole::Editor, 2),
        (1_019, ProjectRole::Editor, 2),
        (1_020, ProjectRole::Viewer, 2),
        (1_029, ProjectRole::Viewer, 2),
        (1_030, ProjectRole::Viewer, 1),
    ] {
        clock.set(now);
        let access = store.access(p.scope, &learner).await.unwrap();
        assert_eq!(access.role, role, "at {now}");
        assert_eq!(access.grants.len(), count, "at {now}");
        assert_eq!(access.evaluated_at, now);
        assert!(
            access
                .grants
                .iter()
                .any(|entry| entry.grant.id == permanent.id)
        );
    }
    store
        .apply(
            org.id,
            &owner,
            Command::RevokeGrant {
                project: p.scope.project,
                grant: timed.id,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        store.access(p.scope, &learner).await.unwrap().role,
        ProjectRole::Viewer
    );
    store
        .apply(
            org.id,
            &owner,
            Command::RevokeGrant {
                project: p.scope.project,
                grant: permanent.id,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        store.access(p.scope, &learner).await,
        Err(Error::NotFound)
    ));
    assert!(store.projects(org.id, &learner).await.unwrap().is_empty());
}

pub async fn timed_access_expires_without_a_new_session(store: &dyn IamStore, clock: &TestClock) {
    let (owner, learner) = (user("owner"), user("learner"));
    let org = store.create_organization(&owner, "Expiry").await.unwrap();
    member(store, org.id, &owner, &learner, OrganizationRole::Member).await;
    let p = project(store, org.id, &owner).await;
    grant(
        store,
        &p,
        &owner,
        Grantee::User(learner.clone()),
        ProjectRole::Admin,
        GrantWindow {
            valid_from: 1_010,
            edit_until: Some(1_020),
            read_until: Some(1_030),
        },
    )
    .await;
    assert!(matches!(
        store.access(p.scope, &learner).await,
        Err(Error::NotFound)
    ));
    clock.set(1_010);
    store
        .access(p.scope, &learner)
        .await
        .unwrap()
        .require(ProjectRole::Admin)
        .unwrap();
    clock.set(1_020);
    let access = store.access(p.scope, &learner).await.unwrap();
    access.require(ProjectRole::Viewer).unwrap();
    assert!(matches!(
        access.require(ProjectRole::Editor),
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        store
            .apply(
                org.id,
                &learner,
                Command::Grant {
                    project: p.scope.project,
                    grantee: Grantee::User(learner.clone()),
                    role: ProjectRole::Admin,
                    window: GrantWindow::permanent(1_020)
                }
            )
            .await,
        Err(Error::Forbidden)
    ));
    clock.set(1_030);
    assert!(matches!(
        store.access(p.scope, &learner).await,
        Err(Error::NotFound)
    ));
    // Optional read_until really means ongoing read access after edit_until.
    grant(
        store,
        &p,
        &owner,
        Grantee::User(learner.clone()),
        ProjectRole::Editor,
        GrantWindow {
            valid_from: 1_030,
            edit_until: Some(1_040),
            read_until: None,
        },
    )
    .await;
    clock.set(2_000);
    assert_eq!(
        store.access(p.scope, &learner).await.unwrap().role,
        ProjectRole::Viewer
    );
}

pub async fn revocation_cannot_be_undone_by_rejoining(store: &dyn IamStore, _: &TestClock) {
    let (owner, learner) = (user("owner"), user("learner"));
    let org = store
        .create_organization(&owner, "Revocation")
        .await
        .unwrap();
    member(store, org.id, &owner, &learner, OrganizationRole::Member).await;
    let p = project(store, org.id, &owner).await;
    let t = team(store, org.id, &owner).await;
    join_team(store, &t, &owner, &learner, true).await;
    grant(
        store,
        &p,
        &owner,
        Grantee::User(learner.clone()),
        ProjectRole::Viewer,
        GrantWindow::permanent(1_000),
    )
    .await;
    grant(
        store,
        &p,
        &owner,
        Grantee::Team(t.id),
        ProjectRole::Editor,
        GrantWindow::permanent(1_000),
    )
    .await;
    join_team(store, &t, &owner, &learner, false).await;
    assert_eq!(
        store.access(p.scope, &learner).await.unwrap().role,
        ProjectRole::Viewer
    );
    join_team(store, &t, &owner, &learner, true).await;
    store
        .apply(
            org.id,
            &owner,
            Command::RemoveMember {
                principal: learner.clone(),
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        store.access(p.scope, &learner).await,
        Err(Error::NotFound)
    ));
    assert!(store.organizations(&learner).await.unwrap().is_empty());
    member(store, org.id, &owner, &learner, OrganizationRole::Member).await;
    assert!(
        matches!(store.access(p.scope, &learner).await, Err(Error::NotFound)),
        "neither old direct grants nor team memberships revive"
    );
    join_team(store, &t, &owner, &learner, true).await;
    assert_eq!(
        store.access(p.scope, &learner).await.unwrap().role,
        ProjectRole::Editor
    );
    store
        .apply(org.id, &owner, Command::DeleteTeam { team: t.id })
        .await
        .unwrap();
    assert!(matches!(
        store.access(p.scope, &learner).await,
        Err(Error::NotFound)
    ));
}

pub async fn role_administration_and_last_owner(store: &dyn IamStore, _: &TestClock) {
    let (owner, admin, member_user) = (user("owner"), user("admin"), user("member"));
    let org = store.create_organization(&owner, "Roles").await.unwrap();
    member(store, org.id, &owner, &admin, OrganizationRole::Admin).await;
    member(
        store,
        org.id,
        &admin,
        &member_user,
        OrganizationRole::Member,
    )
    .await;
    for command in [
        Command::SetMember {
            principal: admin.clone(),
            role: OrganizationRole::Owner,
        },
        Command::SetMember {
            principal: member_user.clone(),
            role: OrganizationRole::Admin,
        },
        Command::RemoveMember {
            principal: owner.clone(),
        },
    ] {
        assert!(matches!(
            store.apply(org.id, &admin, command).await,
            Err(Error::Forbidden)
        ));
    }
    for command in [
        Command::RemoveMember {
            principal: owner.clone(),
        },
        Command::SetMember {
            principal: owner.clone(),
            role: OrganizationRole::Admin,
        },
    ] {
        assert!(matches!(
            store.apply(org.id, &owner, command).await,
            Err(Error::LastOwner)
        ));
    }
    for command in [
        Command::CreateProject {
            name: "denied".into(),
        },
        Command::CreateTeam {
            name: "denied".into(),
        },
        Command::SetMember {
            principal: member_user.clone(),
            role: OrganizationRole::Owner,
        },
    ] {
        assert!(matches!(
            store.apply(org.id, &member_user, command).await,
            Err(Error::Forbidden)
        ));
    }
    let p = project(store, org.id, &owner).await;
    let g = grant(
        store,
        &p,
        &owner,
        Grantee::User(member_user.clone()),
        ProjectRole::Admin,
        GrantWindow::permanent(1_000),
    )
    .await;
    grant(
        store,
        &p,
        &member_user,
        Grantee::User(admin.clone()),
        ProjectRole::Viewer,
        GrantWindow::permanent(1_000),
    )
    .await;
    store
        .apply(
            org.id,
            &owner,
            Command::RevokeGrant {
                project: p.scope.project,
                grant: g.id,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .apply(
                org.id,
                &member_user,
                Command::Grant {
                    project: p.scope.project,
                    grantee: Grantee::User(member_user.clone()),
                    role: ProjectRole::Admin,
                    window: GrantWindow::permanent(1_000)
                }
            )
            .await,
        Err(Error::NotFound)
    ));
    member(store, org.id, &owner, &admin, OrganizationRole::Owner).await;
    store
        .apply(
            org.id,
            &admin,
            Command::RemoveMember {
                principal: owner.clone(),
            },
        )
        .await
        .unwrap();
    assert!(store.organizations(&owner).await.unwrap().is_empty());
    assert!(matches!(
        store
            .apply(
                org.id,
                &admin,
                Command::RemoveMember {
                    principal: admin.clone()
                }
            )
            .await,
        Err(Error::LastOwner)
    ));
}

pub async fn invalid_commands_have_no_effect(store: &dyn IamStore, _: &TestClock) {
    let owner = user("owner");
    assert!(matches!(
        store.create_organization(&owner, " \n ").await,
        Err(Error::Invalid(_))
    ));
    assert!(store.organizations(&owner).await.unwrap().is_empty());
    let org = store
        .create_organization(&owner, "Validation")
        .await
        .unwrap();
    let p = project(store, org.id, &owner).await;
    for window in [
        GrantWindow {
            valid_from: 1_000,
            edit_until: Some(1_000),
            read_until: None,
        },
        GrantWindow {
            valid_from: 1_000,
            edit_until: None,
            read_until: Some(999),
        },
        GrantWindow {
            valid_from: 1_000,
            edit_until: Some(1_020),
            read_until: Some(1_010),
        },
    ] {
        assert!(matches!(
            store
                .apply(
                    org.id,
                    &owner,
                    Command::Grant {
                        project: p.scope.project,
                        grantee: Grantee::User(owner.clone()),
                        role: ProjectRole::Admin,
                        window
                    }
                )
                .await,
            Err(Error::Invalid(_))
        ));
    }
    assert!(matches!(
        store
            .apply(
                org.id,
                &owner,
                Command::SetMember {
                    principal: Principal {
                        provider: "".into(),
                        subject: "bad".into()
                    },
                    role: OrganizationRole::Owner
                }
            )
            .await,
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        store
            .apply(
                org.id,
                &owner,
                Command::CreateProject {
                    name: "x".repeat(201)
                }
            )
            .await,
        Err(Error::Invalid(_))
    ));
    assert_eq!(store.projects(org.id, &owner).await.unwrap().len(), 1);
    assert_eq!(store.access(p.scope, &owner).await.unwrap().grants.len(), 1);
}

pub async fn audit_is_atomic_paginated_and_administrator_only(
    store: &dyn IamStore,
    clock: &TestClock,
) {
    let owner = user("audit-owner");
    let member_user = user("audit-member");
    let outsider = user("audit-outsider");
    let org = store.create_organization(&owner, "Audit").await.unwrap();
    let created = store.audit(org.id, &owner, 0, 1).await.unwrap();
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].sequence, 1);
    assert_eq!(created[0].actor, owner);
    assert!(matches!(
        created[0].action,
        AuditAction::OrganizationCreated { .. }
    ));
    clock.set(1010);
    member(
        store,
        org.id,
        &owner,
        &member_user,
        OrganizationRole::Member,
    )
    .await;
    assert!(matches!(
        store
            .apply(
                org.id,
                &member_user,
                Command::CreateTeam {
                    name: "denied".into()
                }
            )
            .await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        store
            .apply(
                org.id,
                &owner,
                Command::RemoveMember {
                    principal: owner.clone()
                }
            )
            .await,
        Err(Error::LastOwner)
    ));
    assert!(matches!(
        store.audit(org.id, &member_user, 0, 50).await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        store.audit(org.id, &outsider, 0, 50).await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        store.audit(org.id, &owner, -1, 50).await,
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        store.audit(org.id, &owner, 0, 101).await,
        Err(Error::Invalid(_))
    ));
    let page = store
        .audit(org.id, &owner, created[0].sequence, 100)
        .await
        .unwrap();
    assert_eq!(
        page.len(),
        1,
        "failed commands leave no successful audit entry"
    );
    assert_eq!(page[0].sequence, 2);
    assert_eq!(page[0].occurred_at, 1010);
    assert_eq!(page[0].organization, org.id);
    assert!(
        matches!(&page[0].action, AuditAction::CommandApplied { command, change: Change::Applied } if matches!(command.as_ref(), Command::SetMember { principal, .. } if principal == &member_user))
    );
    assert!(
        store
            .audit(org.id, &owner, 2, 100)
            .await
            .unwrap()
            .is_empty()
    );
}

/// What the trail holds, to whoever may read it and to nobody else.
pub async fn audit_bounds_answer_administrators_and_nobody_else(
    store: &dyn IamStore,
    clock: &TestClock,
) {
    let owner = user("bounds-owner");
    let plain = user("bounds-member");
    let outsider = user("bounds-outsider");
    let org = store.create_organization(&owner, "Bounds").await.unwrap();
    clock.set(1_010);
    member(store, org.id, &owner, &plain, OrganizationRole::Member).await;

    let bounds = store.audit_bounds(org.id, &owner).await.unwrap();
    assert_eq!(bounds.organization, org.id);
    assert_eq!(bounds.first_sequence, Some(1));
    assert_eq!(bounds.last_sequence, Some(2));
    assert_eq!(bounds.entries, 2);
    assert!(
        bounds.watermark.is_none(),
        "a trail nothing has swept begins where it always did"
    );

    store.authorize_audit(org.id, &owner).await.unwrap();
    assert!(matches!(
        store.authorize_audit(org.id, &plain).await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        store.audit_bounds(org.id, &plain).await,
        Err(Error::Forbidden)
    ));
    // An outsider is told the organization does not exist, as every other read
    // here tells them.
    assert!(matches!(
        store.audit_bounds(org.id, &outsider).await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        store.authorize_audit(org.id, &outsider).await,
        Err(Error::NotFound)
    ));
}

/// Retention removes what it may, keeps the newest entry, and says so.
///
/// The sweep is deliberately global — one deployment, one clock, one pass — so
/// this scenario keeps to its own corner of a shared test database by writing
/// its entries at seconds 1..4 and pruning with a cutoff of 4. Every other
/// scenario here writes at 1 000 or later, which is what makes a concurrent run
/// safe rather than lucky.
pub async fn retention_keeps_the_newest_entry_and_records_where_the_trail_begins(
    store: &dyn IamStore,
    clock: &TestClock,
) {
    let owner = user("prune-owner");
    let plain = user("prune-member");
    clock.set(1);
    let org = store.create_organization(&owner, "Swept").await.unwrap();
    clock.set(2);
    member(store, org.id, &owner, &plain, OrganizationRole::Member).await;
    clock.set(3);
    store
        .apply(
            org.id,
            &owner,
            Command::CreateTeam {
                name: "before".into(),
            },
        )
        .await
        .unwrap();
    clock.set(4);
    store
        .apply(
            org.id,
            &owner,
            Command::CreateTeam {
                name: "after".into(),
            },
        )
        .await
        .unwrap();

    let retention = AuditRetention::new(1, "policy-iam-audit").unwrap();
    // A day past the fourth entry, so the cutoff lands exactly on it.
    let now = 4 + 86_400;
    let report = store.prune_audit(&retention, now).await.unwrap();
    assert_eq!(report.cutoff, 4);
    assert!(report.removed >= 3, "three entries of this trail were old");

    let bounds = store.audit_bounds(org.id, &owner).await.unwrap();
    assert_eq!(bounds.entries, 1, "the newest entry is never swept");
    assert_eq!(bounds.first_sequence, Some(4));
    assert_eq!(bounds.last_sequence, Some(4));
    let mark = bounds
        .watermark
        .expect("a swept trail says where it begins");
    assert_eq!(mark.pruned_through_sequence, 3);
    assert_eq!(mark.pruned_before, 4);
    assert_eq!(mark.policy_id, "policy-iam-audit");
    assert_eq!(mark.ttl_days, 1);
    assert_eq!(mark.pruned_at, now);
    assert_eq!(mark.removed_total, 3);

    let page = store.audit(org.id, &owner, 0, 100).await.unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].sequence, 4);

    // The one thing a sweep must not do: let the next entry reuse a number an
    // older one already had. A reader paging from the watermark would then meet
    // two different acts under one sequence.
    clock.set(now);
    store
        .apply(
            org.id,
            &owner,
            Command::CreateTeam {
                name: "afterwards".into(),
            },
        )
        .await
        .unwrap();
    let page = store.audit(org.id, &owner, 0, 100).await.unwrap();
    assert_eq!(
        page.iter().map(|entry| entry.sequence).collect::<Vec<_>>(),
        vec![4, 5]
    );

    // Running again at the same moment removes nothing and moves nothing: the
    // window is half-open, so the entry the cutoff lands exactly on is kept.
    store.prune_audit(&retention, now).await.unwrap();
    let mark = store
        .audit_bounds(org.id, &owner)
        .await
        .unwrap()
        .watermark
        .expect("still swept");
    assert_eq!(mark.pruned_through_sequence, 3);
    assert_eq!(mark.removed_total, 3);

    // A second later, entry 4 is old enough. Entry 5 stays because it is both
    // newer than the cutoff and the newest there is.
    store.prune_audit(&retention, now + 1).await.unwrap();
    let bounds = store.audit_bounds(org.id, &owner).await.unwrap();
    let mark = bounds.watermark.expect("still swept");
    assert_eq!(mark.pruned_through_sequence, 4);
    assert_eq!(mark.removed_total, 4);
    assert_eq!(bounds.first_sequence, Some(5));
    assert_eq!(bounds.entries, 1);
}
