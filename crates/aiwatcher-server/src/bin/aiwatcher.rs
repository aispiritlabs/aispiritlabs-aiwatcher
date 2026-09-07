//! The aiwatcher server.
//!
//! One binary, two roles (section 27, ADR_0025). `aiwatcher serve` holds the
//! API, the read model and the object store; `aiwatcher work` holds the
//! execution outbox and the reactors, and is the only role that opens a socket
//! to Flow, a notebook runtime, an engine or the cluster.
//!
//! Called with no role — `just run`, `just dev`, `cargo run --bin aiwatcher` —
//! it runs **both in one process**, which is not a third role: it is the two of
//! them together, and it is what the `file` workflow store requires, since that
//! store holds one process and both halves need it. A deployment that wants the
//! network boundary runs two Deployments against `postgres`, and the one
//! holding the cluster's credentials is the one holding no ingress.
//!
//! The projector runs in `serve` rather than in `work`, which is where section
//! 27 puts "the consumers". In this codebase the projector *is* the read model
//! the API answers from, in process, under a memory contract; a `serve` role
//! without it would serve every read from an empty fold. See
//! `execution`'s module docs.

use std::time::Duration;

use anyhow::{Context, Result};
use tokio_util::sync::CancellationToken;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use aiwatcher_server::config::{Config, LogFormat, ProcessRole};

/// How long a background task is given to stop before the process exits anyway.
const GRACE: Duration = Duration::from_secs(10);

#[tokio::main]
async fn main() -> Result<()> {
    let mut config = Config::from_env().context("reading configuration")?;
    // The argument wins over the variable: a container sets `AIWATCHER_ROLE`
    // and a person types `aiwatcher work`, and the one typed last is the one
    // that was meant.
    if let Some(role) = role_argument()? {
        config.role = role;
    }
    // Read again, because the argument can have made a valid configuration
    // invalid — splitting the binary in two on a store that holds one process
    // is the case, and it is better said here than by a lock file.
    config.validate().context("reading configuration")?;
    init_tracing(config.log_format);

    tracing::info!(
        role = config.role.as_str(),
        listen = %config.listen,
        bus = ?config.bus,
        workflow_store = ?config.workflow_store,
        flow = config.flow_url.as_deref().unwrap_or("<none>"),
        otlp = config.otlp_endpoint.as_deref().unwrap_or("<none>"),
        ingest_enabled = config.ingest_enabled,
        auth = config.auth.mode.as_str(),
        "starting aiwatcher"
    );

    let runtime = aiwatcher_server::build(config).await?;
    let aiwatcher_server::Runtime {
        state,
        config,
        workflow_store,
        artifacts,
        sink,
        projector,
    } = runtime;

    let shutdown = CancellationToken::new();

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
        return Ok(());
    }

    let projector_task = {
        let shutdown = shutdown.clone();
        tokio::spawn(async move { projector.run(shutdown).await })
    };

    // The archive's own two background jobs: the export worker and the
    // retention sweep. `None` when this deployment keeps no archive, which is
    // the default.
    let archive_task = aiwatcher_server::conversations::spawn(&state, &config, shutdown.clone());

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
    if let Some(task) = archive_task {
        match tokio::time::timeout(GRACE, task).await {
            Ok(Ok(())) => tracing::info!("the conversation archive worker stopped"),
            Ok(Err(error)) => tracing::error!(%error, "the conversation archive worker panicked"),
            // An export in flight has committed every shard it finished, so
            // whichever process picks the job up next resumes from there.
            Err(_) => tracing::warn!("the conversation archive worker did not stop within 10s"),
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
    match tokio::time::timeout(Duration::from_secs(30), projector_task).await {
        Ok(Ok(Ok(()))) => tracing::info!("projector drained cleanly"),
        Ok(Ok(Err(error))) => tracing::error!(%error, "projector stopped with an error"),
        Ok(Err(error)) => tracing::error!(%error, "projector task panicked"),
        Err(_) => tracing::warn!("projector did not drain within 30s; exiting anyway"),
    }

    Ok(())
}

/// `aiwatcher serve` or `aiwatcher work`, when one was typed.
///
/// A hand-rolled read rather than a parser: this binary has exactly one
/// argument and adding a dependency to read it would be the wrong trade. An
/// unrecognised one is refused by name rather than ignored — a typo that
/// silently ran both roles would be a deployment quietly holding the cluster's
/// credentials next to its ingress.
fn role_argument() -> Result<Option<ProcessRole>> {
    let Some(argument) = std::env::args().nth(1) else {
        return Ok(None);
    };
    argument
        .parse()
        .map(Some)
        .map_err(|error| anyhow::anyhow!("{error}"))
        .with_context(|| format!("reading the role argument {argument:?}"))
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
