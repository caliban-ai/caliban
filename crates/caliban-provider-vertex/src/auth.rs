//! Background GCP credential-refresh task.
//!
//! `gcp_auth::TokenProvider` already caches a fresh token and refreshes
//! on demand. This task pre-fetches the token on a fixed interval (5min
//! default) so the first user-facing request after a long idle period
//! doesn't pay token-refresh latency.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gcp_auth::TokenProvider;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const GCP_SCOPE: &[&str] = &["https://www.googleapis.com/auth/cloud-platform"];

/// Background auth-refresh handle for the Vertex provider.
pub struct AuthRefresh {
    interval: Duration,
    refreshes: Arc<AtomicU64>,
    failures: Arc<AtomicU64>,
    cancel: CancellationToken,
    handle: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for AuthRefresh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthRefresh")
            .field("interval", &self.interval)
            .field("refreshes", &self.refreshes.load(Ordering::Relaxed))
            .field("failures", &self.failures.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl AuthRefresh {
    /// Spawn a background refresh task. `Duration::ZERO` disables it.
    #[must_use]
    pub fn spawn(provider: Arc<dyn TokenProvider>, interval: Duration) -> Self {
        let refreshes = Arc::new(AtomicU64::new(0));
        let failures = Arc::new(AtomicU64::new(0));
        let cancel = CancellationToken::new();
        let handle = if interval.is_zero() {
            None
        } else {
            let refreshes = refreshes.clone();
            let failures = failures.clone();
            let cancel = cancel.clone();
            Some(tokio::spawn(refresh_loop(
                provider, interval, refreshes, failures, cancel,
            )))
        };
        Self {
            interval,
            refreshes,
            failures,
            cancel,
            handle,
        }
    }

    /// Successful refresh count.
    #[must_use]
    pub fn refresh_count(&self) -> u64 {
        self.refreshes.load(Ordering::Relaxed)
    }

    /// Failed refresh count.
    #[must_use]
    pub fn failure_count(&self) -> u64 {
        self.failures.load(Ordering::Relaxed)
    }

    /// Configured interval.
    #[must_use]
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Stop the background task.
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

async fn refresh_loop(
    provider: Arc<dyn TokenProvider>,
    interval: Duration,
    refreshes: Arc<AtomicU64>,
    failures: Arc<AtomicU64>,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            () = tokio::time::sleep(interval) => {
                match provider.token(GCP_SCOPE).await {
                    Ok(_) => {
                        refreshes.fetch_add(1, Ordering::Relaxed);
                        tracing::debug!(target: "caliban::provider::vertex", "gcp token refresh ok");
                    }
                    Err(e) => {
                        failures.fetch_add(1, Ordering::Relaxed);
                        tracing::warn!(target: "caliban::provider::vertex", error = %e, "gcp token refresh failed");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::duration_suboptimal_units)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use gcp_auth::{Error as GcpError, Token, TokenProvider};

    struct CountingProvider {
        calls: Arc<AtomicU64>,
        fail_after: u64,
    }

    #[async_trait]
    impl TokenProvider for CountingProvider {
        async fn token(&self, _scopes: &[&str]) -> Result<Arc<Token>, GcpError> {
            let n = self.calls.fetch_add(1, Ordering::Relaxed);
            if self.fail_after != 0 && n >= self.fail_after {
                // Synthesize a token error using the public Json variant.
                let err =
                    serde_json::from_str::<serde_json::Value>("not json").expect_err("forced");
                return Err(GcpError::Json("forced", err));
            }
            // Construct a fresh, never-expiring token via the JSON parser
            // (the only public path for tests).
            let payload = r#"{"access_token":"tok","expires_in":3600,"token_type":"Bearer"}"#;
            let t: Token = serde_json::from_str(payload).expect("token parse");
            Ok(Arc::new(t))
        }

        async fn project_id(&self) -> Result<Arc<str>, GcpError> {
            Ok(Arc::from("test-project"))
        }
    }

    fn make_provider(fail_after: u64) -> (Arc<dyn TokenProvider>, Arc<AtomicU64>) {
        let calls = Arc::new(AtomicU64::new(0));
        let p: Arc<dyn TokenProvider> = Arc::new(CountingProvider {
            calls: calls.clone(),
            fail_after,
        });
        (p, calls)
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_loop_pre_fetches_token() {
        let (provider, calls) = make_provider(0);
        let auth = AuthRefresh::spawn(provider, Duration::from_secs(60));
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_secs(125)).await;
        assert!(
            auth.refresh_count() >= 1,
            "expected at least one refresh, got {}",
            auth.refresh_count()
        );
        assert!(
            calls.load(Ordering::Relaxed) >= 1,
            "token provider should be called"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_loop_zero_interval_is_disabled() {
        let (provider, calls) = make_provider(0);
        let auth = AuthRefresh::spawn(provider, Duration::ZERO);
        tokio::time::sleep(Duration::from_secs(3600)).await;
        assert_eq!(auth.refresh_count(), 0);
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_failure_increments_counter() {
        let (provider, _) = make_provider(1);
        let auth = AuthRefresh::spawn(provider, Duration::from_secs(60));
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_secs(125)).await;
        assert!(auth.failure_count() >= 1, "expected a failure");
    }
}
