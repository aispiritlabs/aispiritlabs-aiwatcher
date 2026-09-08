//! Which instants a cadence fires at, inside an interval.
//!
//! Pure: no clock, no store, no socket. The tick supplies the interval and this
//! answers what fell in it — see [the module docs](super) for why that
//! inversion is the whole design.

use serde::{Deserialize, Serialize};
use time::{Date, Duration, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset, Weekday};
use utoipa::ToSchema;

/// How often a definition runs.
///
/// Not a cron expression, and the reason is the one that settled decision 15
/// the other way: a cron parser is a small, closed grammar somebody would have
/// to learn, while this covers what people ask for — hourly, a time each day, a
/// time each week — and is readable in a form. A cron expression can be added
/// later as a fourth variant without changing anything here, because every
/// variant answers the same question: which instants of this cadence fall in an
/// interval.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "every")]
pub enum Cadence {
    /// Every hour, at `minute` past.
    Hourly { minute: u8 },
    /// Every day, at `hour`:`minute` local time.
    Daily { hour: u8, minute: u8 },
    /// One day a week, at `hour`:`minute` local time.
    Weekly { weekday: u8, hour: u8, minute: u8 },
}

/// What to do when a slot comes round and the last one is still going.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OverlapPolicy {
    /// Do not start it. The default, because a curation over a growing corpus
    /// that overlaps itself is two runs writing one dataset version, and the
    /// second is not more up to date than the first in any way anybody asked
    /// for.
    #[default]
    Skip,
    /// Start it anyway. For a definition whose steps are independent of each
    /// other, where a slow run should not cost a slot.
    Allow,
}

/// A definition's schedule, as somebody set it.
///
/// **Mutable, and deliberately not part of the definition's content address.**
/// Changing the hour must not mint a new pipeline revision: that would be a
/// revision history about scheduling, which is the same mistake as making
/// `produced_by` part of a dataset version's identity. It lives as a head
/// beside the definition, the way a prompt's labels live beside its versions.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct Schedule {
    /// A field rather than a flattened one, and not for taste: serde's
    /// `deny_unknown_fields` and `flatten` do not compose, and the bodies here
    /// deny unknown fields so a `cron` somebody hoped would work is named
    /// rather than ignored. Nesting is what keeps that guardrail.
    pub cadence: Cadence,
    /// An IANA name — `Europe/Warsaw`, not an offset.
    ///
    /// An offset is unambiguous and wrong twice a year: somebody who asked for
    /// nine in the morning means nine in the morning in March and in October.
    /// Storing the zone is what lets the answer change when the zone's rules
    /// say it should.
    pub timezone: String,
    /// Off keeps the schedule and starts nothing. Deleting it and re-typing it
    /// next month is how a schedule gets re-typed wrongly.
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub overlap: OverlapPolicy,
}

const fn yes() -> bool {
    true
}

/// What went wrong with a schedule somebody sent.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ScheduleError {
    #[error("{field} is {value}; it must be between 0 and {max}")]
    OutOfRange {
        field: &'static str,
        value: u8,
        max: u8,
    },
    #[error("there is no time zone called {0}; use an IANA name such as Europe/Warsaw or UTC")]
    UnknownZone(String),
}

impl Schedule {
    /// Refuse a schedule that cannot mean anything, before it is stored.
    ///
    /// # Errors
    ///
    /// [`ScheduleError`] naming the field and what it may be.
    pub fn check(&self) -> Result<(), ScheduleError> {
        let (hour, minute, weekday) = match self.cadence {
            Cadence::Hourly { minute } => (0, minute, None),
            Cadence::Daily { hour, minute } => (hour, minute, None),
            Cadence::Weekly {
                weekday,
                hour,
                minute,
            } => (hour, minute, Some(weekday)),
        };
        bounded("hour", hour, 23)?;
        bounded("minute", minute, 59)?;
        if let Some(weekday) = weekday {
            bounded("weekday", weekday, 6)?;
        }
        if zone(&self.timezone).is_none() {
            return Err(ScheduleError::UnknownZone(self.timezone.clone()));
        }
        Ok(())
    }

