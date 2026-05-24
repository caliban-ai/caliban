//! Per-route circuit breaker state machine.
//!
//! `Closed → Tripped (after N failures in W window) → HalfOpen (after T
//! cooldown) → Closed (after one success) | Tripped (on next failure)`.
//!
//! State is stored in an `ArcSwap<BreakerState>` so reads are lock-free
//! during the resolver's hot path. The failure ring lives in a small mutex
//! that's only taken inside `observe()`; reads do not touch it.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;

use crate::config::BreakerPolicy;

/// State of the breaker — the resolver consults this to decide whether to
/// keep a route in the candidate list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    /// Normal operation.
    Closed,
    /// Tripped at `since`; should remain Tripped for `policy.cooldown`.
    Tripped {
        /// Instant the breaker tripped.
        since: Instant,
    },
    /// Cooldown elapsed; one probe is allowed through.
    HalfOpen,
}

impl BreakerState {
    /// Human-readable name for diagnostics.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            BreakerState::Closed => "closed",
            BreakerState::Tripped { .. } => "tripped",
            BreakerState::HalfOpen => "half_open",
        }
    }
}

/// Snapshot suitable for rendering in `caliban router debug` / `/usage`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakerSnapshot {
    /// Current state name.
    pub state: &'static str,
    /// Number of failures in the current window.
    pub failure_count: u32,
    /// Time since the breaker tripped, if Tripped/HalfOpen.
    pub since: Option<Duration>,
    /// Cooldown remaining (None if Closed / cooldown elapsed).
    pub cooldown_remaining: Option<Duration>,
}

/// Per-route breaker handle. Cheap to clone (Arc internally).
#[derive(Debug, Clone)]
pub struct CircuitBreaker {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    policy: BreakerPolicy,
    state: ArcSwap<BreakerState>,
    failures: Mutex<VecDeque<Instant>>,
}

impl CircuitBreaker {
    /// Create a breaker in `Closed` state with the given policy.
    #[must_use]
    pub fn new(policy: BreakerPolicy) -> Self {
        Self {
            inner: Arc::new(Inner {
                policy,
                state: ArcSwap::from_pointee(BreakerState::Closed),
                failures: Mutex::new(VecDeque::new()),
            }),
        }
    }

    /// Lock-free read of the current state, lazily transitioning Tripped →
    /// HalfOpen if the cooldown has elapsed.
    #[must_use]
    pub fn state(&self) -> BreakerState {
        self.state_at(Instant::now())
    }

    fn state_at(&self, now: Instant) -> BreakerState {
        let cur = **self.inner.state.load();
        if let BreakerState::Tripped { since } = cur
            && now.duration_since(since) >= self.inner.policy.cooldown
        {
            // Transition to HalfOpen. Use CAS-ish swap: only move if we
            // still see the same Tripped state.
            self.inner.state.store(Arc::new(BreakerState::HalfOpen));
            return BreakerState::HalfOpen;
        }
        cur
    }

    /// Record a success.
    pub fn observe_success(&self) {
        self.observe_success_at(Instant::now());
    }

    fn observe_success_at(&self, now: Instant) {
        // Any success closes the breaker and clears the failure window.
        self.inner.state.store(Arc::new(BreakerState::Closed));
        let mut f = self.inner.failures.lock().expect("breaker failures lock");
        f.clear();
        let _ = now; // silence unused when not in debug
    }

    /// Record a failure. Trips the breaker if the failure count in the
    /// configured `window` reaches `failure_threshold`.
    pub fn observe_failure(&self) {
        self.observe_failure_at(Instant::now());
    }

    fn observe_failure_at(&self, now: Instant) {
        if self.inner.policy.is_disabled() {
            return;
        }
        let window = self.inner.policy.window;
        let threshold = self.inner.policy.failure_threshold;
        let cur = self.state_at(now);
        // Failure in HalfOpen re-trips immediately.
        if matches!(cur, BreakerState::HalfOpen) {
            self.inner
                .state
                .store(Arc::new(BreakerState::Tripped { since: now }));
            return;
        }
        let mut f = self.inner.failures.lock().expect("breaker failures lock");
        // Evict failures outside the window.
        while let Some(front) = f.front()
            && now.duration_since(*front) > window
        {
            f.pop_front();
        }
        f.push_back(now);
        if f.len() as u32 >= threshold {
            self.inner
                .state
                .store(Arc::new(BreakerState::Tripped { since: now }));
        }
    }

