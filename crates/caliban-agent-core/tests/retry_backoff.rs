#![allow(missing_docs)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use caliban_agent_core::retry::{RetryPolicy, compute_backoff, is_retryable, with_retry};
use caliban_provider::Error as ProviderError;
use tokio_util::sync::CancellationToken;

#[test]
fn default_policy_has_3_attempts() {
    let p = RetryPolicy::default();
    assert_eq!(p.max_attempts, 3);
}

#[test]
fn no_retry_has_1_attempt() {
    let p = RetryPolicy::no_retry();
    assert_eq!(p.max_attempts, 1);
}

#[test]
fn retryable_classification() {
    assert!(is_retryable(&ProviderError::Network(Box::<
        dyn std::error::Error + Send + Sync,
    >::from("x"))));
    assert!(is_retryable(&ProviderError::RateLimit {
        retry_after: None
    }));
    assert!(is_retryable(&ProviderError::ServerError {
        status: 503,
        body: String::new()
    }));
    assert!(is_retryable(&ProviderError::StreamInterrupted(
        "connection reset by peer".into()
    )));
    assert!(is_retryable(&ProviderError::ServerError {
        status: 500,
        body: String::new()
    }));
    assert!(!is_retryable(&ProviderError::Auth("nope".into())));
    assert!(!is_retryable(&ProviderError::InvalidRequest("nope".into())));
}

#[test]
fn backoff_math_no_jitter() {
    let p = RetryPolicy {
        initial_backoff: Duration::from_millis(100),
        backoff_multiplier: 2.0,
        max_backoff: Duration::from_mins(1),
        jitter: false,
        ..Default::default()
    };
    assert_eq!(compute_backoff(&p, 1), Duration::from_millis(100));
    assert_eq!(compute_backoff(&p, 2), Duration::from_millis(200));
    assert_eq!(compute_backoff(&p, 3), Duration::from_millis(400));
}

#[test]
fn backoff_caps_at_max() {
    let p = RetryPolicy {
        initial_backoff: Duration::from_secs(1),
        backoff_multiplier: 10.0,
        max_backoff: Duration::from_secs(5),
        jitter: false,
        ..Default::default()
    };
    assert_eq!(compute_backoff(&p, 10), Duration::from_secs(5));
}

#[tokio::test(start_paused = true)]
async fn retries_until_success() {
    let counter = Arc::new(AtomicU32::new(0));
    let cancel = CancellationToken::new();
    let policy = RetryPolicy {
        jitter: false,
        initial_backoff: Duration::from_millis(10),
        ..Default::default()
    };
    let counter_clone = counter.clone();
    let result: Result<u32, ProviderError> = with_retry(&policy, &cancel, move || {
        let c = counter_clone.clone();
        async move {
            let n = c.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                Err(ProviderError::Network(Box::<
                    dyn std::error::Error + Send + Sync,
                >::from("nope")))
            } else {
                Ok(42)
            }
        }
    })
    .await;
    assert_eq!(result.unwrap(), 42);
    assert_eq!(counter.load(Ordering::SeqCst), 3);
}

#[tokio::test(start_paused = true)]
async fn does_not_retry_on_auth_error() {
    let counter = Arc::new(AtomicU32::new(0));
    let cancel = CancellationToken::new();
    let policy = RetryPolicy::default();
    let counter_clone = counter.clone();
    let _result: Result<u32, ProviderError> = with_retry(&policy, &cancel, move || {
        let c = counter_clone.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            Err(ProviderError::Auth("bad key".into()))
        }
    })
    .await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn cancellation_during_backoff_returns_cancelled() {
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    let policy = RetryPolicy {
        jitter: false,
        initial_backoff: Duration::from_secs(10),
        ..Default::default()
    };

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel_clone.cancel();
    });

    let result: Result<u32, ProviderError> = with_retry(&policy, &cancel, || async {
        Err::<u32, _>(ProviderError::Network(Box::<
            dyn std::error::Error + Send + Sync,
        >::from("nope")))
    })
    .await;
    assert!(matches!(result, Err(ProviderError::Cancelled)));
}

#[tokio::test(start_paused = true)]
async fn server_error_500_retries_then_succeeds() {
    let counter = Arc::new(AtomicU32::new(0));
    let cancel = CancellationToken::new();
    let policy = RetryPolicy {
        max_attempts: 3,
        initial_backoff: Duration::from_millis(0),
        jitter: false,
        ..Default::default()
    };
    let counter_clone = counter.clone();
    let result: Result<u32, ProviderError> = with_retry(&policy, &cancel, move || {
        let c = counter_clone.clone();
        async move {
            let n = c.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Err(ProviderError::ServerError {
                    status: 500,
                    body: "Internal Server Error".into(),
                })
            } else {
                Ok(7)
            }
        }
    })
    .await;
    assert_eq!(result.unwrap(), 7);
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn invalid_request_does_not_retry() {
    let counter = Arc::new(AtomicU32::new(0));
    let cancel = CancellationToken::new();
    let policy = RetryPolicy {
        max_attempts: 3,
        initial_backoff: Duration::from_millis(0),
        jitter: false,
        ..Default::default()
    };
    let counter_clone = counter.clone();
    let result: Result<u32, ProviderError> = with_retry(&policy, &cancel, move || {
        let c = counter_clone.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            Err(ProviderError::InvalidRequest("bad param".into()))
        }
    })
    .await;
    assert!(matches!(result, Err(ProviderError::InvalidRequest(_))));
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}
