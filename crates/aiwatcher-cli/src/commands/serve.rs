//! Running the server: `aiwatcher serve`, `aiwatcher work`, and the bare
//! invocation that is both.
//!
//! Moved here from the binary unchanged, because it is one of the CLI's
//! commands rather than the whole of it. What changed is where the role comes
//! from: the dispatcher has already read it, so this takes a [`Config`] that is
//! settled and does not re-read `argv`.

use std::time::Duration;

use anyhow::{Context, Result};
use tokio_util::sync::CancellationToken;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use aiwatcher_server::config::{Config, LogFormat};

/// How long a background task is given to stop before the process exits anyway.
const GRACE: Duration = Duration::from_secs(10);

/// Run the server to completion, in whichever role `config` names.
///
/// # Errors
///
/// Whatever stopped it from starting: a configuration the crate refused, a
/// port already held, a backend that could not be reached.
pub async fn run(config: Config) -> Result<()> {
    init_tracing(config.log_format);

    tracing::info!(
        role = config.role.as_str(),
        listen = %config.listen,
        bus = ?config.bus,
        workflow_store = ?config.workflow_store,
        query_engine = config.query_engine.as_str(),
        query = config.query_url.as_deref().unwrap_or("<none>"),
        otlp = config.otlp_endpoint.as_deref().unwrap_or("<none>"),
        ingest_enabled = config.ingest_enabled,
        auth = config.auth.mode.as_str(),
        "starting aiwatcher"
    );

    if config.role == aiwatcher_server::config::ProcessRole::Journal {
        return journal(&config).await;
    }

    let runtime = aiwatcher_server::build(config).await?;
    let aiwatcher_server::Runtime {
        state,
        config,
        workflow_store,
        artifacts,
        sink,
        metrics,
        projector,
        journal,
    } = runtime;

    let shutdown = CancellationToken::new();

    // The journal of what the period fold reads runs in every role: beside the
    // projector in one process, and in both halves of a split one, which read
    // under one group name so the log gives its partition to one of them and
    // hands it to the other when that one stops. While anything that accepts
    // events is up, a journal is reading them.
    let journal_task = journal.map(|journal| {
        let shutdown = shutdown.clone();
        tokio::spawn(async move { journal.run(shutdown).await })
    });

    // Both roles' background work, started before anything is served: a
    // command accepted by an instance whose reactors are not running yet is a
    // step that waits for a poll interval, and the store is durable either
    // way — but the log line saying which runtimes this process can run
    // belongs above "listening", not below it.
    let execution = aiwatcher_server::execution::spawn(
        &state,
        &config,
        &workflow_store,
        &sink,
        artifacts.as_ref(),
        &metrics,
        &shutdown,
    );

    if !config.role.serves() {
        // The work role holds no ingress. It stops on a signal, drains, and
        // that is the whole lifecycle — there is no socket to close.
        tracing::info!("the work role is running; no HTTP listener");
        wait_for_signal().await;
        tracing::info!("shutdown signal received");
        shutdown.cancel();
        execution.drain(GRACE).await;
        stop_journal(journal_task).await;
        return Ok(());
    }

    let projector_task = {
        let shutdown = shutdown.clone();
        tokio::spawn(async move { projector.run(shutdown).await })
    };

    // The archive's own two background jobs: the export worker and the
    // retention sweep. `None` when this deployment keeps no archive, which is
    // the default.
    let evaluation_task = aiwatcher_server::evaluation::spawn(&state, shutdown.clone());
    let archive_task = aiwatcher_server::conversations::spawn(&state, &config, shutdown.clone());

    // The IAM control plane's own two: the audit export worker and the audit
    // retention sweep. `None` unless this deployment has an IAM store, and
    // unless it has either somewhere to export into or a retention to apply.
    let iam_audit_task = aiwatcher_server::iam::spawn(&state, &config, shutdown.clone());

    // The alert half: two watchers looking for what is worth telling somebody
    // about, the queue that sends it, and the sweep that bounds the history.
    // Empty when this deployment has no object store to keep rules in.
    let alert_tasks = aiwatcher_server::alerts::spawn(&state, &config, shutdown.clone());

    // The annotation registry's own background job: the import queue. `None`
    // when no object store is configured, which is when there is no registry
    // to import into either.
    let import_task = aiwatcher_server::imports::spawn(&state, &config, shutdown.clone());

    // Ready once the projector is consuming. Before this, `/readyz` reports
    // 503 so a rolling deploy does not send traffic to an instance whose read
    // model is still empty.
    state.health.mark_ready();

    let app = aiwatcher_api::router(state.clone())
        .layer(TraceLayer::new_for_http())
        .layer(cors_layer(&config.cors_origins));

    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("binding {}", config.listen))?;
    tracing::info!(address = %config.listen, "http server listening");

    let server_shutdown = shutdown.clone();
    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        wait_for_signal().await;
        tracing::info!("shutdown signal received");
        server_shutdown.cancel();
    });

    if let Err(error) = server.await {
        tracing::error!(%error, "http server stopped");
    }

    // Give the projector a moment to drain its open spans before exiting.
    state.health.mark_unready();
    shutdown.cancel();
    execution.drain(GRACE).await;
    if let Some(task) = evaluation_task {
        task.await.context("evaluation retention worker")?;
    }
    if let Some(task) = archive_task {
        match tokio::time::timeout(GRACE, task).await {
            Ok(Ok(())) => tracing::info!("the conversation archive worker stopped"),
            Ok(Err(error)) => tracing::error!(%error, "the conversation archive worker panicked"),
            // An export in flight has committed every shard it finished, so
            // whichever process picks the job up next resumes from there.
            Err(_) => tracing::warn!("the conversation archive worker did not stop within 10s"),
        }
    }
    if let Some(task) = iam_audit_task {
        match tokio::time::timeout(GRACE, task).await {
            Ok(Ok(())) => tracing::info!("the IAM audit worker stopped"),
            Ok(Err(error)) => tracing::error!(%error, "the IAM audit worker panicked"),
            // An export in flight has committed every shard it finished, so
            // whichever process picks the job up next resumes from there.
            Err(_) => tracing::warn!("the IAM audit worker did not stop within 10s"),
        }
    }
    for task in alert_tasks {
        match tokio::time::timeout(GRACE, task).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::error!(%error, "an alert worker panicked"),
            // A notification in flight is recorded after the attempt, so one
            // cut off here is re-sent and the receiver knows it by its key.
            Err(_) => tracing::warn!("an alert worker did not stop within 10s"),
        }
    }
    if let Some(task) = import_task {
        match tokio::time::timeout(GRACE, task).await {
            Ok(Ok(())) => tracing::info!("the annotation import worker stopped"),
            Ok(Err(error)) => tracing::error!(%error, "the annotation import worker panicked"),
            // An import in flight has committed every page it finished, so
            // whichever process picks the job up next resumes from there.
            Err(_) => tracing::warn!("the annotation import worker did not stop within 10s"),
        }
    }
    stop_journal(journal_task).await;
    match tokio::time::timeout(Duration::from_secs(30), projector_task).await {
        Ok(Ok(Ok(()))) => tracing::info!("projector drained cleanly"),
        Ok(Ok(Err(error))) => tracing::error!(%error, "projector stopped with an error"),
        Ok(Err(error)) => tracing::error!(%error, "projector task panicked"),
        Err(_) => tracing::warn!("projector did not drain within 30s; exiting anyway"),
    }

    Ok(())
}

