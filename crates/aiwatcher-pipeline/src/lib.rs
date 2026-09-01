//! Pipeline engines, behind `core::engine::WorkflowEngine`.
//!
//! Two things aiwatcher cannot learn from its own log: what a cluster is able
//! to run, and what inputs that thing declares. Nothing publishes an event
//! about a workflow nobody has started yet, and an input interface is not
//! visible in a workflow's output. Both live in the orchestrator, so both are
//! read from it — and nothing else here is.
//!
//! ADR_0016 has the whole argument; the short form is that this reads an
//! orchestrator's **inventory**, never its history. The shape of a graph still
//! comes from `workflow.declared` on the log (ADR_0012), because that is the
//! source that is still right when the orchestrator is bypassed — which
//! planner does whenever `settings.flyte_enabled` is off.
//!
//! ## Why the crate is not called `aiwatcher-flyte`
//!
//! [`flyte`] is the only engine here today and the port has one shape, which
//! is the moment a name gets decided badly. The thing being modelled is the
//! feature/training/inference cycle's execution layer: a catalog of runnable
//! entities, their declared inputs, and starting one. Flyte is an
//! implementation of that, in the same way Laser is an implementation of
//! `MessageSource` — and `aiwatcher-bus` is not called `aiwatcher-iggy`.
//!
//! ## What an engine adapter may and may not do
//!
//! It may list, read and start. It may not be told where to point: the
//! endpoint is configuration, exactly as the rerun target is, because
//! aiwatcher runs inside the cluster and a caller-supplied URL is a request to
//! reach that cluster's network on the caller's behalf.

pub mod flyte;
mod literals;

pub use flyte::{FlyteConfig, FlyteEngine, RUN_ID_INPUTS, RUN_ID_LABEL};
