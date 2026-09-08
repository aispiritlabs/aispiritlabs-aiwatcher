//! When a definition should run, and where that is written down.
//!
//! The shape is Oskar Dudycz's `PassageOfTimeJob`, and what is taken from it is
//! the *inversion*: a scheduler that publishes a fact — "time has passed, from
//! here to here" — rather than one that decides what to start. Everything that
//! decides is then a pure function of `(schedules, previous..now)`, testable
//! with no clock, no store and no socket, exactly like [`crate::decide`].
//!
//! Three things follow from carrying the *interval* rather than the moment, and
//! all three are properties this system would otherwise have had to build:
//!
//! * **Catch-up is free.** A process down from 08:59 to 10:05 ticks once with
//!   `previous = 08:59`, and the 09:00 slot is in that interval. There is no
//!   missed-slot table and no grace window to tune.
//! * **The tick rate stops mattering.** Ticking every minute and every ten
//!   minutes produce the same slots; only the latency of the first one differs.
//!   `how_often_the_clock_ticks_changes_nothing_about_which_slots_fire` is what
//!   keeps that true.
//! * **A replay is a replay.** The same interval always yields the same slots,
//!   so re-processing a tick after a crash cannot invent a run.
//!
//! ## What is deliberately not taken
//!
//! The reference publishes `MinuteHasPassed` onto the event bus, so anything
//! may subscribe. Here the only subscriber would be this module, and the event
//! log is the durable one every projector folds — a minute-tick on it is five
//! hundred thousand records a year that mean nothing to any of them, which is
//! the flooding [`crate::facts`]' guardrail is about. So the tick stays in
//! process, and what is durable is its *cursor*: `previous` comes from the
//! store's own processor checkpoint, so a restart resumes the interval rather
//! than losing it.
//!
//! Quartz is not taken either. It is a scheduler library for the case where
//! schedules are the product; the work role already runs three interval loops
//! and this is a fourth.
//!
//! ## Two files, for the reason `artifact` next door has three
//!
//! [`rule`] is what a cadence *means* and is pure. [`store`] is where a
//! schedule is written down, over the same `ObjectStore` port the five
//! registries and the artifact catalog already use. A change to what "daily at
//! nine" means touches the first; a change to where it lives touches the
//! second.

pub mod rule;
pub mod store;

pub use rule::{Cadence, OverlapPolicy, Schedule, ScheduleError};
pub use store::{FiringOutcome, LastFiring, ScheduleStore, ScheduledDefinition};