    /// Whether two versions of a schedule fire at the same instants.
    ///
    /// The three fields that decide *when*, and deliberately not `overlap`,
    /// which decides what happens when a slot comes round rather than whether
    /// one does. The distinction is the whole of review R7's second half: a new
    /// activation moment silently drops a slot that was already due, so it may
    /// only be taken when the rule for producing slots actually changed.
    /// Somebody switching `skip` to `allow` at 08:59 must not lose the nine
    /// o'clock run.
    #[must_use]
    pub fn fires_the_same_as(&self, other: &Self) -> bool {
        self.cadence == other.cadence
            && self.timezone == other.timezone
            && self.enabled == other.enabled
    }

    /// Every instant this schedule fires at, after `previous` and up to `now`.
    ///
    /// Half-open on purpose — `(previous, now]`. A slot exactly at `previous`
    /// was returned by the last call and starting it again would be a second
    /// run of one slot; a slot exactly at `now` is due, and holding it back
    /// until the next tick would make every run late by the tick interval.
    ///
    /// Oldest first, so a catch-up after an outage runs the backlog in the
    /// order it accumulated.
    ///
    /// Returns nothing for a disabled schedule, for an interval that is not
    /// moving forward, and for a zone this build does not know — the last
    /// because `check` refuses one at the door, so reaching here means the
    /// database changed under a stored schedule, and starting nothing is
    /// better than starting something at the wrong hour.
    #[must_use]
    pub fn slots_between(
        &self,
        previous: OffsetDateTime,
        now: OffsetDateTime,
    ) -> Vec<OffsetDateTime> {
        if !self.enabled || now <= previous {
            return Vec::new();
        }
        let Some(zone) = zone(&self.timezone) else {
            return Vec::new();
        };

        // Enumerated by **local calendar date**, never by stepping from
        // `previous`. That is the whole of the fix for review R5: the old walk
        // sampled the zone at `previous`, `previous + step`, … and around a
        // fall-back the same local time exists at two instants, so which of
        // them a sample landed on depended on where the interval had been cut.
        // Daily 02:30 in Warsaw on 2026-10-25 fired once for one call over
        // 00:00Z–02:00Z and twice for the same span in twenty-minute ticks —
        // two instants, two derived ids, two runs of one day's intention.
        //
        // A candidate set that depends only on the local calendar makes the
        // answer a function of the interval and nothing else, which is what
        // `how_often_the_clock_ticks_changes_nothing_about_which_slots_fire`
        // claims and could not previously keep across a transition.
        let mut slots = Vec::new();
        let mut date = local_date(previous, zone) - Duration::days(2);
        // Two days either side, so a candidate whose local day sits at the edge
        // of the interval is still generated whatever the offset does to it.
        let last = local_date(now, zone) + Duration::days(2);
        while date <= last {
            for wall in self.wall_times_on(date) {
                for instant in self.resolve(wall, zone) {
                    if instant > previous && instant <= now && !slots.contains(&instant) {
                        slots.push(instant);
                    }
                }
            }
            let Some(next) = date.next_day() else { break };
            date = next;
        }
        slots.sort_unstable();
        slots
    }

    /// The next instant this schedule fires at, after `now`.
    ///
    /// [`Self::slots_between`] over the smallest span that must contain one —
    /// eight days covers the longest cadence with a day to spare — rather than
    /// a second walk of its own. Anything that answered this differently from
    /// the tick would be a panel showing an hour the run does not happen at.
    ///
    /// `None` for a schedule that is off, which is what "starts nothing" means.
    #[must_use]
    pub fn next_after(&self, now: OffsetDateTime) -> Option<OffsetDateTime> {
        self.slots_between(now, now + Duration::days(8))
            .into_iter()
            .next()
    }

    /// Every wall-clock time this cadence names on one local date.
    ///
    /// Local, and not yet an instant: a wall time is what somebody wrote down,
    /// and turning it into an instant is where a zone gets a say. See
    /// [`Self::resolve`].
    fn wall_times_on(&self, date: Date) -> Vec<PrimitiveDateTime> {
        let (hours, minute) = match self.cadence {
            Cadence::Hourly { minute } => ((0..24).collect::<Vec<u8>>(), minute),
            Cadence::Daily { hour, minute } => (vec![hour], minute),
            Cadence::Weekly {
                weekday,
                hour,
                minute,
            } => {
                if weekday_number(date.weekday()) != weekday {
                    return Vec::new();
                }
                (vec![hour], minute)
            }
        };
        hours
            .into_iter()
            .filter_map(|hour| {
                Time::from_hms(hour, minute, 0)
                    .ok()
                    .map(|time| PrimitiveDateTime::new(date, time))
            })
            .collect()
    }

