//! The aiwatcher server.
//!
//! One process runs both the projector and the HTTP API. They are separate
//! crates and could be separate deployments — the projector scales by consumer
//! group, the API by replica — but a single binary is the right starting point:
//! the live hub is then an in-process channel rather than another network hop.

use std::time::Duration;

use anyhow::{Context, Result};
use tokio_util::sync::CancellationToken;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use aiwatcher_server::config::{Config, LogFormat};

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::from_env().context("reading configuration")?;
    init_tracing(config.log_format);

    tracing::info!(
        listen = %config.listen,
        bus = ?config.bus,
        otlp = config.otlp_endpoint.as_deref().unwrap_or("<none>"),
        ingest_enabled = config.ingest_enabled,
        auth = config.auth.mode.as_str(),
        "starting aiwatcher"
    );

    let runtime = aiwatcher_server::build(config).await?;
    let (state, config, projector) = runtime.split();

    let shutdown = CancellationToken::new();
    let projector_task = {
        let shutdown = shutdown.clone();
        tokio::spawn(async move { projector.run(shutdown).await })
    };

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
    match tokio::time::timeout(Duration::from_secs(30), projector_task).await {
        Ok(Ok(Ok(()))) => tracing::info!("projector drained cleanly"),
        Ok(Ok(Err(error))) => tracing::error!(%error, "projector stopped with an error"),
        Ok(Err(error)) => tracing::error!(%error, "projector task panicked"),
        Err(_) => tracing::warn!("projector did not drain within 30s; exiting anyway"),
    }

    Ok(())
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
