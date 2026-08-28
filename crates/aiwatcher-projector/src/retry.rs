//! Exponential backoff with jitter.
//!
//! Jitter matters more than the backoff curve: without it, every projector
//! that failed against the same unavailable trace store retries in lockstep and
//! re-creates the thundering herd that took it down.
//!
//! The randomness is a small xorshift seeded from the clock rather than a
//! dependency on `rand`. For spreading retries that is entirely sufficient, and
//! it is not used for anything security-sensitive.

use std::time::Duration;

use aiwatcher_core::ports::PortResult;

#[derive(Clone, Copy, Debug)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            base_delay: Duration::from_millis(200),
            max_delay: Duration::from_secs(30),
        }
    }
}

impl RetryPolicy {
    /// Delay before attempt `attempt` (1-based), with up to ±25% jitter.
    #[must_use]
    pub fn delay_for(&self, attempt: u32, seed: u64) -> Duration {
        let exponent = attempt.saturating_sub(1).min(16);
        let raw = self
            .base_delay
            .saturating_mul(1u32 << exponent)
            .min(self.max_delay);
        let millis = raw.as_millis() as u64;
        if millis == 0 {
            return raw;
        }
        // ±25%: scale by a factor in [0.75, 1.25].
        let spread = millis / 2;
        let offset = xorshift(seed ^ u64::from(attempt)) % (spread + 1);
        Duration::from_millis(millis.saturating_sub(spread).saturating_add(offset).max(1))
    }
}

fn xorshift(seed: u64) -> u64 {
    let mut state = seed | 1;
    state ^= state << 13;
    state ^= state >> 7;
    state ^= state << 17;
    state
}

/// Retry `operation` while it fails retryably.
///
/// A [`PortError::Rejected`] is returned immediately: the backend understood
/// the payload and refused it, so the same payload will be refused again. That
/// is the caller's cue to dead-letter rather than spin.
pub async fn with_backoff<T, F, Fut>(
    policy: RetryPolicy,
    seed: u64,
    label: &str,
    mut operation: F,
) -> PortResult<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = PortResult<T>>,
{
    let mut attempt = 1;
    loop {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(error) if !error.is_retryable() || attempt >= policy.max_attempts => {
                return Err(error);
            }
            Err(error) => {
                let delay = policy.delay_for(attempt, seed);
                tracing::warn!(
                    %error,
                    label,
                    attempt,
                    delay_ms = delay.as_millis(),
                    "retrying after a transient failure"
                );
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use aiwatcher_core::ports::PortError;

    use super::*;

    #[test]
    fn the_delay_grows_and_is_capped() {
        let policy = RetryPolicy {
            max_attempts: 10,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(2),
        };
        let first = policy.delay_for(1, 7);
        let third = policy.delay_for(3, 7);
        assert!(first < third, "{first:?} should be shorter than {third:?}");
        assert!(policy.delay_for(20, 7) <= policy.max_delay);
        assert!(policy.delay_for(1, 7) >= Duration::from_millis(1));
    }

    #[test]
    fn jitter_spreads_two_projectors_apart() {
        let policy = RetryPolicy::default();
        let mine = policy.delay_for(3, 1);
        let theirs = policy.delay_for(3, 999);
        assert_ne!(
            mine, theirs,
            "two instances with different seeds must not retry in lockstep"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_transient_failure_is_retried_until_it_succeeds() {
        let attempts = AtomicU32::new(0);
        let result = with_backoff(RetryPolicy::default(), 1, "test", || {
            let seen = attempts.fetch_add(1, Ordering::SeqCst) + 1;
            async move {
                if seen < 3 {
                    Err(PortError::Unavailable {
                        target: "test",
                        message: "down".to_owned(),
                    })
                } else {
                    Ok(seen)
                }
            }
        })
        .await;

        assert_eq!(result.ok(), Some(3));
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn a_rejection_is_not_retried() {
        let attempts = AtomicU32::new(0);
        let result: PortResult<()> = with_backoff(RetryPolicy::default(), 1, "test", || {
            attempts.fetch_add(1, Ordering::SeqCst);
            async move {
                Err(PortError::Rejected {
                    target: "test",
                    message: "malformed".to_owned(),
                })
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "a rejected payload will be rejected again"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn retries_give_up_at_the_attempt_limit() {
        let attempts = AtomicU32::new(0);
        let policy = RetryPolicy {
            max_attempts: 3,
            ..RetryPolicy::default()
        };
        let result: PortResult<()> = with_backoff(policy, 1, "test", || {
            attempts.fetch_add(1, Ordering::SeqCst);
            async move {
                Err(PortError::Unavailable {
                    target: "test",
                    message: "down".to_owned(),
                })
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }
}
