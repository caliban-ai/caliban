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
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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
    /// `true` while a single HalfOpen recovery probe is in flight. Gates the
    /// recovery window to one probe at a time (#183).
    probe_inflight: AtomicBool,
    /// Successful probes accumulated in the current HalfOpen window; the
    /// breaker Closes once this reaches `policy.half_open_probes` (#183).
    probe_successes: AtomicU32,
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
                probe_inflight: AtomicBool::new(false),
                probe_successes: AtomicU32::new(0),
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
        let cur_guard = self.inner.state.load();
        if let BreakerState::Tripped { since } = **cur_guard
            && now.duration_since(since) >= self.inner.policy.cooldown
        {
            // Cooldown elapsed: atomically flip Tripped→HalfOpen with a single
            // CAS keyed on the exact pointer we observed. A plain `store` would
            // let a stale reader resurrect HalfOpen over a breaker another
            // thread already Closed, and — together with the old read-path
            // probe-gate reset — let two readers racing the transition each
            // claim the single recovery probe (#215 bug 1, gap in #183 under
            // concurrency). The probe gate is reset when the breaker trips, so
            // the read path must NOT touch probe_inflight here.
            let half_open = Arc::new(BreakerState::HalfOpen);
            let prev = self.inner.state.compare_and_swap(&cur_guard, half_open);
            if Arc::ptr_eq(&prev, &cur_guard) {
                return BreakerState::HalfOpen;
            }
            // Lost the CAS: another thread already transitioned the breaker —
            // report whatever state it installed rather than asserting HalfOpen.
            return **self.inner.state.load();
        }
        **cur_guard
    }

    /// Decide whether the caller may issue a request through this route, and
    /// (in HalfOpen) claim the single recovery probe slot.
    ///
    /// - `Closed` → always `true` (no gating).
    /// - `Tripped` → `false`.
    /// - `HalfOpen` → `true` for exactly one caller until the probe resolves
    ///   via [`observe_success`](Self::observe_success) /
    ///   [`observe_failure`](Self::observe_failure); concurrent callers get
    ///   `false` and should fall back (#183).
    #[must_use]
    pub fn try_admit(&self) -> bool {
        self.try_admit_at(Instant::now())
    }

    fn try_admit_at(&self, now: Instant) -> bool {
        match self.state_at(now) {
            BreakerState::Closed => true,
            BreakerState::Tripped { .. } => false,
            BreakerState::HalfOpen => self
                .inner
                .probe_inflight
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok(),
        }
    }

    /// Record a success.
    pub fn observe_success(&self) {
        self.observe_success_at(Instant::now());
    }

    fn observe_success_at(&self, now: Instant) {
        match self.state_at(now) {
            BreakerState::HalfOpen => {
                // A probe succeeded. Release the probe slot and count it; Close
                // only once `half_open_probes` successes have accumulated (#183).
                let needed = self.inner.policy.half_open_probes.max(1);
                let prior = self.inner.probe_successes.fetch_add(1, Ordering::SeqCst);
                self.inner.probe_inflight.store(false, Ordering::SeqCst);
                if prior + 1 >= needed {
                    self.close();
                }
            }
            BreakerState::Closed => {
                // A success on a healthy route clears the failure window.
                self.close();
            }
            BreakerState::Tripped { .. } => {
                // A late success from a request admitted *before* the route
                // tripped must not reopen the breaker — the spec has no
                // Tripped→Closed edge. Dropping it keeps the breaker open for
                // the full cooldown (#215 bug 2).
            }
        }
    }

    /// Transition to `Closed` and clear all recovery/failure bookkeeping.
    fn close(&self) {
        self.inner.state.store(Arc::new(BreakerState::Closed));
        self.inner.probe_successes.store(0, Ordering::SeqCst);
        self.inner.probe_inflight.store(false, Ordering::SeqCst);
        self.inner
            .failures
            .lock()
            .expect("breaker failures lock")
            .clear();
    }

    /// Release the single recovery-probe slot without recording an outcome.
    ///
    /// Used when a HalfOpen probe is cancelled in flight (e.g. it lost a hedge
    /// race before completing) so the slot isn't leaked and the route can be
    /// probed again. A no-op for Closed/Tripped routes (the slot is already
    /// clear there), so it's safe to call unconditionally (#215 bug 4).
    pub(crate) fn release_probe(&self) {
        self.inner.probe_inflight.store(false, Ordering::SeqCst);
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
        // Failure in HalfOpen re-trips immediately and discards any partial
        // recovery progress / probe claim (#183).
        if matches!(cur, BreakerState::HalfOpen) {
            self.inner.probe_successes.store(0, Ordering::SeqCst);
            self.inner.probe_inflight.store(false, Ordering::SeqCst);
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
            // Reset the recovery gate as the breaker trips so the eventual
            // Tripped→HalfOpen read path never has to touch probe_inflight —
            // that read-path reset is what allowed a concurrent double-admit
            // (#215 bug 1). In Closed these are already cleared; resetting here
            // makes the "gate is clear whenever Tripped" invariant explicit.
            self.inner.probe_successes.store(0, Ordering::SeqCst);
            self.inner.probe_inflight.store(false, Ordering::SeqCst);
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
    fn half_open_admits_only_one_probe() {
        // #183: after cooldown, concurrent admits let exactly one probe through.
        let b = CircuitBreaker::new(policy(1, 60_000, 5));
        let t0 = Instant::now();
        b.observe_failure_at(t0);
        let t1 = t0 + Duration::from_millis(50);
        assert!(matches!(b.state_at(t1), BreakerState::HalfOpen));
        assert!(b.try_admit_at(t1), "first probe is admitted");
        assert!(!b.try_admit_at(t1), "second concurrent probe is denied");
        assert!(!b.try_admit_at(t1), "third concurrent probe is denied");
    }

    #[test]
    fn half_open_releases_slot_after_probe_resolves() {
        // Once a probe resolves (without closing), the next probe may proceed.
        let mut p = policy(1, 60_000, 5);
        p.half_open_probes = 2;
        let b = CircuitBreaker::new(p);
        let t0 = Instant::now();
        b.observe_failure_at(t0);
        let t1 = t0 + Duration::from_millis(50);
        assert!(matches!(b.state_at(t1), BreakerState::HalfOpen));
        assert!(b.try_admit_at(t1));
        assert!(!b.try_admit_at(t1), "slot held while probe in flight");
        b.observe_success_at(t1); // 1 of 2 — releases slot, stays HalfOpen
        assert!(matches!(b.state_at(t1), BreakerState::HalfOpen));
        assert!(b.try_admit_at(t1), "slot freed for the next probe");
    }

    #[test]
    fn half_open_requires_configured_successes_to_close() {
        // #183: half_open_probes = 3 needs three successes to Close.
        let mut p = policy(1, 60_000, 5);
        p.half_open_probes = 3;
        let b = CircuitBreaker::new(p);
        let t0 = Instant::now();
        b.observe_failure_at(t0);
        let t1 = t0 + Duration::from_millis(50);
        for n in 1..=3 {
            assert!(matches!(b.state_at(t1), BreakerState::HalfOpen));
            assert!(b.try_admit_at(t1), "probe {n} admitted");
            b.observe_success_at(t1);
            if n < 3 {
                assert!(
                    matches!(b.state_at(t1), BreakerState::HalfOpen),
                    "still recovering after {n}/3"
                );
            }
        }
        assert!(
            matches!(b.state_at(t1), BreakerState::Closed),
            "closed after 3/3 successful probes"
        );
    }

    #[test]
    fn half_open_failure_resets_probe_progress() {
        let mut p = policy(1, 60_000, 5);
        p.half_open_probes = 3;
        let b = CircuitBreaker::new(p);
        let t0 = Instant::now();
        b.observe_failure_at(t0);
        let t1 = t0 + Duration::from_millis(50);
        assert!(b.try_admit_at(t1));
        b.observe_success_at(t1); // 1 of 3
        assert!(b.try_admit_at(t1));
        b.observe_failure_at(t1); // re-trips, discards progress
        assert!(matches!(b.state_at(t1), BreakerState::Tripped { .. }));
        // After the next cooldown, recovery starts from zero again.
        let t2 = t1 + Duration::from_millis(50);
        assert!(matches!(b.state_at(t2), BreakerState::HalfOpen));
        assert!(b.try_admit_at(t2));
        b.observe_success_at(t2); // would be 1 of 3, not 2
        assert!(
            matches!(b.state_at(t2), BreakerState::HalfOpen),
            "progress reset: one success is not enough to close"
        );
    }

    #[test]
    fn concurrent_admits_after_cooldown_admit_exactly_one_probe() {
        // #215 bug 1: the Tripped→HalfOpen transition must be atomic and must
        // not reset the probe gate on the read path, or two threads both
        // catching the transition can each claim the single recovery probe.
        use std::sync::Barrier;
        use std::sync::atomic::AtomicU32;
        use std::thread;
        for round in 0..200 {
            let b = CircuitBreaker::new(policy(1, 60_000, 5));
            let t0 = Instant::now();
            b.observe_failure_at(t0);
            // A *fixed* post-cooldown instant shared by every thread keeps the
            // timing deterministic regardless of scheduler load — the only
            // variable under test is the concurrency of the transition itself.
            let probe_at = t0 + Duration::from_millis(50);
            let n = 8;
            let barrier = Arc::new(Barrier::new(n));
            let admits = Arc::new(AtomicU32::new(0));
            let mut handles = Vec::new();
            for _ in 0..n {
                let bc = b.clone();
                let bar = Arc::clone(&barrier);
                let ac = Arc::clone(&admits);
                handles.push(thread::spawn(move || {
                    bar.wait();
                    if bc.try_admit_at(probe_at) {
                        ac.fetch_add(1, Ordering::SeqCst);
                    }
                }));
            }
            for h in handles {
                h.join().unwrap();
            }
            assert_eq!(
                admits.load(Ordering::SeqCst),
                1,
                "round {round}: exactly one probe must be admitted under concurrency"
            );
        }
    }

    #[test]
    fn success_observed_while_tripped_does_not_reopen() {
        // #215 bug 2: a late success from a request admitted before the route
        // tripped must not reopen the breaker — the spec has no Tripped→Closed
        // edge. The breaker must stay Tripped for the full cooldown.
        let b = CircuitBreaker::new(policy(1, 60_000, 10_000));
        let t0 = Instant::now();
        b.observe_failure_at(t0);
        assert!(matches!(b.state_at(t0), BreakerState::Tripped { .. }));
        // A straggler success arrives while still Tripped (cooldown not yet up).
        b.observe_success_at(t0 + Duration::from_millis(5));
        assert!(
            matches!(
                b.state_at(t0 + Duration::from_millis(5)),
                BreakerState::Tripped { .. }
            ),
            "a success observed while Tripped must not close the breaker"
        );
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