    /// The instants one wall-clock time happens at, in this zone.
    ///
    /// Usually one. Twice a year a local time is **ambiguous** — the clocks go
    /// back and the hour is lived through twice — or **nonexistent**, in the
    /// hour a spring-forward skips. Both were previously resolved by whichever
    /// offset the sampling instant happened to carry, which is not a policy;
    /// it is the absence of one, and it is why the answer moved with the tick
    /// rate.
    ///
    /// The policy is stated per cadence, because the cadences mean different
    /// things by an hour:
    ///
    /// * **Daily and weekly** name one intention per day. An ambiguous time
    ///   fires at the **first** of its two instants — the clock reached 02:30
    ///   and the run happened — and a nonexistent one fires when the clocks
    ///   **reach** it, which is the transition instant. One run either way,
    ///   which is what "every day at 02:30" asks for.
    /// * **Hourly** counts hours. A repeated hour is genuinely two hours and
    ///   fires twice; an hour the clocks skipped never happens and fires not at
    ///   all. That day has 25 runs and 23 respectively, which is the honest
    ///   answer for "every hour".
    ///
    /// The pair is sorted rather than taken in the order the zone database
    /// hands it back, so "the first" means the earlier instant whatever
    /// `time_tz` calls it.
    fn resolve(&self, wall: PrimitiveDateTime, zone: &'static Tz) -> Vec<OffsetDateTime> {
        use time_tz::OffsetResult;
        use time_tz::PrimitiveDateTimeExt as _;

        let utc = |at: OffsetDateTime| at.to_offset(UtcOffset::UTC);
        match wall.assume_timezone(zone) {
            OffsetResult::Some(at) => vec![utc(at)],
            OffsetResult::Ambiguous(one, other) => {
                let mut both = [utc(one), utc(other)];
                both.sort_unstable();
                match self.cadence {
                    Cadence::Hourly { .. } => both.to_vec(),
                    Cadence::Daily { .. } | Cadence::Weekly { .. } => vec![both[0]],
                }
            }
            OffsetResult::None => match self.cadence {
                Cadence::Hourly { .. } => Vec::new(),
                Cadence::Daily { .. } | Cadence::Weekly { .. } => {
                    clocks_reach(wall, zone).into_iter().collect()
                }
            },
        }
    }
}

/// The first instant whose local time has reached `wall`, for a `wall` the
/// clocks skipped.
///
/// A bisection rather than a scan, and over a window wide enough to hold any
/// zone's offset: local time is strictly increasing across a spring-forward,
/// and the nearest transition in the other direction is six months away, so
/// the predicate is monotone over the twenty-eight hours searched. The answer
/// is the transition instant itself — 02:30 did not happen, and this is the
/// moment the clock passed it.
fn clocks_reach(wall: PrimitiveDateTime, zone: &'static Tz) -> Option<OffsetDateTime> {
    // Whole Unix seconds, not `Duration` halving: a midpoint of two instants
    // carries nanoseconds, and a slot at 01:00:00.549316406Z is one the derived
    // execution id would not match on a re-run, because `execution_id_for`
    // names the slot's `unix_timestamp`. A transition is on a whole minute, so
    // seconds are finer than the answer needs.
    let anchor = wall.assume_utc().unix_timestamp();
    let at = |seconds: i64| OffsetDateTime::from_unix_timestamp(seconds).ok();
    let (mut low, mut high) = (anchor - 14 * 3600, anchor + 14 * 3600);
    // Not a gap this window can answer for. Nothing here may guess.
    if local_wall(at(low)?, zone) >= wall || local_wall(at(high)?, zone) < wall {
        return None;
    }
    while high - low > 1 {
        let middle = low + (high - low) / 2;
        if local_wall(at(middle)?, zone) < wall {
            low = middle;
        } else {
            high = middle;
        }
    }
    at(high)
}

/// What a clock in `zone` reads at this instant, as a wall time.
fn local_wall(at: OffsetDateTime, zone: &'static Tz) -> PrimitiveDateTime {
    use time_tz::OffsetDateTimeExt as _;
    let local = at.to_timezone(zone);
    PrimitiveDateTime::new(local.date(), local.time())
}

