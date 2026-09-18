//! The two background jobs the IAM control plane's audit trail needs.
//!
//! **The export worker** is what makes an export asynchronous rather than a
//! request somebody holds open. It picks up whatever is queued — including a
//! job an earlier process left `running` when it died, which is the whole
//! reason the cursor is durable — and runs it a shard at a time.
//!
//! **The retention sweep** is what makes the trail's clock real. A retention
//! policy nothing enforces is a paragraph, and the difference between the two
//! is a loop that runs every hour and usually finds nothing.
//!
//! They are one task, because they share a shutdown and neither is busy. Both
//! are safe in several replicas at once: a job another worker holds is skipped,
//! one whose lease was taken over mid-export stops itself, and a sweep that
//! finds nothing left to remove removes nothing.
//!
//! **The sweep runs in the `serve` role**, beside the archive's, because the IAM
//! store is what that role opens. It is deliberately not reachable over HTTP —
//! an administrator who could delete the record of their own administration is
//! the one capability an audit trail must not grant — so a deployment turns it
//! on with `AIWATCHER_IAM_AUDIT_RETENTION_DAYS` and nothing else does.

use std::sync::Arc;

use aiwatcher_api::state::AppState;
use aiwatcher_iam::{AuditExports, AuditRetention, IamStore, JobState};
use time::OffsetDateTime;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::config::Config;

/// Start the worker, if this deployment has anything for it to do.
///
/// `None` when there is no IAM store — which is the default — and also when
/// there is one but neither an object store to export into nor a retention to
/// apply, because a task that would find nothing to do forever is worse than no
/// task.
#[must_use]
pub fn spawn(
    state: &AppState,
    config: &Config,
    shutdown: CancellationToken,
) -> Option<JoinHandle<()>> {
    let iam = Arc::clone(state.iam.as_ref()?);
    let exports = state.iam_audit_exports.clone();
    let retention = retention(config)?;
    if exports.is_none() && retention.is_none() {
        tracing::info!(
            "the IAM control plane keeps its audit trail whole and exports nothing: no object \
             store and no AIWATCHER_IAM_AUDIT_RETENTION_DAYS"
        );
        return None;
    }
    let notify = Arc::clone(state.iam_audit_worker.as_ref()?);
    let poll = config.iam_audit_export_poll;
    let sweep = config.iam_audit_sweep_interval;
    let worker = crate::conversations::worker_id();
    match &retention {
        Some(policy) => tracing::info!(
            ttl_days = policy.ttl_days,
            policy_id = %policy.policy_id,
            sweep_seconds = sweep.as_secs(),
            exports = exports.is_some(),
            %worker,
            "the IAM audit trail has a retention and a sweep to apply it"
        ),
        None => tracing::info!(
            exports = exports.is_some(),
            %worker,
            "IAM audit exports are on; nothing removes an entry"
        ),
    }
    Some(tokio::spawn(async move {
        run(
            iam, exports, retention, notify, poll, sweep, worker, shutdown,
        )
        .await;
    }))
}

