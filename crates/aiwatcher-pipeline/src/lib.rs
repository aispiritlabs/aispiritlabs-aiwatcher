//! Pipeline engines, behind `core::engine::WorkflowEngine`.
//!
//! Two things aiwatcher cannot learn from its own log: what a cluster is able
//! to run, and what inputs that thing declares. Nothing publishes an event
//! about a workflow nobody has started, and an interface is not visible in a
//! workflow's output. Both are read from the orchestrator — and nothing else
//! here is. The *shape* of a graph still comes from `workflow.declared`,
//! because that is the source that is right when the orchestrator is bypassed.
//!
//! The crate is not `aiwatcher-flyte` for the reason `aiwatcher-bus` is not
//! `aiwatcher-iggy`: the thing modelled is a catalog of runnable entities,
//! their declared inputs, and starting one. [`flyte`] is one implementation.
//!
//! An adapter may list, read and start. It may not be told where to point —
//! the endpoint is configuration, because aiwatcher runs inside the cluster and
//! a caller-supplied URL is a request to reach that network on its behalf.
//!
//! ADR_0012, ADR_0016.

pub mod flyte;
mod literals;

pub use flyte::{FlyteConfig, FlyteEngine, RUN_ID_INPUTS, RUN_ID_LABEL};