/// The local calendar date an instant falls on.
fn local_date(at: OffsetDateTime, zone: &'static Tz) -> Date {
    use time_tz::OffsetDateTimeExt as _;
    at.to_timezone(zone).date()
}

fn bounded(field: &'static str, value: u8, max: u8) -> Result<(), ScheduleError> {
    if value > max {
        return Err(ScheduleError::OutOfRange { field, value, max });
    }
    Ok(())
}

/// `time`'s `Weekday` as the number a form sends. Monday is 0.
const fn weekday_number(day: Weekday) -> u8 {
    day.number_days_from_monday()
}

type Tz = time_tz::Tz;

fn zone(name: &str) -> Option<&'static Tz> {
    time_tz::timezones::get_by_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn daily_at(hour: u8, minute: u8, timezone: &str) -> Schedule {
        Schedule {
            cadence: Cadence::Daily { hour, minute },
            timezone: timezone.to_owned(),
            enabled: true,
            overlap: OverlapPolicy::Skip,
        }
    }

    fn hourly_at(minute: u8, timezone: &str) -> Schedule {
        Schedule {
            cadence: Cadence::Hourly { minute },
            timezone: timezone.to_owned(),
            enabled: true,
            overlap: OverlapPolicy::Skip,
        }
    }

    /// One interval, cut four ways, so a property can say "the tick rate
    /// changed nothing" rather than checking one arbitrary split.
    fn cut_every_way(
        schedule: &Schedule,
        from: OffsetDateTime,
        to: OffsetDateTime,
    ) -> Vec<Vec<OffsetDateTime>> {
        [5_i64, 20, 60, 90]
            .into_iter()
            .map(|minutes| {
                let mut found = Vec::new();
                let mut at = from;
                while at < to {
                    let next = (at + Duration::minutes(minutes)).min(to);
                    found.extend(schedule.slots_between(at, next));
                    at = next;
                }
                found
            })
            .collect()
    }

    fn daily(hour: u8, timezone: &str) -> Schedule {
        Schedule {
            cadence: Cadence::Daily { hour, minute: 0 },
            timezone: timezone.to_owned(),
            enabled: true,
            overlap: OverlapPolicy::Skip,
        }
    }

    #[test]
    fn a_tick_that_spans_the_hour_a_schedule_names_yields_that_one_slot() {
        let slots = daily(9, "UTC").slots_between(
            datetime!(2026-09-08 08:59 UTC),
            datetime!(2026-09-08 09:00 UTC),
        );

        assert_eq!(slots, vec![datetime!(2026-09-08 09:00 UTC)]);
    }

    #[test]
    fn an_outage_across_the_hour_runs_the_slot_it_missed_rather_than_losing_it() {
        // The whole reason the tick carries an interval rather than a moment.
        // A scheduler that asked "is it 09:00 now?" would answer no at 10:05
        // and the day's run would simply not happen, with nothing to say so.
        let slots = daily(9, "UTC").slots_between(
            datetime!(2026-09-08 08:59 UTC),
            datetime!(2026-09-08 10:05 UTC),
        );

        assert_eq!(slots, vec![datetime!(2026-09-08 09:00 UTC)]);
    }

    #[test]
    fn a_long_outage_runs_every_slot_it_missed_oldest_first() {
        // Oldest first so a backlog runs in the order it accumulated, which is
        // the order the windows it curates went past.
        let slots = daily(9, "UTC").slots_between(
            datetime!(2026-09-06 09:30 UTC),
            datetime!(2026-09-09 10:00 UTC),
        );

        assert_eq!(
            slots,
            vec![
                datetime!(2026-09-07 09:00 UTC),
                datetime!(2026-09-08 09:00 UTC),
                datetime!(2026-09-09 09:00 UTC),
            ]
        );
    }

    #[test]
    fn a_slot_already_returned_is_not_returned_again_by_the_next_tick() {
        // Half-open at the near end: `previous` is where the last tick stopped,
        // and a slot exactly there has already been started. Closed at the far
        // end, so a slot at `now` is not held back a whole tick.
        let schedule = daily(9, "UTC");
        let first = schedule.slots_between(
            datetime!(2026-09-08 08:00 UTC),
            datetime!(2026-09-08 09:00 UTC),
        );
        let second = schedule.slots_between(
            datetime!(2026-09-08 09:00 UTC),
            datetime!(2026-09-08 10:00 UTC),
        );

        assert_eq!(first, vec![datetime!(2026-09-08 09:00 UTC)]);
        assert!(second.is_empty(), "{second:?}");
    }

    #[test]
    fn how_often_the_clock_ticks_changes_nothing_about_which_slots_fire() {
        // A property rather than an example: the same span cut into pieces
        // yields the same slots as one call over the whole of it. That is what
        // makes the tick interval an operational choice instead of a
        // correctness one.
        let schedule = daily(9, "UTC");
        let from = datetime!(2026-09-06 00:00 UTC);
        let to = datetime!(2026-09-09 00:00 UTC);

        let whole = schedule.slots_between(from, to);
        let mut pieces = Vec::new();
        let mut at = from;
        while at < to {
            let next = (at + Duration::minutes(37)).min(to);
            pieces.extend(schedule.slots_between(at, next));
            at = next;
        }

        assert_eq!(whole, pieces);
    }

    #[test]
    fn nine_in_the_morning_stays_nine_in_the_morning_after_the_clocks_change() {
        // The reason the zone is an IANA name and not an offset. Warsaw is
        // UTC+2 in September and UTC+1 in November, so a schedule stored as an
        // offset would run at eight one of those months.
        let warsaw = daily(9, "Europe/Warsaw");

        let summer = warsaw.slots_between(
            datetime!(2026-09-08 00:00 UTC),
            datetime!(2026-09-09 00:00 UTC),
        );
        let winter = warsaw.slots_between(
            datetime!(2026-11-10 00:00 UTC),
            datetime!(2026-11-11 00:00 UTC),
        );

        assert_eq!(summer, vec![datetime!(2026-09-08 07:00 UTC)]);
        assert_eq!(winter, vec![datetime!(2026-11-10 08:00 UTC)]);
    }

    #[test]
    fn an_interval_that_crosses_the_change_still_fires_each_day_at_nine() {
        // The case a naive walk gets wrong: stepping a day in UTC from a slot
        // before the change lands an hour out after it, so the second day
        // would fire at ten.
        let slots = daily(9, "Europe/Warsaw").slots_between(
            datetime!(2026-10-24 00:00 UTC),
            datetime!(2026-10-27 00:00 UTC),
        );

        assert_eq!(
            slots,
            vec![
                datetime!(2026-10-24 07:00 UTC),
                datetime!(2026-10-25 08:00 UTC),
                datetime!(2026-10-26 08:00 UTC),
            ]
        );
    }

    #[test]
    fn an_hourly_schedule_fires_once_an_hour_at_the_minute_it_names() {
        let hourly = Schedule {
            cadence: Cadence::Hourly { minute: 30 },
            timezone: "UTC".to_owned(),
            enabled: true,
            overlap: OverlapPolicy::Skip,
        };

        assert_eq!(
            hourly.slots_between(
                datetime!(2026-09-08 09:00 UTC),
                datetime!(2026-09-08 12:00 UTC)
            ),
            vec![
                datetime!(2026-09-08 09:30 UTC),
                datetime!(2026-09-08 10:30 UTC),
                datetime!(2026-09-08 11:30 UTC),
            ]
        );
    }

    #[test]
    fn a_weekly_schedule_fires_on_its_own_day_and_no_other() {
        // 2026-09-08 is a Tuesday; weekday 1 counting Monday from zero.
        let weekly = Schedule {
            cadence: Cadence::Weekly {
                weekday: 1,
                hour: 9,
                minute: 0,
            },
            timezone: "UTC".to_owned(),
            enabled: true,
            overlap: OverlapPolicy::Skip,
        };

        assert_eq!(
            weekly.slots_between(
                datetime!(2026-09-06 00:00 UTC),
                datetime!(2026-09-13 00:00 UTC)
            ),
            vec![datetime!(2026-09-08 09:00 UTC)]
        );
    }

    #[test]
    fn the_next_run_is_the_first_slot_the_tick_would_find() {
        // Answered from `slots_between` rather than from a second walk: a panel
        // that computed this itself would be a second implementation of the
        // rule, free to show an hour the run does not happen at.
        let warsaw = daily(9, "Europe/Warsaw");

        assert_eq!(
            warsaw.next_after(datetime!(2026-09-08 06:00 UTC)),
            Some(datetime!(2026-09-08 07:00 UTC)),
        );
        // Past today's, so tomorrow's.
        assert_eq!(
            warsaw.next_after(datetime!(2026-09-08 08:00 UTC)),
            Some(datetime!(2026-09-09 07:00 UTC)),
        );
    }

    #[test]
    fn a_schedule_that_is_off_has_no_next_run_because_it_starts_nothing() {
        let mut schedule = daily(9, "UTC");
        schedule.enabled = false;

        assert_eq!(schedule.next_after(datetime!(2026-09-08 06:00 UTC)), None);
    }

    #[test]
    fn a_schedule_that_is_off_keeps_its_hour_and_starts_nothing() {
        let mut schedule = daily(9, "UTC");
        schedule.enabled = false;

        assert!(
            schedule
                .slots_between(
                    datetime!(2026-09-08 00:00 UTC),
                    datetime!(2026-09-09 00:00 UTC)
                )
                .is_empty()
        );
        assert_eq!(schedule.cadence, Cadence::Daily { hour: 9, minute: 0 });
    }

    #[test]
    fn an_interval_that_does_not_move_forward_yields_nothing() {
        // A clock that went backwards — an NTP correction, or a checkpoint
        // written by a process whose clock was ahead. Starting the whole day
        // again would be the alternative.
        let schedule = daily(9, "UTC");

        assert!(
            schedule
                .slots_between(
                    datetime!(2026-09-08 12:00 UTC),
                    datetime!(2026-09-08 11:00 UTC)
                )
                .is_empty()
        );
    }

    #[test]
    fn a_schedule_nobody_could_mean_is_refused_by_the_field_that_is_wrong() {
        assert_eq!(
            Schedule {
                cadence: Cadence::Daily {
                    hour: 25,
                    minute: 0
                },
                timezone: "UTC".to_owned(),
                enabled: true,
                overlap: OverlapPolicy::Skip,
            }
            .check(),
            Err(ScheduleError::OutOfRange {
                field: "hour",
                value: 25,
                max: 23
            })
        );
        assert_eq!(
            daily(9, "Europe/Atlantis").check(),
            Err(ScheduleError::UnknownZone("Europe/Atlantis".to_owned()))
        );
        assert!(daily(9, "Europe/Warsaw").check().is_ok());
    }

    #[test]
    fn a_daily_slot_in_the_hour_the_clocks_repeat_fires_once_however_the_tick_is_cut() {
        // Review R5, reproduced exactly. Warsaw goes back an hour at 01:00Z on
        // 2026-10-25, so local 02:30 is lived through twice — at 00:30Z and at
        // 01:30Z. The old walk sampled the zone at `previous + n * step`, so
        // which of the two it found depended on where the interval had been
        // cut: one call over 00:00Z–02:00Z returned 00:30Z, and the same span
        // in twenty-minute ticks returned both. Two instants are two derived
        // ids, so one day's intention ran twice.
        let schedule = daily_at(2, 30, "Europe/Warsaw");
        let (from, to) = (
            datetime!(2026-10-25 00:00 UTC),
            datetime!(2026-10-25 02:00 UTC),
        );

        let once = schedule.slots_between(from, to);
        assert_eq!(
            once,
            vec![datetime!(2026-10-25 00:30 UTC)],
            "the first of the two 02:30s, because a daily schedule is one intention a day"
        );
        for cut in cut_every_way(&schedule, from, to) {
            assert_eq!(cut, once, "the tick rate decided which slots fired");
        }
    }

    #[test]
    fn a_daily_slot_in_the_hour_the_clocks_skip_fires_when_they_reach_it() {
        // The other transition, and the one with no instant to name: Warsaw
        // jumps 02:00 to 03:00 local at 01:00Z on 2026-03-29, so 02:30 never
        // happens. Firing not at all would lose a day's run to a calendar
        // quirk; this fires at the moment the clock passed it.
        let schedule = daily_at(2, 30, "Europe/Warsaw");
        let (from, to) = (
            datetime!(2026-03-29 00:00 UTC),
            datetime!(2026-03-29 04:00 UTC),
        );

        let once = schedule.slots_between(from, to);
        assert_eq!(
            once,
            vec![datetime!(2026-03-29 01:00 UTC)],
            "the transition instant itself — 03:00 local, the first reading past 02:30"
        );
        for cut in cut_every_way(&schedule, from, to) {
            assert_eq!(cut, once, "the tick rate decided which slots fired");
        }
    }

    #[test]
    fn a_slot_the_clocks_skipped_is_a_whole_second() {
        // `execution_id_for` names the slot's `unix_timestamp`, so a slot
        // carrying nanoseconds from the search that found it would derive one
        // id on the tick that started it and another on a replay.
        let slot = daily_at(2, 30, "Europe/Warsaw")
            .slots_between(
                datetime!(2026-03-29 00:00 UTC),
                datetime!(2026-03-29 04:00 UTC),
            )
            .into_iter()
            .next()
            .expect("the slot the clocks reached");
        assert_eq!(slot.nanosecond(), 0, "{slot} is not a whole second");
    }

    #[test]
    fn an_hourly_schedule_lives_the_repeated_hour_twice_and_the_skipped_one_not_at_all() {
        // The policy is stated per cadence, because the cadences mean different
        // things by an hour. "Every hour" counts hours: a day with 25 of them
        // has 25 runs and a day with 23 has 23. "Every day at 02:30" is one
        // intention whatever the clocks do, which the two tests above cover.
        let schedule = hourly_at(30, "Europe/Warsaw");

        assert_eq!(
            schedule.slots_between(
                datetime!(2026-10-25 00:00 UTC),
                datetime!(2026-10-25 02:00 UTC)
            ),
            vec![
                datetime!(2026-10-25 00:30 UTC),
                datetime!(2026-10-25 01:30 UTC)
            ],
            "02:30 local happened twice and is two hours"
        );
        assert_eq!(
            schedule.slots_between(
                datetime!(2026-03-29 00:00 UTC),
                datetime!(2026-03-29 02:00 UTC)
            ),
            vec![
                datetime!(2026-03-29 00:30 UTC),
                datetime!(2026-03-29 01:30 UTC)
            ],
            "01:30 and 03:30 local; 02:30 never happened, so it is not an hour"
        );
    }

    #[test]
    fn the_hour_the_panel_shows_is_the_hour_the_tick_fires_at_across_a_change() {
        // The guardrail in its own words: a second implementation would have
        // its own idea of when the clocks change, and the first hour it
        // disagreed on would be one somebody planned a morning around.
        for schedule in [
            daily_at(2, 30, "Europe/Warsaw"),
            daily_at(9, 0, "Europe/Warsaw"),
            daily_at(0, 30, "America/New_York"),
        ] {
            for before in [
                datetime!(2026-10-24 12:00 UTC),
                datetime!(2026-03-28 12:00 UTC),
            ] {
                let next = schedule.next_after(before).expect("a next run");
                assert_eq!(
                    schedule.slots_between(before, next),
                    vec![next],
                    "next_after and the tick disagreed about {next}"
                );
            }
        }
    }

    #[test]
    fn cutting_an_interval_that_crosses_either_change_yields_the_same_slots() {
        // The property the two reproductions above are instances of, over both
        // transitions and every cadence — including the southern hemisphere,
        // where the two happen the other way round in the year.
        for zone in ["Europe/Warsaw", "America/New_York", "Australia/Sydney"] {
            for schedule in [
                daily_at(2, 30, zone),
                daily_at(9, 0, zone),
                hourly_at(30, zone),
                Schedule {
                    cadence: Cadence::Weekly {
                        weekday: 6,
                        hour: 2,
                        minute: 30,
                    },
                    timezone: zone.to_owned(),
                    enabled: true,
                    overlap: OverlapPolicy::Skip,
                },
            ] {
                for (from, to) in [
                    (
                        datetime!(2026-10-24 12:00 UTC),
                        datetime!(2026-10-26 12:00 UTC),
                    ),
                    (
                        datetime!(2026-03-28 12:00 UTC),
                        datetime!(2026-03-30 12:00 UTC),
                    ),
                    (
                        datetime!(2026-04-04 12:00 UTC),
                        datetime!(2026-04-06 12:00 UTC),
                    ),
                ] {
                    let once = schedule.slots_between(from, to);
                    for cut in cut_every_way(&schedule, from, to) {
                        assert_eq!(
                            cut, once,
                            "{zone} {:?} between {from} and {to}",
                            schedule.cadence
                        );
                    }
                }
            }
        }
    }
}
