//! Which instants a cadence fires at, inside an interval.
//!
//! Pure: no clock, no store, no socket. The tick supplies the interval and this
//! answers what fell in it — see [the module docs](super) for why that
//! inversion is the whole design.

use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime, Time, UtcOffset, Weekday};
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

        // Walked in local time, because that is what the schedule is written
        // in: "nine each day" is nine after the clocks change as well as
        // before, and an interval walked in UTC would drift by an hour.
        let mut slots = Vec::new();
        let mut at = previous;
        // A day either side of the interval, so a slot whose local time lands
        // inside it is still found when the offset moves.
        let last = now + Duration::days(1);
        while at <= last {
            if let Some(candidate) = self.local_slot_at(at, zone)
                && candidate > previous
                && candidate <= now
                && !slots.contains(&candidate)
            {
                slots.push(candidate);
            }
            at += self.step();
            // A schedule whose step is zero would spin. `check` cannot produce
            // one, and this is what makes that true rather than assumed.
            if self.step().is_zero() {
                break;
            }
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

    /// How far to walk between candidates. One cadence, one stride.
    const fn step(&self) -> Duration {
        match self.cadence {
            Cadence::Hourly { .. } => Duration::hours(1),
            Cadence::Daily { .. } | Cadence::Weekly { .. } => Duration::days(1),
        }
    }

    /// The instant this schedule fires on the local day `at` falls in, if it
    /// fires that day at all.
    fn local_slot_at(&self, at: OffsetDateTime, zone: &'static Tz) -> Option<OffsetDateTime> {
        let local = at.to_offset(offset_at(zone, at));
        let (hour, minute) = match self.cadence {
            Cadence::Hourly { minute } => (local.hour(), minute),
            Cadence::Daily { hour, minute } => (hour, minute),
            Cadence::Weekly {
                weekday,
                hour,
                minute,
            } => {
                if weekday_number(local.weekday()) != weekday {
                    return None;
                }
                (hour, minute)
            }
        };
        let wall = local.replace_time(Time::from_hms(hour, minute, 0).ok()?);
        // The offset is resolved for the *wall clock* moment rather than kept
        // from `at`: an interval that crosses a change would otherwise place
        // every later slot an hour out. A local time that does not exist —
        // the hour a spring-forward skips — resolves to the instant the clocks
        // reach, which fires once rather than not at all.
        Some(
            wall.replace_offset(offset_at(zone, wall))
                .to_offset(UtcOffset::UTC),
        )
    }
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

fn offset_at(zone: &'static Tz, at: OffsetDateTime) -> UtcOffset {
    use time_tz::OffsetDateTimeExt as _;
    at.to_timezone(zone).offset()
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

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
}
