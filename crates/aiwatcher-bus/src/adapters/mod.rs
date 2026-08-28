//! Log adapters. Pick one at wiring time; nothing above this module cares.

pub mod broker;
#[cfg(feature = "laser")]
pub mod laser;
pub mod memory;
pub mod wal;
