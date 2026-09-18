use aiwatcher_iam::memory::MemoryIamStore;
use std::sync::Arc;
mod support;

macro_rules! contract {
    ($($name:ident),+ $(,)?) => { $(
        #[tokio::test]
        async fn $name() {
            let clock = Arc::new(support::TestClock::default());
            let store = MemoryIamStore::with_clock(clock.clone());
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
