//! The tick: an interval, and what fell in it.
//!
//! The loop is deliberately stupid. It reads where the last tick stopped, asks
//! the clock where it is now, hands both to
//! [`Schedule::slots_between`](aiwatcher_execution::Schedule::slots_between) —
//! which is pure — and starts what that returns. Nothing here decides when
//! anything runs; it only supplies the interval, which is the inversion
//! `PassageOfTimeJob` is about (see [`aiwatcher_execution::schedule`]).
//!
//! ## Two workers, no lease
//!
//! An execution started for a slot is named after that slot:
//!
//! ```text
//! aiwatcher/execution/schedule/curation_pipeline/curation-pii/2026-09-08T07:00:00Z
//! ```
//!
//! So two workers that both notice 09:00 is due derive **one** id, and
//! `ExpectedVersion::NoStream` makes the second a conflict rather than a second
//! run — the mechanism that already makes two API replicas racing a start safe,
//! and ADR_0001's "ids are derived, not generated" one layer up. There is no
//! lease here and nothing to expire.
//!
//! It is deliberately derived from the *definition and the slot* rather than
//! from the compiled plan: two workers reading the head a moment apart could
//! compile different `plan_id`s, and an id built on one of those would let both
//! runs through.
//!
//! ## The cursor moves last
//!
//! [`aiwatcher_jobs::ORDERING`] in a seventh place. The starts commit, then the
//! checkpoint advances. A crash in between re-derives the same slot ids on the
//! next tick and the inbox recognises them, so the failure is a repeat that
//! costs nothing — while the other order would skip a slot with nothing
//! anywhere to say it had.

use std::sync::Arc;

use aiwatcher_api::state::AppState;
use aiwatcher_core::Checkpoint;
use aiwatcher_execution::plan::DefinitionKind;
use aiwatcher_execution::{
    FiringOutcome, LastFiring, OverlapPolicy, ScheduleStore, ScheduledDefinition, WorkflowStore,
};
use time::OffsetDateTime;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// The processor whose checkpoint holds where the last tick stopped.
///
/// A processor id like the projector's, in the same table, because it is the
/// same kind of fact: how far through time this consumer has read.
const PROCESSOR: &str = "aiwatcher-scheduler";

/// How often to look.
///
/// A minute, and the tick rate is an operational choice rather than a
/// correctness one — `slots_between` returns the same slots however the span is
/// cut, which `how_often_the_clock_ticks_changes_nothing_about_which_slots_fire`
/// keeps true. What it decides is how late a nine o'clock run may be.
const TICK: std::time::Duration = std::time::Duration::from_secs(60);

/// How far back a first tick reaches.
///
/// A process that has never ticked has no cursor, and reaching back for ever
/// would start every slot since the schedule was written. One interval means a
/// fresh installation starts what is due now and nothing else.
const FIRST_TICK_REACHES_BACK: time::Duration = time::Duration::minutes(1);

/// Start the tick, when this deployment has anywhere to read schedules from.
pub fn spawn(
    state: &AppState,
    schedules: Arc<ScheduleStore>,
    store: Arc<dyn WorkflowStore>,
    shutdown: CancellationToken,
) -> JoinHandle<()> {
    let state = state.clone();
    tracing::info!(
        seconds = TICK.as_secs(),
        "the scheduler is looking for definitions that are due"
    );
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => {
                    tracing::info!("the scheduler is stopping");
                    return;
                }
                () = tokio::time::sleep(TICK) => {}
            }
            if let Err(error) = tick(&state, schedules.as_ref(), store.as_ref()).await {
                // The cursor did not move, so the next tick covers this
                // interval as well. A store that is down costs latency and
                // never a slot.
                tracing::warn!(%error, "a scheduler tick could not run");
            }
        }
    })
}

