//! When a definition should run, and where that is written down.
//!
//! The tick supplies an **interval** — where the last one stopped, and now —
//! and [`rule`] answers which slots fell in it, purely. Three things follow
//! from carrying the interval rather than the moment:
//!
//! * **Catch-up is free.** A process down from 08:59 to 10:05 ticks once with
//!   `previous = 08:59`, and 09:00 is inside it. No missed-slot table, no
//!   grace window.
//! * **The tick rate stops mattering.** Every minute and every ten minutes
//!   produce the same slots; only the latency of the first differs.
//! * **A replay is a replay.** The same interval always yields the same slots,
//!   so re-processing a tick after a crash cannot invent a run.
//!
//! The tick is **not** on the event log: its only subscriber is this module,
//! and a minute-tick would be half a million records a year that mean nothing
//! to any fold. What is durable is its cursor, in the store's own processor
//! checkpoint, so a restart resumes the interval.
//!
//! [`rule`] is what a cadence means, and is pure. [`store`] is where a schedule
//! lives, over the same `ObjectStore` port the registries use.

pub mod rule;
pub mod slot;
pub mod store;

pub use rule::{Cadence, OverlapPolicy, Schedule, ScheduleError};
pub use slot::{
    SlotAdmission, SlotAdmissionRequest, SlotKey, SlotOutcome, SlotRecord, SlotSettlement,
};
pub use store::{ScheduleReader, ScheduleStore, ScheduledDefinition};
