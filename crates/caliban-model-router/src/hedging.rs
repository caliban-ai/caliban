//! Hedged dispatch primitives: per-attempt cancellation + race driver.
//!
//! The router calls [`race_hedged`] with a closure that, given an index and
//! a `CancellationToken`, returns a future running the candidate at that
//! index. `race_hedged` launches the primary immediately, hedges after
//! `policy.hedge_after`, and returns the winner's result; losers are
//! cancelled. If the primary errors before the hedge is launched, the
//! hedge fires immediately.

use std::future::Future;
use std::time::Duration;

use caliban_provider::{CompletionResponse, Error as ProviderError, Result as ProviderResult};
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use crate::config::HedgePolicy;

/// Outcome of one attempt — used by the driver to decide whether to keep
/// waiting for outstanding hedges or fall back to a later candidate.
#[derive(Debug)]
pub(crate) struct AttemptOutcome {
    /// Index in the candidate vec.
    pub idx: usize,
    /// The provider result.
    pub result: ProviderResult<CompletionResponse>,
}

/// Race the primary against up to `max_hedges` hedges over the first
/// `candidate_count` candidates of the dispatch chain.
///
/// Returns `(winner_idx, winner_result, losers, launched)`. `losers` are
/// reported in completion order; the driver may use them when classifying the
/// winner's fatal-for-route status. `launched` is the number of attempts
/// actually spawned (segment positions `0..launched`) — candidates never
/// launched (the primary won before `hedge_after`) are excluded, so the caller
/// can charge hedge metrics only to attempts that actually ran (#215 bug 3). A
/// non-`Ok` winner means every attempt failed.
pub(crate) async fn race_hedged<S, F>(
    policy: HedgePolicy,
    candidate_count: usize,
    spawn: S,
) -> (
    usize,
    ProviderResult<CompletionResponse>,
    Vec<AttemptOutcome>,
    usize,
)
where
    S: Fn(usize, CancellationToken) -> F,
    F: Future<Output = ProviderResult<CompletionResponse>> + Send + 'static,
{
    debug_assert!(candidate_count >= 1);
    let (hedge_after, max_hedges) = match policy {
        HedgePolicy::Disabled => (Duration::from_millis(0), 0u8),
        HedgePolicy::Race {
            hedge_after,
            max_hedges,
        } => (hedge_after, max_hedges),
    };
    let max_extra = std::cmp::min(max_hedges as usize, candidate_count.saturating_sub(1));

    let (tx, mut rx) = mpsc::unbounded_channel::<AttemptOutcome>();
    let mut tokens: Vec<CancellationToken> = Vec::with_capacity(1 + max_extra);

    let launch = |idx: usize,
                  tx: mpsc::UnboundedSender<AttemptOutcome>,
                  tokens: &mut Vec<CancellationToken>| {
        let tok = CancellationToken::new();
        tokens.push(tok.clone());
        let fut = spawn(idx, tok);
        tokio::spawn(async move {
            let result = fut.await;
            let _ = tx.send(AttemptOutcome { idx, result });
        });
    };

    launch(0, tx.clone(), &mut tokens);
    let mut launched: usize = 1;

    let mut losers: Vec<AttemptOutcome> = Vec::new();

    // Optionally arm the hedge timer.
    let mut hedge_timer = if max_extra > 0 {
        Some(Box::pin(sleep(hedge_after)))
    } else {
        None
    };

    loop {
        match hedge_timer.as_mut() {
            Some(timer) => {
                tokio::select! {
                    biased;
                    maybe_outcome = rx.recv() => {
                        let Some(outcome) = maybe_outcome else {
                            return (
                                0,
                                Err(ProviderError::adapter(ChannelClosed)),
                                losers,
                                launched,
                            );
                        };
                        if outcome.result.is_ok() {
                            cancel_others(&tokens, outcome.idx);
                            return (outcome.idx, outcome.result, losers, launched);
                        }
                        losers.push(outcome);
                        if losers.len() == launched {
                            // All in-flight attempts failed. If we can hedge,
                            // launch the next now (immediate hedge); else,
                            // return the last error.
                            if launched - 1 < max_extra && launched < candidate_count {
                                launch(launched, tx.clone(), &mut tokens);
                                launched += 1;
                                if launched - 1 >= max_extra || launched >= candidate_count {
                                    hedge_timer = None;
                                } else {
                                    hedge_timer = Some(Box::pin(sleep(hedge_after)));
                                }
                            } else {
                                let last = losers.pop().expect("we just pushed");
                                return (last.idx, last.result, losers, launched);
                            }
                        }
                    }
                    _ = timer => {
                        if launched - 1 < max_extra && launched < candidate_count {
                            launch(launched, tx.clone(), &mut tokens);
                            launched += 1;
                        }
                        if launched - 1 < max_extra && launched < candidate_count {
                            hedge_timer = Some(Box::pin(sleep(hedge_after)));
                        } else {
                            hedge_timer = None;
                        }
                    }
                }
            }
            None => {
                let Some(outcome) = rx.recv().await else {
                    return (
                        0,
                        Err(ProviderError::adapter(ChannelClosed)),
                        losers,
                        launched,
                    );
                };
                if outcome.result.is_ok() {
                    cancel_others(&tokens, outcome.idx);
                    return (outcome.idx, outcome.result, losers, launched);
                }
                losers.push(outcome);
                if losers.len() == launched {
                    let last = losers.pop().expect("we just pushed");
                    return (last.idx, last.result, losers, launched);
                }
            }
        }
    }
}

