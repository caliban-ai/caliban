//! Background AWS credential-refresh task.
//!
//! The AWS SDK manages credential rotation internally for IMDS / SSO /
//! web-identity / process credentials providers. This task exists so that
//! externally-rotated credentials (e.g. `aws sso login` rewriting the
//! cache file, or a sidecar that rewrites `~/.aws/credentials`) are picked
//! up promptly without waiting for the SDK's internal expiry timer.
//!
//! On each tick we increment a refresh counter; integration with a fresh
//! `SdkConfig::load()` is left to a follow-up because the SDK exposes the
//! credential cache only behind private APIs. In the meantime the task
//! keeps a counter that tests assert against, and the cancellation token
//! lets callers shut the task down cleanly.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Background auth-refresh handle.
pub struct AuthRefresh {
    interval: Duration,
    refreshes: Arc<AtomicU64>,
    cancel: CancellationToken,
    handle: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for AuthRefresh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthRefresh")
            .field("interval", &self.interval)
            .field("refreshes", &self.refreshes.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl AuthRefresh {
    /// Spawn a new background refresh task with the given interval.
    ///
    /// A `Duration::ZERO` interval disables proactive refresh — the task
    /// is still constructed (the counter starts at 0) but never ticks.
    #[must_use]
    pub fn spawn(interval: Duration) -> Self {
        let refreshes = Arc::new(AtomicU64::new(0));
        let cancel = CancellationToken::new();
        let handle = if interval.is_zero() {
            None
        } else {
            let refreshes = refreshes.clone();
            let cancel = cancel.clone();
            Some(tokio::spawn(refresh_loop(interval, refreshes, cancel)))
        };
        Self {
            interval,
            refreshes,
            cancel,
            handle,
        }
    }

    /// Number of completed refresh ticks (mostly for tests).
    #[must_use]
    pub fn refresh_count(&self) -> u64 {
        self.refreshes.load(Ordering::Relaxed)
    }

    /// Configured refresh interval (read-only).
    #[must_use]
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Trigger an immediate refresh tick (mostly for tests).
    pub fn refresh_now(&self) {
        self.refreshes.fetch_add(1, Ordering::Relaxed);
    }

    /// Stop the background task. Idempotent.
    pub fn stop(&self) {
        self.cancel.cancel();
    }
}

impl Drop for AuthRefresh {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(h) = self.handle.take() {
            h.abort();
        }
    }
}

async fn refresh_loop(interval: Duration, counter: Arc<AtomicU64>, cancel: CancellationToken) {
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            () = tokio::time::sleep(interval) => {
                counter.fetch_add(1, Ordering::Relaxed);
                tracing::debug!(target: "caliban::provider::bedrock", "aws credential refresh tick");
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::duration_suboptimal_units)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn refresh_loop_ticks_on_interval() {
        let auth = AuthRefresh::spawn(Duration::from_secs(60));
        // Let the spawned task reach its first sleep before we advance time.
        tokio::task::yield_now().await;
        assert_eq!(auth.refresh_count(), 0);
        tokio::time::sleep(Duration::from_secs(125)).await;
        assert!(auth.refresh_count() >= 1, "expected at least one tick");
        auth.stop();
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_loop_zero_interval_is_disabled() {
        let auth = AuthRefresh::spawn(Duration::ZERO);
        tokio::time::sleep(Duration::from_secs(3600)).await;
        assert_eq!(auth.refresh_count(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_now_increments_counter() {
        let auth = AuthRefresh::spawn(Duration::ZERO);
        auth.refresh_now();
        auth.refresh_now();
        assert_eq!(auth.refresh_count(), 2);
    }
}
