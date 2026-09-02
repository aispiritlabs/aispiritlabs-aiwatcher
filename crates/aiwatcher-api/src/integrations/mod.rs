//! Routes that read a service aiwatcher does not run.
//!
//! Grouped for the same reason [`aiwatcher_annotations::integrations`] is, and
//! it is the one grouping in this crate that is not about a product area:
//! every other module here answers from the log, the read model or the object
//! store. These leave the building, and a reader of `routes::router` should be
//! able to see which ones do at a glance.
//!
//! The guardrail itself lives in the domain crate rather than here, so that a
//! second caller cannot route around it. What this layer adds is the 501 — a
//! hub nobody configured is not an empty result.

pub mod hubs;