fn cancel_others(tokens: &[CancellationToken], winner: usize) {
    for (i, tok) in tokens.iter().enumerate() {
        if i != winner {
            tok.cancel();
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("hedge channel closed unexpectedly")]
struct ChannelClosed;

#[cfg(test)]
mod tests {
    use super::*;
    use caliban_provider::{CompletionResponse, Message, Role, StopReason, Usage};

    fn resp(model: &str) -> CompletionResponse {
        CompletionResponse {
            id: "id".into(),
            model: model.into(),
            message: Message {
                role: Role::Assistant,
                content: vec![],
            },
            stop_reason: StopReason::EndTurn,
            stop_sequence: None,
            usage: Usage::default(),
        }
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn primary_wins_when_fast_enough() {
        let policy = HedgePolicy::Race {
            hedge_after: Duration::from_millis(100),
            max_hedges: 1,
        };
        let (idx, result, losers, _launched) = race_hedged(policy, 2, |i, _tok| async move {
            if i == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
                Ok(resp("a"))
            } else {
                tokio::time::sleep(Duration::from_millis(500)).await;
                Ok(resp("b"))
            }
        })
        .await;
        assert_eq!(idx, 0);
        assert!(result.is_ok());
        assert!(losers.is_empty());
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn hedge_wins_when_primary_slow() {
        let policy = HedgePolicy::Race {
            hedge_after: Duration::from_millis(50),
            max_hedges: 1,
        };
        let (idx, result, _losers, _launched) = race_hedged(policy, 2, |i, _tok| async move {
            if i == 0 {
                tokio::time::sleep(Duration::from_millis(10_000)).await;
                Ok(resp("a"))
            } else {
                tokio::time::sleep(Duration::from_millis(10)).await;
                Ok(resp("b"))
            }
        })
        .await;
        assert_eq!(idx, 1);
        assert!(result.is_ok());
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn cancels_loser_token() {
        let policy = HedgePolicy::Race {
            hedge_after: Duration::from_millis(50),
            max_hedges: 1,
        };
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancelled2 = cancelled.clone();
        let (idx, _result, _losers, _launched) = race_hedged(policy, 2, move |i, tok| {
            let cancelled = cancelled2.clone();
            async move {
                if i == 0 {
                    // Primary: waits a long time, observes cancellation.
                    tokio::select! {
                        _ = tok.cancelled() => {
                            cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
                            Err(ProviderError::Cancelled)
                        }
                        _ = tokio::time::sleep(Duration::from_millis(10_000)) => {
                            Ok(resp("a"))
                        }
                    }
                } else {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    Ok(resp("b"))
                }
            }
        })
        .await;
        assert_eq!(idx, 1);
        // Give the cancellation a moment to propagate to the spawned task.
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(cancelled.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn primary_errors_immediately_triggers_hedge() {
        let policy = HedgePolicy::Race {
            hedge_after: Duration::from_secs(60),
            max_hedges: 1,
        };
        let (idx, result, _losers, _launched) = race_hedged(policy, 2, |i, _tok| async move {
            if i == 0 {
                Err(ProviderError::ServerError {
                    status: 503,
                    body: "down".into(),
                })
            } else {
                tokio::time::sleep(Duration::from_millis(10)).await;
                Ok(resp("b"))
            }
        })
        .await;
        assert_eq!(idx, 1);
        assert!(result.is_ok());
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn both_fail_returns_last_error() {
        let policy = HedgePolicy::Race {
            hedge_after: Duration::from_millis(10),
            max_hedges: 1,
        };
        let (_idx, result, _losers, _launched) = race_hedged(policy, 2, |_i, _tok| async move {
            Err::<CompletionResponse, _>(ProviderError::ModelUnavailable("nope".into()))
        })
        .await;
        assert!(result.is_err());
    }
}
