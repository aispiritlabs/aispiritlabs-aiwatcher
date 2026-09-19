//! Wiring the alert half: the channel this deployment sends through, the
//! watchers that decide there is something to send, and the loop that sends
//! it.
//!
//! Three loops rather than one, because they are three different clocks. The
//! drain wants to be quick — a notification held for a minute is a minute
//! nobody knows. The watchers want to be cheap — they read folds and page an
//! index, and asking twice a second would buy nothing a source updates that
//! rarely. The sweep wants to be rare, because it only bounds a history.
//!
//! All three live in the serving role. The sources are what this process
//! already holds: the workflow fold is in its read model and the evaluation
//! registry is its object store, and a work role holds neither.

pub mod watch;
pub mod webhook;

use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use aiwatcher_alerts::{AlertChannel, Delivery, Registry};
use aiwatcher_api::state::AppState;

use crate::config::Config;

/// How often the queue is drained.
const DRAIN_EVERY: Duration = Duration::from_secs(5);

/// How often the watchers look.
///
/// Half a minute. An execution that failed and an evaluation that regressed
/// are both minutes-old news by the time anybody could act, and a tighter loop
/// would page the evidence index for nothing.
const WATCH_EVERY: Duration = Duration::from_secs(30);

/// How often finished history is forgotten.
const SWEEP_EVERY: Duration = Duration::from_secs(3_600);

/// How many finished rows one sweep removes.
const SWEEP_BATCH: usize = 500;

/// How many notifications one drain pass sends.
///
/// Bounded so a backlog after an outage is worked through in steady batches
/// rather than in one burst the receiver rate-limits into failures.
const DRAIN_BATCH: usize = 32;

/// The channel this deployment sends through, or `None` when it has none.
///
/// # Errors
///
/// Only when the HTTP client cannot be built. An endpoint that does not answer
/// is not an error here: a channel that refused to start because the receiver
/// was down would take aiwatcher down with whatever it was meant to warn
/// about.
pub fn build_channel(config: &Config) -> anyhow::Result<Option<Arc<dyn AlertChannel>>> {
    let Some(endpoint) = config.alert_webhook_url.clone() else {
        return Ok(None);
    };
    let channel = webhook::WebhookChannel::new(webhook::WebhookConfig {
        endpoint,
        token: config.alert_webhook_token.clone(),
        secret: config.alert_webhook_secret.clone(),
        timeout: Duration::from_secs(config.alert_webhook_timeout_seconds),
    })?;
    tracing::info!(
        variable = webhook::URL_VARIABLE,
        signed = config.alert_webhook_secret.is_some(),
        "alerts will be sent to the configured webhook"
    );
    Ok(Some(Arc::new(channel)))
}

/// Start the alert loops, when this deployment has somewhere to keep rules.
///
/// Returns nothing to hold when there is no alert registry — the same
/// condition that leaves the routes answering 501 by name.
#[must_use]
pub fn spawn(
    state: &AppState,
    config: &Config,
    shutdown: CancellationToken,
) -> Vec<JoinHandle<()>> {
    let Some(alerts) = state.alerts.clone() else {
        return Vec::new();
    };
    let mut tasks = vec![spawn_watchers(state, &alerts, shutdown.clone())];
    tasks.push(spawn_sweep(
        Arc::clone(&alerts),
        config.alert_history_days,
        shutdown.clone(),
    ));
    match state.alert_channel.clone() {
        Some(channel) => tasks.push(spawn_drain(alerts, channel, shutdown)),
        None => tracing::info!(
            variable = webhook::URL_VARIABLE,
            "no alert channel is configured; rules are kept and nothing is sent"
        ),
    }
    tasks
}