/// The journal role: the observation journal and nothing else, until a signal
/// or until it stops on its own — which is an error, since it reads a log that
/// does not end.
async fn journal(config: &Config) -> Result<()> {
    let journal = aiwatcher_server::build_journal(config).await?;
    let shutdown = CancellationToken::new();
    let mut task = {
        let shutdown = shutdown.clone();
        tokio::spawn(async move { journal.run(shutdown).await })
    };
    tracing::info!("the journal role is running; no HTTP listener");
    tokio::select! {
        () = wait_for_signal() => {
            tracing::info!("shutdown signal received");
            shutdown.cancel();
            stop_journal(Some(task)).await;
            Ok(())
        }
        stopped = &mut task => match stopped {
            Ok(Ok(())) => anyhow::bail!("the observation journal stopped with nothing asking it to"),
            Ok(Err(error)) => Err(error),
            Err(error) => Err(anyhow::anyhow!(error).context("the observation journal panicked")),
        },
    }
}

fn init_tracing(format: LogFormat) {
    let filter = EnvFilter::try_from_env("AIWATCHER_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new("info,aiwatcher=debug"));
    let registry = tracing_subscriber::registry().with(filter);

    match format {
        LogFormat::Json => registry
            .with(tracing_subscriber::fmt::layer().json())
            .init(),
        LogFormat::Pretty => registry.with(tracing_subscriber::fmt::layer()).init(),
    }
}

/// Permissive only when explicitly configured. An empty origin list leaves CORS
/// off entirely, which is right when the panel is served from the same origin.
fn cors_layer(origins: &[String]) -> CorsLayer {
    if origins.is_empty() {
        return CorsLayer::new();
    }
    if origins.iter().any(|origin| origin == "*") {
        tracing::warn!("AIWATCHER_CORS_ORIGINS is '*'; every origin may call this API");
        return CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any);
    }
    let parsed: Vec<_> = origins
        .iter()
        .filter_map(|origin| match origin.parse() {
            Ok(value) => Some(value),
            Err(_) => {
                tracing::warn!(origin, "ignoring an unparsable CORS origin");
                None
            }
        })
        .collect();
    CorsLayer::new()
        .allow_origin(parsed)
        .allow_methods(Any)
        .allow_headers(Any)
}

/// Ctrl-C, plus SIGTERM where there is one — Kubernetes sends SIGTERM, and
/// listening only for Ctrl-C means every pod eviction is a hard kill.
async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = match signal(SignalKind::terminate()) {
            Ok(stream) => stream,
            Err(error) => {
                tracing::error!(%error, "cannot listen for SIGTERM; Ctrl-C only");
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Let the journal keep the page it was reading, within the grace.
async fn stop_journal(task: Option<tokio::task::JoinHandle<Result<()>>>) {
    let Some(task) = task else {
        return;
    };
    match tokio::time::timeout(GRACE, task).await {
        Ok(Ok(Ok(()))) => tracing::info!("the observation journal stopped"),
        Ok(Ok(Err(error))) => {
            tracing::error!(%error, "the observation journal stopped with an error")
        }
        Ok(Err(error)) => tracing::error!(%error, "the observation journal panicked"),
        // What it had not kept is read again from its committed position.
        Err(_) => tracing::warn!("the observation journal did not stop within 10s"),
    }
}
