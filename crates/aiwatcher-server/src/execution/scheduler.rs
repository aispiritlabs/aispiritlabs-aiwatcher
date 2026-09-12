//! The tick: an interval, and what fell in it.
//!
//! It reads where the last tick stopped, asks the clock where it is now, hands
//! both to [`Schedule::slots_between`](aiwatcher_execution::Schedule::slots_between)
//! — which is pure — and starts what comes back. Nothing here decides when
//! anything runs.
//!
//! An execution is named after its slot:
//!
//! ```text
//! aiwatcher/execution/schedule/curation_pipeline/curation-pii/2026-09-08T07:00:00Z
//! ```
//!
//! So two workers that both notice 09:00 derive **one** id, and
//! `ExpectedVersion::NoStream` makes the second a conflict rather than a second
//! run. No lease, nothing to expire. It is derived from the definition and the
//! slot, never from the compiled plan: two workers reading the head a moment
//! apart compile different `plan_id`s and both runs would go through.
//!
//! The cursor moves last ([`aiwatcher_jobs::ORDERING`]). A crash between the
//! starts and the checkpoint re-derives the same ids and the inbox recognises
//! them; the other order skips a slot with nothing to say it had.

use std::sync::Arc;

use aiwatcher_api::state::AppState;
use aiwatcher_core::Checkpoint;
use aiwatcher_execution::{
    Decider, RunIdentity, ScheduleReader, ScheduleStore, ScheduledDefinition, SlotAdmission,
    SlotAdmissionRequest, SlotKey, SlotSettlement, StartRefused, StartRun, WorkflowStore,
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
    metrics: Arc<dyn aiwatcher_core::ports::MetricSink>,
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
            if let Err(error) =
                tick(&state, schedules.as_ref(), store.as_ref(), metrics.as_ref()).await
            {
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
    schedules: &dyn ScheduleReader,
    store: &dyn WorkflowStore,
    metrics: &dyn aiwatcher_core::ports::MetricSink,
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
    // Every slot this tick found, before any of them is started. How late one
    // slot was and how many were due together are one reading: a single slot
    // four minutes behind is a busy tick, and forty is an instance that has
    // fallen behind, and the lateness alone cannot tell those apart.
    let due: Vec<_> = scheduled
        .iter()
        .flat_map(|definition| {
            // `slots_due`, not `slots_between`: the interval is clipped to the
            // schedule's own activation moment, so a schedule written while
            // this worker was down does not run the days before somebody asked
            // for it.
            definition
                .slots_due(previous, now)
                .into_iter()
                .map(move |slot| (definition, slot))
        })
        .collect();
    let backlog = due.len();

    let mut measured = Vec::new();
    for (definition, slot) in due {
        measured.extend(super::measure::slot_samples(
            &definition.definition_name,
            definition.definition_kind.as_str(),
            slot,
            now,
            backlog,
        ));
        // One slot that could not start does not stop the others: a definition
        // that stopped compiling must not hold up every other schedule in the
        // instance.
        if let Err(error) = process(state, store, definition, slot, now).await {
            tracing::warn!(
                definition = %definition.definition_name,
                slot = %slot,
                %error,
                "a scheduled slot could not be processed"
            );
        }
    }
    // After the slots, and never in place of starting one: a sink that is down
    // costs a graph and must not cost a run. The cursor still moves below,
    // because whether the measurement was reported says nothing about whether
    // the interval was read.
    if !measured.is_empty()
        && let Err(error) = metrics.record(measured).await
    {
        tracing::warn!(%error, "the scheduler's own timing could not be reported");
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

/// Take one slot, start what it asks for, and write down what happened.
///
/// The order is the whole point. The slot is taken in the workflow store —
/// where the overlap check happens in the same transaction — the run is
/// started, and only then is the slot settled. Nothing here writes the
/// schedule object, which is what stops a tick undoing an edit or bringing a
/// deleted schedule back.
async fn process(
    state: &AppState,
    store: &dyn WorkflowStore,
    definition: &ScheduledDefinition,
    slot: OffsetDateTime,
    now: OffsetDateTime,
) -> anyhow::Result<()> {
    let key = SlotKey::new(
        definition.definition_kind,
        definition.definition_name.clone(),
        slot,
    );
    let owner = owner();
    let admission = store
        .admit_slot(&SlotAdmissionRequest {
            key: key.clone(),
            owner: owner.clone(),
            overlap: definition.schedule.overlap,
            now,
        })
        .await?;

    match admission {
        SlotAdmission::Settled { outcome } => {
            // The expected answer for the second of two replicas, and for a
            // replay of an interval this instance has already processed.
            tracing::debug!(
                definition = %definition.definition_name,
                slot = %slot,
                ?outcome,
                "a slot somebody already decided"
            );
            return Ok(());
        }
        SlotAdmission::Held { owner: holder } => {
            tracing::debug!(
                definition = %definition.definition_name,
                slot = %slot,
                %holder,
                "a slot somebody else is starting"
            );
            return Ok(());
        }
        SlotAdmission::Blocked { execution_id } => {
            // `skip`, decided against a projection this store holds rather than
            // against a read model that is empty in this role. Settled by the
            // caller, because "skipped" is a decision and `admit_slot` is only
            // ever asked whether this caller *may* start.
            tracing::info!(
                definition = %definition.definition_name,
                slot = %slot,
                still_running = %execution_id,
                "skipped a scheduled slot because the last run has not finished"
            );
            store
                .settle_slot(&key, &owner, SlotSettlement::Skipped { execution_id }, now)
                .await?;
            return Ok(());
        }
        SlotAdmission::Admitted => {}
    }

    let settlement = match start_run(state, definition, slot).await {
        Ok(execution_id) => {
            tracing::info!(
                execution_id = %execution_id,
                definition = %definition.definition_name,
                slot = %slot,
                "started a scheduled run"
            );
            SlotSettlement::Started { execution_id }
        }
        Err(refusal) => {
            // R2's line, and the refusal answers it. A definition that stopped
            // compiling will not compile on the next tick either, and neither
            // will a deployment with no registry behind it; a store that was
            // unreachable for ten seconds may. The first is a decision; the
            // second must leave the slot due, which is what `TryAgain` does.
            let settled = refusal.says_the_same_next_time();
            let detail = refusal.to_string();
            if settled {
                tracing::warn!(
                    definition = %definition.definition_name,
                    slot = %slot,
                    %detail,
                    "a scheduled run was refused"
                );
                SlotSettlement::Refused { detail }
            } else {
                tracing::warn!(
                    definition = %definition.definition_name,
                    slot = %slot,
                    %detail,
                    "a scheduled run could not be started yet; the slot stays due"
                );
                SlotSettlement::TryAgain { detail }
            }
        }
    };
    store.settle_slot(&key, &owner, settlement, now).await?;
    Ok(())
}

/// Compile the head, pin it, and start the run named after this slot.
///
/// One call into the use case both this and the route go through
/// ([`aiwatcher_execution::start`]), rather than into an axum handler's crate:
/// the tick used to read the **HTTP status** off an `ApiError` to decide
/// whether the slot should stay due, and a status is a lossy encoding of that
/// question. A deployment with no dataset registry answers 501, a corrupt
/// definition 500 and a rejected read 502 — all 5xx, all permanent, all filed
/// as "come back in a minute", every minute, for ever.
async fn start_run(
    state: &AppState,
    definition: &ScheduledDefinition,
    slot: OffsetDateTime,
) -> std::result::Result<String, StartRefused> {
    // The head, read and pinned now. A schedule says *what* to run and never
    // which revision: "every day at nine, the latest saved version" is what
    // somebody setting one means, and the run records the revision it pinned so
    // a bad save is visible in the run rather than silent.
    //
    // Which compiler answers is the definition's kind, and the choice is made
    // once, by the same call the schedule route makes before it agrees to save
    // this. A `match` here as well would be a second answer to "what does this
    // schedule run": the day a third kind arrives, one side starts a run and
    // the other refuses to save the schedule for it.
    let executions = state.executions();
    let plan = executions
        .compile_head(definition.definition_kind, &definition.definition_name)
        .await?;

    let execution_id = definition.execution_id_for(slot);
    executions
        .start(
            plan,
            StartRun {
                // Derived from the definition and the slot, so two workers that
                // both notice nine o'clock reach one run.
                identity: RunIdentity::Named(execution_id.clone()),
                parameters: std::collections::BTreeMap::new(),
                requested_by: format!("schedule:{}", definition.set_by),
                // The tick compiles a definition and this system runs it. A
                // hosted run needs a worker present to decide it, which a slot
                // coming due cannot arrange.
                decided_by: Decider::Local,
                // The deployment's own. A pinned instance with no archive is a
                // fact about the deployment, and the refusal says so itself.
                payloads: None,
            },
        )
        .await?;
    Ok(execution_id.to_string())
}

/// This process, for the lease on a slot.
///
/// A name rather than an id, because the only thing read off it is whether the
/// holder is still this caller — and after a restart it deliberately is not,
/// so a slot the previous process died holding is taken over once its lease
/// runs out.
fn owner() -> String {
    format!(
        "scheduler/{}/{}",
        std::process::id(),
        std::env::var("HOSTNAME").unwrap_or_else(|_| "local".to_owned())
    )
}
