//! The durable log, behind a port.
//!
//! Laser is the intended backbone. Nothing above this crate names it: the
//! pipeline talks to [`MessageSource`], [`MessageSink`] and [`Checkpointer`],
//! and which log is behind them is a wiring decision. That indirection is what
//! lets the default build — and every test that does not need a broker — stay
//! free of the dependency entirely.
//!
//! Four adapters ship:
//!
//! * [`adapters::memory`] — everything in a `Vec`. Tests and `just dev`.
//! * [`adapters::wal`] — an append-only JSONL file. Single node, survives a
//!   restart, no broker to run. The default, and what makes the system usable
//!   without standing anything up.
//! * [`adapters::laser`] — the real `laser_sdk` over Apache Iggy. Behind the
//!   `laser` feature, because it pulls ~360 crates and needs a broker to test.
//! * [`adapters::broker`] — a generic poll/commit adapter over a four-method
//!   trait. What a Kafka or NATS backend would implement, and the one the
//!   contract test can drive without a broker.

pub mod adapters;
pub mod ports;

pub use ports::{
    AppendResult, BusError, BusResult, Checkpointer, MessageSink, MessageSource, SourceMessage,
    StartFrom, SubscribeOptions,
};
