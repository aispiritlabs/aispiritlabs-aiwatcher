//! One time window, shared by every list.
//!
//! Every list in the panel answers "what happened recently", and until this
//! only the metrics page could say how recently — everything else showed the
//! whole retention window and left the reader to date the rows themselves.
//!
//! The window is **relative**: seconds back from now, resolved on the server.
//! A pair of timestamps would have been more expressive and worse to share — a
//! link someone pastes into a chat should mean "the last hour" when it is
//! opened, not the hour it was copied.
//!
//! ## `as_of`, for the caller that has to pin one
//!
//! That reasoning argued against *replacing* the relative window, and it also
//! said absolute bounds "belong on a route that pins an incident, and there is
//! not one yet". There is one now: a managed execution pins a run, and a step
//! that reads "the last hour" has to read the same hour when it is retried five
//! minutes later — or the rows it produced are not the rows its cache key
//! claims (section 43.15).
//!
//! So the window keeps its shape and gains an optional *end*. Absent, it is
//! now, and every link still means what it meant. Present, the window is a
//! closed span and two reads of it agree.
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
    bounds(window_seconds, None, now).since
}

/// Both ends of a window: where it starts, and where it stops.
///
/// `until` is `None` for every ordinary read, which is what keeps a relative
/// window relative — a row that arrived a second ago is in "the last hour"
/// because there is no upper bound to be past.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bounds {
    pub since: Option<OffsetDateTime>,
    pub until: Option<OffsetDateTime>,
}

impl Bounds {
    /// Whether an instant is inside. Both ends are inclusive: a window is a
    /// period somebody named, and excluding its last second would drop the row
    /// that closed the incident.
    #[must_use]
    pub fn holds(&self, at: OffsetDateTime) -> bool {
        self.since.is_none_or(|since| at >= since) && self.until.is_none_or(|until| at <= until)
    }
}

/// Seconds since the epoch, as an instant a filter can use.
///
/// `None` for an absent value and for one no calendar has — a caller that sent
/// nonsense gets the ordinary relative window rather than an error, which is
/// the same direction [`bounds`] treats a negative width in.
#[must_use]
pub fn at(seconds: Option<i64>) -> Option<OffsetDateTime> {
    seconds.and_then(|seconds| OffsetDateTime::from_unix_timestamp(seconds).ok())
}

/// The window `window_seconds` names, ending at `as_of` or at `now`.
///
/// `as_of` alone — a window end with no width — bounds the future and nothing
/// else, which is what somebody asking "everything up to the incident" means.
#[must_use]
pub fn bounds(
    window_seconds: Option<i64>,
    as_of: Option<OffsetDateTime>,
    now: OffsetDateTime,
) -> Bounds {
    let end = as_of.unwrap_or(now);
    Bounds {
        since: window_seconds
            .filter(|seconds| *seconds > 0)
            .map(|seconds| end - time::Duration::seconds(seconds)),
        // Only when it was asked for. Defaulting it to `now` would be the same
        // answer for every ordinary read and one more comparison per row.
        until: as_of,
    }
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
    fn a_pinned_end_makes_the_window_a_closed_span_two_reads_agree_on() {
        // The reason `as_of` exists: a managed step that read "the last hour"
        // has to read the same hour when it is retried five minutes later, or
        // the rows it produced are not the rows its cache key claims.
        let pinned = bounds(
            Some(3600),
            Some(datetime!(2026-08-29 10:00:00 UTC)),
            datetime!(2026-08-29 10:05:00 UTC),
        );
        assert_eq!(pinned.since, Some(datetime!(2026-08-29 09:00:00 UTC)));
        assert_eq!(pinned.until, Some(datetime!(2026-08-29 10:00:00 UTC)));
        assert!(pinned.holds(datetime!(2026-08-29 09:30:00 UTC)));
        assert!(
            !pinned.holds(datetime!(2026-08-29 10:02:00 UTC)),
            "a row that arrived after the span is not in it, however recent it is"
        );

        // And the same window with no end is the ordinary relative one, whose
        // upper bound is absent rather than `now`.
        let relative = bounds(Some(3600), None, datetime!(2026-08-29 10:05:00 UTC));
        assert_eq!(relative.since, Some(datetime!(2026-08-29 09:05:00 UTC)));
        assert_eq!(relative.until, None);
        assert!(relative.holds(datetime!(2026-08-29 10:05:00 UTC)));
    }

    #[test]
    fn an_end_with_no_width_bounds_the_future_and_nothing_else() {
        let up_to = bounds(
            None,
            Some(datetime!(2026-08-29 10:00:00 UTC)),
            datetime!(2026-08-29 12:00:00 UTC),
        );
        assert_eq!(up_to.since, None);
        assert!(up_to.holds(datetime!(2020-01-01 00:00:00 UTC)));
        assert!(!up_to.holds(datetime!(2026-08-29 11:00:00 UTC)));
    }

    #[test]
    fn a_negative_window_is_ignored_rather_than_reaching_into_the_future() {
        assert_eq!(cutoff(Some(-60), datetime!(2026-08-29 10:00:00 UTC)), None);
    }
}
