//! Configuration and wiring.
//!
//! Everything below this crate is decided by a trait. This is where the traits
//! get their implementations, which makes it the only place that knows Laser,
//! VictoriaTraces and axum all exist.

pub mod config;
pub mod conversations;
pub mod execution;
pub mod iam;
pub mod imports;
pub mod seed;
pub mod wiring;

pub use config::{BackendKind, Config, ConfigError};
pub use wiring::{JournalTask, Runtime, build, build_journal};

pub mod evaluation;