/// Look for what is worth telling somebody about.
fn spawn_watchers(
    state: &AppState,
    alerts: &Arc<Registry>,
    shutdown: CancellationToken,
) -> JoinHandle<()> {
    let alerts = Arc::clone(alerts);
    let read_model = Arc::clone(&state.read_model);
    let evaluations = state.evaluations.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => {
                    tracing::info!("the alert watchers are stopping");
                    return;
                }
                // Before the first pass as well as between them, so a process
                // that restarts every few minutes is not a process that
                // watches on every start.
                () = tokio::time::sleep(WATCH_EVERY) => {}
            }
            let rules = match alerts.active().await {
                Ok(rules) => rules,
                Err(error) => {
                    tracing::warn!(%error, "the alert rules could not be read");
                    continue;
                }
            };
            if rules.is_empty() {
                continue;
            }
            let now = aiwatcher_alerts::now();

            match watch::execution_failures(&alerts, &read_model, &rules, now).await {
                Ok(watched) if watched.started => tracing::info!(
                    "alerts will report executions that fail from now on, not before"
                ),
                Ok(watched) if watched.raised > 0 => {
                    tracing::info!(raised = watched.raised, "failed executions raised alerts");
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(%error, "the execution watcher could not run"),
            }

            let Some(evaluations) = evaluations.clone() else {
                continue;
            };
            match watch::evaluation_regressions(&alerts, &evaluations, &rules, now).await {
                Ok(watched) if watched.started => tracing::info!(
                    "alerts will report regressions measured from now on, not before"
                ),
                Ok(watched) if watched.raised > 0 => {
                    tracing::info!(raised = watched.raised, "regressions raised alerts");
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(%error, "the evaluation watcher could not run"),
            }
        }
    })
}

/// Send what is due, until there is nothing due.
fn spawn_drain(
    alerts: Arc<Registry>,
    channel: Arc<dyn AlertChannel>,
    shutdown: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => {
                    tracing::info!("the alert drain is stopping");
                    return;
                }
                () = tokio::time::sleep(DRAIN_EVERY) => {}
            }
            if let Err(error) = drain(&alerts, channel.as_ref()).await {
                // The rows stay queued. A store that is down keeps its queue,
                // which is the failure worth having.
                tracing::warn!(%error, "the alert queue could not be drained");
            }
        }
    })
}

/// One pass: try every due notification once, and write down what happened.
///
/// The record is written **after** the attempt, always — a crash between the
/// send and the write re-sends, which the receiver deduplicates by the key it
/// was given, and writing first would mark a notification delivered that
/// nobody has.
async fn drain(alerts: &Registry, channel: &dyn AlertChannel) -> aiwatcher_alerts::Result<()> {
    loop {
        let now = aiwatcher_alerts::now();
        let due = alerts.due(DRAIN_BATCH, now).await?;
        if due.is_empty() {
            return Ok(());
        }
        for mut delivery in due {
            send_once(alerts, channel, &mut delivery, aiwatcher_alerts::now()).await?;
        }
    }
}

async fn send_once(
    alerts: &Registry,
    channel: &dyn AlertChannel,
    delivery: &mut Delivery,
    now: i64,
) -> aiwatcher_alerts::Result<()> {
    match channel.deliver(&delivery.payload).await {
        Ok(()) => delivery.sent(now),
        Err(error) => {
            let retryable = error.is_retryable();
            delivery.attempt_failed(&error.to_string(), retryable, now);
            if delivery.state == aiwatcher_jobs::JobState::Failed {
                tracing::warn!(
                    rule = %delivery.rule,
                    key = delivery.dedup_key,
                    attempts = delivery.attempts,
                    %error,
                    "a notification was given up on; it stays in the history to be sent by hand"
                );
            }
        }
    }
    alerts.record(delivery).await
}

/// Forget finished history past the window this deployment keeps.
fn spawn_sweep(alerts: Arc<Registry>, days: u32, shutdown: CancellationToken) -> JoinHandle<()> {
    tracing::info!(days, "alert history is forgotten after this long");
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => {
                    tracing::info!("the alert history sweep is stopping");
                    return;
                }
                () = tokio::time::sleep(SWEEP_EVERY) => {}
            }
            let before = aiwatcher_alerts::now() - i64::from(days) * 86_400;
            match alerts.prune(before, SWEEP_BATCH).await {
                Ok(0) => {}
                Ok(pruned) => tracing::info!(pruned, "forgot alert history past the window"),
                Err(error) => tracing::warn!(%error, "the alert history sweep could not run"),
            }
        }
    })
}
