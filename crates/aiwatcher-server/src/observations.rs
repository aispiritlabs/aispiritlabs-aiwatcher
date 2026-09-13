//! What variants were observed doing, written down period by period as each
//! closes, so a window reaching past the read model still has an answer.
//!
//! The serve role holds the read model, so it is the one that writes: every
//! tick, each closed period back to a bound — two days, or 500 periods — that
//! is not written yet and that the fold can vouch for. A period closes a tenth
//! of its width after its end, at most five minutes, so an end event that
//! arrives a little late still lands in it; one later than that is in no
//! written period. See `aiwatcher_projector::periods`.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use aiwatcher_core::ports::PortError;
use aiwatcher_projector::{PeriodStore, ReadModel};
use time::OffsetDateTime;

/// How far back a pass looks for a period nobody wrote.
const LOOKBACK: Duration = Duration::from_secs(48 * 3_600);
const MOST_PERIODS: i64 = 500;

/// Start the writer, when there is somewhere to write.
#[must_use]
pub fn spawn(
    state: &aiwatcher_api::state::AppState,
    config: &crate::config::Config,
    shutdown: tokio_util::sync::CancellationToken,
) -> Option<tokio::task::JoinHandle<()>> {
    let store = state.observation_periods.clone()?;
    let read_model = Arc::clone(&state.read_model);
    let width = i64::try_from(config.observation_period.as_secs())
        .unwrap_or(3_600)
        .max(1);
    Some(tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(Duration::from_secs(width.clamp(1, 60).unsigned_abs()));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut known = BTreeSet::new();
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                _ = interval.tick() => {
                    match pass(&store, &read_model, width, OffsetDateTime::now_utc(), &mut known).await {
                        Ok(0) => {}
                        Ok(written) => tracing::debug!(written, "observed periods written"),
                        Err(error) => tracing::warn!(%error, "observed periods could not be written; the next pass tries again"),
                    }
                }
            }
        }
    }))
}

/// One pass: every closed period in the bound that is not written and that the
/// read model vouches for, oldest first. `known` remembers what is written, so
/// a pass asks the store about each period once.
///
/// # Errors
///
/// The store's own failure; what was written before it stays written.
pub async fn pass(
    store: &PeriodStore,
    read_model: &ReadModel,
    width: i64,
    now: OffsetDateTime,
    known: &mut BTreeSet<i64>,
) -> Result<usize, PortError> {
    let width = width.max(1);
    let margin = (width / 10).clamp(1, 300);
    let closed = (now.unix_timestamp() - margin).div_euclid(width) * width;
    let reach =
        (i64::try_from(LOOKBACK.as_secs()).unwrap_or(i64::MAX) / width).clamp(1, MOST_PERIODS);
    let oldest = closed - reach * width;
    known.retain(|from| *from >= oldest);
    let mut written = 0;
    let mut from = oldest;
    while from + width <= closed {
        let to = from + width;
        if !known.contains(&from) {
            if store.written(from, to).await? {
                known.insert(from);
            } else if let (Ok(start), Ok(end)) = (
                OffsetDateTime::from_unix_timestamp(from),
                OffsetDateTime::from_unix_timestamp(to),
            ) && let Some(records) = read_model.observed_period(start, end).await
            {
                if store
                    .write(from, to, &records, now.unix_timestamp())
                    .await?
                {
                    written += 1;
                }
                known.insert(from);
            }
        }
        from = to;
    }
    Ok(written)
}