/// One pass: read the interval, start what is in it, then move the cursor.
async fn tick(
    state: &AppState,
    schedules: &ScheduleStore,
    store: &dyn WorkflowStore,
) -> anyhow::Result<()> {
    let now = OffsetDateTime::now_utc();
    // A `Checkpoint` is a zero-padded position, so the cursor is a Unix second
    // rather than a formatted time. That is not a workaround: this processor's
    // axis *is* time, and "how far through it have I read" is the same question
    // the projector's checkpoint answers about the log. Seconds are finer than
    // any slot, which is at a minute.
    let previous = store
        .checkpoint(PROCESSOR)
        .await?
        .and_then(|checkpoint| checkpoint.global_position())
        .and_then(|seconds| i64::try_from(seconds).ok())
        .and_then(|seconds| OffsetDateTime::from_unix_timestamp(seconds).ok())
        .unwrap_or(now - FIRST_TICK_REACHES_BACK);

    let scheduled = schedules.all().await?;
    for definition in &scheduled {
        for slot in definition.schedule.slots_between(previous, now) {
            // One slot that could not start does not stop the others: a
            // definition that stopped compiling must not hold up every other
            // schedule in the instance.
            let last = match fire(state, definition, slot).await {
                Ok(last) => last,
                Err(error) => {
                    tracing::warn!(
                        definition = %definition.definition_name,
                        slot = %slot,
                        %error,
                        "a scheduled run could not be started"
                    );
                    LastFiring {
                        slot,
                        outcome: FiringOutcome::Refused,
                        execution_id: None,
                        detail: Some(error.to_string()),
                    }
                }
            };
            // Written to the schedule so somebody can see it. A log line and
            // silence were the alternatives, and a schedule refused every
            // morning for a week looks from the panel exactly like one that
            // has been working.
            //
            // After the run, never before: a note saying `started` for a run
            // that was not is the one wrong answer here, and the other order —
            // a run with no note — is a repeat of the note next tick at worst.
            let recorded = ScheduledDefinition {
                last: Some(last),
                ..definition.clone()
            };
            if let Err(error) = schedules.set(&recorded).await {
                tracing::warn!(
                    definition = %definition.definition_name,
                    %error,
                    "a scheduled run happened and could not be written down"
                );
            }
        }
    }

    // Last, and that is the whole ordering. See the module docs.
    store
        .advance_checkpoint(
            PROCESSOR,
            Checkpoint::from_global_position(now.unix_timestamp().max(0).unsigned_abs()),
        )
        .await?;
    Ok(())
}

/// Start one definition for one slot, unless its last run is still going.
async fn fire(
    state: &AppState,
    definition: &ScheduledDefinition,
    slot: OffsetDateTime,
) -> anyhow::Result<LastFiring> {
    let execution_id = definition.execution_id_for(slot);

    if definition.schedule.overlap == OverlapPolicy::Skip
        && let Some(running) = last_run_still_going(state, definition).await?
    {
        tracing::info!(
            definition = %definition.definition_name,
            slot = %slot,
            still_running = %running,
            "skipped a scheduled slot because the last run has not finished"
        );
        return Ok(LastFiring {
            slot,
            outcome: FiringOutcome::Skipped,
            // The run that was in the way, so "skipped" is followed rather
            // than merely reported.
            execution_id: Some(running),
            detail: None,
        });
    }

    let DefinitionKind::CurationPipeline = definition.definition_kind else {
        anyhow::bail!("only a curation pipeline can be scheduled so far");
    };
    // The head, read and pinned now. A schedule says *what* to run and never
    // which revision: "every day at nine, the latest saved version" is what
    // somebody setting one means, and the run records the revision it pinned so
    // a bad save is visible in the run rather than silent.
    let plan = aiwatcher_api::executions::compile_curation_named(
        state,
        &definition.definition_name,
        None,
        None,
    )
    .await
    .map_err(|error| anyhow::anyhow!("{error}"))?;

    let handled = aiwatcher_api::executions::start(
        state,
        &execution_id,
        plan,
        std::collections::BTreeMap::new(),
        &format!("schedule:{}", definition.set_by),
    )
    .await
    .map_err(|error| anyhow::anyhow!("{error}"))?;

    tracing::info!(
        execution_id = %execution_id,
        definition = %definition.definition_name,
        slot = %slot,
        // True when another worker got there first, which is the expected
        // outcome of two replicas rather than a problem.
        duplicate = handled.duplicate,
        "started a scheduled run"
    );
    Ok(LastFiring {
        slot,
        outcome: FiringOutcome::Started,
        execution_id: Some(execution_id.to_string()),
        detail: None,
    })
}

/// Whichever of this definition's runs has not finished, if one has not.
///
/// Read from the *log's* fold rather than the workflow store, because that is
/// the one that lists executions by definition — the inline projection is per
/// run and ADR_0026 is explicit that a list comes from the fold.
async fn last_run_still_going(
    state: &AppState,
    definition: &ScheduledDefinition,
) -> anyhow::Result<Option<String>> {
    let page = state
        .read_model
        .workflow_executions(&aiwatcher_projector::ExecutionFilter {
            workflow_id: Some(definition.definition_name.clone()),
            status: Some(aiwatcher_projector::ExecutionStatus::Running),
            limit: Some(1),
            ..Default::default()
        })
        .await;
    Ok(page
        .executions
        .into_iter()
        .next()
        .map(|execution| execution.workflow_run_id))
}