/// The deployment's clock, or `None` when it has not set one.
///
/// A `Some` that will not validate is a start-up failure everywhere else here;
/// this one cannot be, because the configuration layer already refuses a zero
/// and nothing else can fail. A retention past the crate's ceiling is reported
/// and dropped rather than silently clamped: clamping would apply a policy
/// nobody wrote down.
fn retention(config: &Config) -> Option<Option<AuditRetention>> {
    let Some(days) = config.iam_audit_retention_days else {
        return Some(None);
    };
    match AuditRetention::new(days, config.iam_audit_policy_id.clone()) {
        Ok(policy) => Some(Some(policy)),
        Err(error) => {
            tracing::error!(
                %error,
                "AIWATCHER_IAM_AUDIT_RETENTION_DAYS was refused; no audit entry will be removed"
            );
            Some(None)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run(
    iam: Arc<dyn IamStore>,
    exports: Option<Arc<AuditExports>>,
    retention: Option<AuditRetention>,
    notify: Arc<tokio::sync::Notify>,
    poll: std::time::Duration,
    sweep: std::time::Duration,
    worker: String,
    shutdown: CancellationToken,
) {
    let mut poll_tick = tokio::time::interval(poll);
    let mut sweep_tick = tokio::time::interval(sweep);
    // `Delay` rather than `Burst`: a missed tick during a long export should not
    // produce a run of catch-up ticks the moment it finishes.
    poll_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    sweep_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            () = shutdown.cancelled() => {
                tracing::info!("the IAM audit worker is stopping");
                return;
            }
            () = notify.notified() => {
                drain(iam.as_ref(), exports.as_deref(), &worker, &shutdown).await;
            }
            _ = poll_tick.tick() => {
                drain(iam.as_ref(), exports.as_deref(), &worker, &shutdown).await;
            }
            _ = sweep_tick.tick() => prune(iam.as_ref(), retention.as_ref()).await,
        }
    }
}

/// Run every export that is waiting, oldest first.
///
/// The list is re-read after each job rather than taken once: a job queued while
/// this one was running should not wait for the next tick, and one somebody
/// cancelled in the meantime should not be started.
async fn drain(
    iam: &dyn IamStore,
    exports: Option<&AuditExports>,
    worker: &str,
    shutdown: &CancellationToken,
) {
    let Some(exports) = exports else {
        return;
    };
    loop {
        if shutdown.is_cancelled() {
            return;
        }
        let waiting = match exports.claimable(OffsetDateTime::now_utc()).await {
            Ok(waiting) => waiting,
            Err(error) => {
                tracing::warn!(%error, "cannot list IAM audit export jobs");
                return;
            }
        };
        let Some((organization, job_id)) = waiting.into_iter().next() else {
            return;
        };
        match exports.run(iam, organization, &job_id, worker).await {
            // Somebody else claimed it between the listing and the call. Not a
            // failure, and not finished either — the next listing skips it.
            Ok(job) if job.state == JobState::Running => tracing::debug!(
                job_id = %job.job_id,
                held_by = %job.claimed_by,
                "another worker took this audit export"
            ),
            // The job's own failure is recorded on the job rather than returned,
            // so what reaches here is only "this job is no longer running".
            Ok(job) => tracing::info!(
                job_id = %job.job_id,
                state = job.state.as_str(),
                entries = job.counts.entries,
                missing = job.counts.missing,
                version = job.version.as_deref().unwrap_or("-"),
                "IAM audit export finished"
            ),
            Err(error) => {
                tracing::warn!(%job_id, %error, "cannot run an IAM audit export");
                // Stop rather than spin: whatever is wrong will be just as wrong
                // for the next job, and the poll tick will try again.
                return;
            }
        }
    }
}

async fn prune(iam: &dyn IamStore, retention: Option<&AuditRetention>) {
    let Some(retention) = retention else {
        return;
    };
    match iam
        .prune_audit(retention, OffsetDateTime::now_utc().unix_timestamp())
        .await
    {
        Ok(report) if report.removed > 0 => tracing::info!(
            removed = report.removed,
            organizations = report.organizations,
            cutoff = report.cutoff,
            ttl_days = retention.ttl_days,
            policy_id = %retention.policy_id,
            "IAM audit entries passed their retention and were removed"
        ),
        Ok(_) => tracing::debug!("nothing in the IAM audit trail has expired"),
        Err(error) => tracing::warn!(%error, "the IAM audit retention sweep failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deployment_that_sets_no_retention_removes_nothing() {
        assert!(retention(&Config::default()).expect("a decision").is_none());
    }

    #[test]
    fn a_configured_retention_carries_its_policy_id() {
        let config = Config {
            iam_audit_retention_days: Some(400),
            iam_audit_policy_id: "policy-iam-1".to_owned(),
            ..Config::default()
        };
        let policy = retention(&config).expect("a decision").expect("a policy");
        assert_eq!(policy.ttl_days, 400);
        assert_eq!(policy.policy_id, "policy-iam-1");
    }

    #[test]
    fn a_retention_the_crate_refuses_removes_nothing_rather_than_something_else() {
        let config = Config {
            iam_audit_retention_days: Some(u32::MAX),
            ..Config::default()
        };
        assert!(retention(&config).expect("a decision").is_none());
    }
}
