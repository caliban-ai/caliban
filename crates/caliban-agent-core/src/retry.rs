//! Retry policy and executor.

use std::time::Duration;

use caliban_provider::Error as ProviderError;
use tokio_util::sync::CancellationToken;

/// Configurable retry policy for provider calls.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Maximum number of attempts (including the initial one). Must be >= 1.
    pub max_attempts: u32,
    /// Duration to wait before the first retry.
    pub initial_backoff: Duration,
    /// Multiplicative factor applied to the backoff after each attempt.
    pub backoff_multiplier: f32,
    /// Upper bound on the computed backoff duration.
    pub max_backoff: Duration,
    /// When true, jitter is applied (50–100 % of nominal backoff).
    pub jitter: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(500),
            backoff_multiplier: 2.0,
            max_backoff: Duration::from_secs(30),
            jitter: true,
        }
    }
}

impl RetryPolicy {
    /// Construct a policy that never retries (single attempt).
    #[must_use]
    pub fn no_retry() -> Self {
        Self {
            max_attempts: 1,
            ..Self::default()
        }
    }
}

/// Classify a provider error as retryable or not.
#[must_use]
pub fn is_retryable(e: &ProviderError) -> bool {
    matches!(
        e,
        ProviderError::RateLimit { .. }
            | ProviderError::Network(_)
            | ProviderError::ServerError {
                status: 502..=599,
                ..
            },
    )
}

/// Compute the backoff for attempt `n` (1-indexed).
///
/// `n = 1` is the first retry (after the initial attempt).
///
/// # Panics
/// Never panics; uses `unwrap_or` for integer cast safety.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub fn compute_backoff(policy: &RetryPolicy, attempt: u32) -> Duration {
    let factor = policy
        .backoff_multiplier
        .powi(i32::try_from(attempt.saturating_sub(1)).unwrap_or(i32::MAX));
    // f64 cast: initial_backoff.as_millis() is u128; as f64 is safe for values in range
    let nominal_ms = (policy.initial_backoff.as_millis() as f64 * f64::from(factor)) as u64;
    let nominal = Duration::from_millis(nominal_ms).min(policy.max_backoff);
    if policy.jitter {
        // 50-100% of nominal
        let pct = 0.5 + rand::random::<f32>() * 0.5;
        // f64 cast: nominal.as_millis() fits in f64; result is non-negative
        let jittered_ms = (nominal.as_millis() as f64 * f64::from(pct)) as u64;
        Duration::from_millis(jittered_ms)
    } else {
        nominal
    }
}

/// Decide the actual sleep duration for a given error + attempt.
///
/// For `RateLimit` with `retry_after`, prefer that. Otherwise use exponential
/// backoff.
#[must_use]
pub fn sleep_for(policy: &RetryPolicy, error: &ProviderError, attempt: u32) -> Duration {
    if let ProviderError::RateLimit {
        retry_after: Some(d),
    } = error
    {
        return *d;
    }
    compute_backoff(policy, attempt)
}

/// Run `f` with retry semantics. Sleeps between attempts using the policy.
/// Cancellation aborts a pending sleep early.
///
/// # Errors
/// Returns the last error if all attempts exhausted, or `ProviderError::Cancelled`
/// if the cancel token fired during a sleep.
pub async fn with_retry<F, Fut, T>(
    policy: &RetryPolicy,
    cancel: &CancellationToken,
    mut f: F,
) -> std::result::Result<T, ProviderError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = std::result::Result<T, ProviderError>>,
{
    let mut last_err: Option<ProviderError> = None;
    for attempt in 1..=policy.max_attempts {
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                if !is_retryable(&e) || attempt == policy.max_attempts {
                    return Err(e);
                }
                let sleep_d = sleep_for(policy, &e, attempt);
                last_err = Some(e);
                tokio::select! {
                    () = tokio::time::sleep(sleep_d) => {}
                    () = cancel.cancelled() => {
                        return Err(ProviderError::Cancelled);
                    }
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| {
        ProviderError::Adapter(Box::<dyn std::error::Error + Send + Sync>::from(
            "retry exhausted",
        ))
    }))
}
