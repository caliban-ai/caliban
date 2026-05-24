//! `BudgetTracker` — placeholder cost accumulator.
//!
//! Until OTel/cost (ADR 0033) lands, every request contributes `0.0`. The
//! struct still records call counts and tracks an "over budget" flag so the
//! driver can short-circuit when an operator-injected cost (used by tests
//! and the future cost crate) exceeds the configured ceiling.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use caliban_provider::Usage;

/// Cumulative usage + budget enforcement for one headless run.
///
/// Thread-safe; `Arc<BudgetTracker>` is the canonical handle.
#[derive(Debug)]
pub(crate) struct BudgetTracker {
    /// Maximum cumulative USD; `None` disables enforcement.
    max_usd: Option<f64>,
    /// Cumulative cost in micro-dollars (f64 * 1e6 floored), kept as u64 so
    /// we can update lock-free.
    cost_micro_usd: AtomicU64,
    /// Cumulative input tokens.
    input_tokens: AtomicU64,
    /// Cumulative output tokens.
    output_tokens: AtomicU64,
    /// Latched true once we've observed an over-budget condition.
    exceeded: AtomicBool,
}

impl BudgetTracker {
    /// Construct a tracker with the given ceiling. `None` disables
    /// enforcement (calls still accumulate but `is_exceeded()` is always
    /// false).
    #[must_use]
    pub(crate) fn new(max_usd: Option<f64>) -> Arc<Self> {
        Arc::new(Self {
            max_usd,
            cost_micro_usd: AtomicU64::new(0),
            input_tokens: AtomicU64::new(0),
            output_tokens: AtomicU64::new(0),
            exceeded: AtomicBool::new(false),
        })
    }

    /// Record a request's usage + an optional caller-supplied cost.
    ///
    /// `cost_usd` is the *placeholder* cost contribution. The headless
    /// driver passes `0.0` per ADR 0025 until ADR 0033 wires real pricing;
    /// tests inject a non-zero value to exercise the budget-exceeded path.
    pub(crate) fn record(&self, usage: &Usage, cost_usd: f64) {
        self.input_tokens
            .fetch_add(u64::from(usage.input_tokens), Ordering::Relaxed);
        self.output_tokens
            .fetch_add(u64::from(usage.output_tokens), Ordering::Relaxed);
        if cost_usd > 0.0 {
            // f64 dollars → micro-dollars; saturating at u64::MAX is fine,
            // we just want monotonic accumulation.
            let micro_f = (cost_usd * 1_000_000.0).max(0.0);
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "saturated above 0.0; truncation is intentional"
            )]
            let micro = if micro_f >= 2.0_f64.powi(63) {
                u64::MAX
            } else {
                micro_f as u64
            };
            self.cost_micro_usd.fetch_add(micro, Ordering::Relaxed);
        }
        if let Some(limit) = self.max_usd
            && self.total_cost_usd() >= limit
        {
            self.exceeded.store(true, Ordering::Relaxed);
        }
    }

    /// Returns the cumulative cost in USD.
    #[must_use]
    pub(crate) fn total_cost_usd(&self) -> f64 {
        // Cost is accumulated lock-free; precision is bounded by f64's
        // 53-bit mantissa, more than enough for any practical run.
        #[allow(clippy::cast_precision_loss)]
        let micro = self.cost_micro_usd.load(Ordering::Relaxed) as f64;
        micro / 1_000_000.0
    }

    /// Returns the cumulative input + output token counts.
    #[must_use]
    pub(crate) fn total_tokens(&self) -> (u64, u64) {
        (
            self.input_tokens.load(Ordering::Relaxed),
            self.output_tokens.load(Ordering::Relaxed),
        )
    }

    /// Returns `true` once the configured ceiling has been crossed.
    #[must_use]
    pub(crate) fn is_exceeded(&self) -> bool {
        self.exceeded.load(Ordering::Relaxed)
    }

    /// Returns the configured ceiling, if any.
    #[must_use]
    pub(crate) fn max_usd(&self) -> Option<f64> {
        self.max_usd
    }

    /// Returns `true` when the budget flag was set but the cost accumulator
    /// will report `0.0` for every request (placeholder mode). Always
    /// `true` today; flips when ADR 0033 ships.
    #[must_use]
    pub(crate) fn is_placeholder() -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u32, output: u32) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }
    }

    #[test]
    fn unbounded_tracker_never_exceeds() {
        let t = BudgetTracker::new(None);
        t.record(&usage(100, 50), 0.0);
        t.record(&usage(100, 50), 999.0);
        assert!(!t.is_exceeded());
    }

    #[test]
    fn placeholder_zero_cost_never_exceeds() {
        let t = BudgetTracker::new(Some(0.001));
        for _ in 0..100 {
            t.record(&usage(10, 5), 0.0);
        }
        assert!(t.total_cost_usd().abs() < f64::EPSILON);
        assert!(!t.is_exceeded());
    }

    #[test]
    fn non_placeholder_cost_triggers_exceeded() {
        let t = BudgetTracker::new(Some(0.01));
        t.record(&usage(10, 5), 0.005);
        assert!(!t.is_exceeded());
        t.record(&usage(10, 5), 0.006);
        assert!(t.is_exceeded(), "0.011 must exceed 0.01");
    }

    #[test]
    fn tracker_accumulates_tokens() {
        let t = BudgetTracker::new(None);
        t.record(&usage(100, 50), 0.0);
        t.record(&usage(30, 20), 0.0);
        let (i, o) = t.total_tokens();
        assert_eq!(i, 130);
        assert_eq!(o, 70);
    }

    #[test]
    fn is_placeholder_returns_true_today() {
        assert!(BudgetTracker::is_placeholder());
    }
}
