//! One time window, shared by every list.
//!
//! Every list in the panel answers "what happened recently", and until this
//! only the metrics page could say how recently — everything else showed the
//! whole retention window and left the reader to date the rows themselves.
//!
//! The window is **relative**: seconds back from now, resolved on the server.
//! A pair of timestamps would have been more expressive and worse to share — a
//! link someone pastes into a chat should mean "the last hour" when it is
//! opened, not the hour it was copied. Absolute bounds belong on a route that
//! pins an incident, and there is not one yet.
//!
//! `None` and `Some(0)` both mean everything, so a panel can send its "all"
//! preset as a zero rather than as an absent parameter and keep one shape.
//!
//! ## What the window matches
//!
//! Every list here windows on **last activity**, not on start: a run that
//! began three hours ago and emitted an event a minute ago is something
//! happening now, and dropping it from the last-15-minutes view would hide
//! exactly the long run someone is looking for. [`crate::metrics`] is the one
//! exception and keeps windowing by start, because there the window is the
//! timeline's x-axis rather than a filter — a run that started before the axis
//! has no bucket to be counted in.

use time::OffsetDateTime;

/// The oldest instant a row may be active at and still be included.
///
/// `None` means no bound: everything the read model still holds.
#[must_use]
pub fn cutoff(window_seconds: Option<i64>, now: OffsetDateTime) -> Option<OffsetDateTime> {
    window_seconds
        .filter(|seconds| *seconds > 0)
        .map(|seconds| now - time::Duration::seconds(seconds))
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    #[test]
    fn a_window_of_zero_seconds_means_everything_rather_than_nothing() {
        assert_eq!(cutoff(Some(0), datetime!(2026-08-29 10:00:00 UTC)), None);
        assert_eq!(cutoff(None, datetime!(2026-08-29 10:00:00 UTC)), None);
    }

    #[test]
    fn a_window_is_measured_back_from_now() {
        assert_eq!(
            cutoff(Some(3600), datetime!(2026-08-29 10:00:00 UTC)),
            Some(datetime!(2026-08-29 09:00:00 UTC)),
        );
    }

    #[test]
    fn a_negative_window_is_ignored_rather_than_reaching_into_the_future() {
        assert_eq!(cutoff(Some(-60), datetime!(2026-08-29 10:00:00 UTC)), None);
    }
}