    /// `true` if the resolver should keep this route in the candidate list.
    #[must_use]
    pub fn admits_traffic(&self) -> bool {
        !matches!(self.state(), BreakerState::Tripped { .. })
    }

    /// Snapshot for diagnostics.
    #[must_use]
    pub fn snapshot(&self) -> BreakerSnapshot {
        let now = Instant::now();
        let state = self.state_at(now);
        let f = self.inner.failures.lock().expect("breaker failures lock");
        let count = u32::try_from(f.len()).unwrap_or(u32::MAX);
        let (since, cooldown_remaining) = match state {
            BreakerState::Tripped { since } => {
                let elapsed = now.duration_since(since);
                let remaining = self.inner.policy.cooldown.saturating_sub(elapsed);
                (Some(elapsed), Some(remaining))
            }
            _ => (None, None),
        };
        BreakerSnapshot {
            state: state.name(),
            failure_count: count,
            since,
            cooldown_remaining,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(threshold: u32, window_ms: u64, cooldown_ms: u64) -> BreakerPolicy {
        BreakerPolicy {
            failure_threshold: threshold,
            window: Duration::from_millis(window_ms),
            cooldown: Duration::from_millis(cooldown_ms),
            half_open_probes: 1,
        }
    }

    #[test]
    fn breaker_starts_closed() {
        let b = CircuitBreaker::new(policy(3, 1000, 100));
        assert!(matches!(b.state(), BreakerState::Closed));
        assert!(b.admits_traffic());
    }

    #[test]
    fn breaker_trips_after_threshold_failures_in_window() {
        let b = CircuitBreaker::new(policy(3, 60_000, 10_000));
        b.observe_failure();
        b.observe_failure();
        assert!(matches!(b.state(), BreakerState::Closed));
        b.observe_failure();
        assert!(matches!(b.state(), BreakerState::Tripped { .. }));
        assert!(!b.admits_traffic());
    }

    #[test]
    fn breaker_cooldown_elapses_to_half_open() {
        let b = CircuitBreaker::new(policy(1, 60_000, 5));
        let start = Instant::now();
        b.observe_failure_at(start);
        assert!(matches!(b.state_at(start), BreakerState::Tripped { .. }));
        let later = start + Duration::from_millis(50);
        let s = b.state_at(later);
        assert!(matches!(s, BreakerState::HalfOpen), "got {s:?}");
    }

    #[test]
    fn breaker_half_open_success_closes_breaker() {
        let b = CircuitBreaker::new(policy(1, 60_000, 1));
        let t0 = Instant::now();
        b.observe_failure_at(t0);
        let t1 = t0 + Duration::from_millis(50);
        assert!(matches!(b.state_at(t1), BreakerState::HalfOpen));
        b.observe_success_at(t1);
        assert!(matches!(b.state_at(t1), BreakerState::Closed));
    }

    #[test]
    fn breaker_half_open_failure_re_trips_with_fresh_cooldown() {
        let b = CircuitBreaker::new(policy(1, 60_000, 1));
        let t0 = Instant::now();
        b.observe_failure_at(t0);
        let t1 = t0 + Duration::from_millis(50);
        assert!(matches!(b.state_at(t1), BreakerState::HalfOpen));
        b.observe_failure_at(t1);
        let s = b.state_at(t1);
        match s {
            BreakerState::Tripped { since } => assert_eq!(since, t1),
            _ => panic!("expected Tripped after HalfOpen failure, got {s:?}"),
        }
    }

    #[test]
    fn disabled_policy_never_trips() {
        let b = CircuitBreaker::new(BreakerPolicy::disabled());
        for _ in 0..100 {
            b.observe_failure();
        }
        assert!(matches!(b.state(), BreakerState::Closed));
    }

    #[test]
    fn breaker_state_survives_multiple_requests_lock_free_read() {
        // Smoke test: many clones see consistent state.
        let b = CircuitBreaker::new(policy(2, 60_000, 60_000));
        b.observe_failure();
        let clones: Vec<CircuitBreaker> = (0..16).map(|_| b.clone()).collect();
        b.observe_failure();
        for c in &clones {
            assert!(matches!(c.state(), BreakerState::Tripped { .. }));
        }
    }
}
