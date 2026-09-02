//! What this crate reaches *out* to.
//!
//! One module, because there is exactly one kind of thing here and it is the
//! kind worth being able to find: every other slice answers from the object
//! store, and these leave the building.
//!
//! That distinction is not stylistic. An outbound call has a timeout, a
//! credential, a partial-failure story and a third party whose answers are
//! data rather than truth — and [`hubs`] is the sharpest case of the last one:
//! a public dataset hub is asked what exists and is never asked what a licence
//! permits. See ADR_0019, and [`crate::license`] for the rule it must not
//! override.

pub mod hubs;
