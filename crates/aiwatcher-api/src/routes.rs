//! The composition root.
//!
//! This file names no handler. Every area of the API is a module with a
//! facade — a `router()` and an `openapi()` — and what happens here is that
//! the routers are merged and the authentication layer is put in front of all
//! of them at once.
//!
//! That the layer wraps the whole router rather than individual routes is the
//! guardrail, not a convenience. A route added later is authenticated by
//! default instead of by somebody remembering to say so, and the one somebody
//! forgets is not the one that leaks. The exceptions live in
//! [`auth::is_public`](crate::auth) and are the health probes and the sign-in
//! routes, which cannot require a session in order to establish one.
//!
//! What the layer does *not* decide is whether a caller may perform the
//! operation. That is a `Role` check in the handler, because the answer
//! differs per handler and a table of paths in a middleware drifts from the
//! routes it guards.

use axum::Router;

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        // ── Observability: folded from the event log ─────────────────────────
        .merge(crate::runs::router())
        .merge(crate::metrics::router())
        .merge(crate::evaluations::router())
        .merge(crate::live::router())
        .merge(crate::ingest::router())
        .merge(crate::health::router())
        // ── Authored artifacts: one object store, four prefixes ──────────────
        //
        // A different store with a different lifetime from the log, which is
        // why they are not folded from it and why they answer 501 rather than
        // 404 when none is configured. See ADR_0011, ADR_0014, ADR_0017.
        .merge(crate::prompts::router())
        .merge(crate::datasets::router())
        .merge(crate::annotations::router())
        // Training is the one of the four whose contents never came from the
        // log at all: a run is a record that grows in place. See ADR_0018.
        .merge(crate::training::router())
        // ── Asking another system to do something ────────────────────────────
        .merge(crate::workflows::router())
        .merge(crate::engine::router())
        // ── Reaching a service aiwatcher does not run ────────────────────────
        .merge(crate::integrations::hubs::router())
        // ── The caller, rather than the data ─────────────────────────────────
        .merge(crate::auth::router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::auth::authenticate,
        ))
        .with_state(state)
}
