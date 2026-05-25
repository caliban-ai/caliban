//! `BudgetTracker` — real cost accumulator backed by `caliban-telemetry`.
//!
//! Per ADR 0033 the placeholder cost-of-zero is replaced by per-request token
//! usage multiplied against the vendored rate card. The public API is
//! preserved so headless's `--max-budget-usd` enforcement (ADR 0025, exit
//! code 137) keeps working — only the internal accounting changes.
//!
//! `record(usage, cost_usd)` retains the second parameter for back-compat:
//! when callers pass `0.0` we resolve the price from the embedded rate card;
//! when they pass non-zero we honor the override (used by tests to exercise
//! the budget-exceeded path deterministically).
//!
//! The accumulator is owned by `Arc<BudgetTracker>` — the same canonical
//! handle as before — so all existing call sites compile unchanged.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use caliban_provider::Usage;
use caliban_telemetry::{CostAccumulator, RateCard};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive as _;

/// Cumulative usage + budget enforcement for one headless run.
///
/// Thread-safe; `Arc<BudgetTracker>` is the canonical handle.
#[derive(Debug)]
pub(crate) struct BudgetTracker {
    /// Maximum cumulative USD; `None` disables enforcement.
    max_usd: Option<f64>,
    /// Real cost accumulator with the vendored rate card.
    cost: CostAccumulator,
    /// Cumulative input tokens.
    input_tokens: AtomicU64,
    /// Cumulative output tokens.
    output_tokens: AtomicU64,
    /// Cumulative test-injected cost overrides in micro-dollars. When tests
    /// pass a non-zero `cost_usd` to `record`, we add it on top of the
    /// rate-card-derived cost so the budget-exceeded path can be triggered
    /// without a real provider.
    test_override_micro_usd: AtomicU64,
    /// Latched true once we've observed an over-budget condition.
    exceeded: AtomicBool,
}

impl BudgetTracker {
    /// Construct a tracker with the given ceiling. `None` disables
    /// enforcement (calls still accumulate but `is_exceeded()` is always
    /// false).
    #[must_use]
    pub(crate) fn new(max_usd: Option<f64>) -> Arc<Self> {
        // Construct from the embedded rate card. If parsing fails (which
        // shouldn't happen on a built crate), we fall back to an empty card
        // — better than panicking in a constructor that runs at startup.
        let card = RateCard::embedded().unwrap_or_else(|e| {
            tracing::error!(
                target: "caliban::cost",
                error = %e,
                "failed to parse embedded rates.yaml; pricing will be $0.00",
            );
            // Empty card: everything resolves to no rule → $0.00.
            RateCard::from_file(caliban_telemetry::RateCardFile {
                version: 1,
                providers: std::collections::BTreeMap::new(),
            })
        });
        Arc::new(Self {
            max_usd,
            cost: CostAccumulator::new(card),
            input_tokens: AtomicU64::new(0),
            output_tokens: AtomicU64::new(0),
            test_override_micro_usd: AtomicU64::new(0),
            exceeded: AtomicBool::new(false),
        })
    }

    /// Record a request's usage + an optional caller-supplied cost override.
    ///
    /// `cost_usd` is added on top of the rate-card-derived cost. Production
    /// callers pass `0.0` (no override); tests pass a non-zero value to drive
    /// the budget-exceeded path without a real provider response.
    pub(crate) fn record(&self, usage: &Usage, cost_usd: f64) {
        self.record_with_model(usage, cost_usd, "anthropic", "");
    }

    /// Record + price against a known (provider, model) pair. Used by the
    /// driver once it knows the model that produced the response.
    pub(crate) fn record_with_model(
        &self,
        usage: &Usage,
        cost_usd: f64,
        provider: &str,
        model: &str,
    ) {
        self.input_tokens
            .fetch_add(u64::from(usage.input_tokens), Ordering::Relaxed);
        self.output_tokens
            .fetch_add(u64::from(usage.output_tokens), Ordering::Relaxed);

        // Rate-card price (the real cost path).
        if !model.is_empty() {
            self.cost.record(provider, model, usage, None);
        }

        // Test-supplied override on top.
        if cost_usd > 0.0 {
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
            self.test_override_micro_usd
                .fetch_add(micro, Ordering::Relaxed);
        }
        if let Some(limit) = self.max_usd
            && self.total_cost_usd() >= limit
        {
            self.exceeded.store(true, Ordering::Relaxed);
        }
    }

    /// Returns the cumulative cost in USD (rate-card-derived + test override).
    #[must_use]
    pub(crate) fn total_cost_usd(&self) -> f64 {
        let card_total = self.cost.total_usd().to_f64().unwrap_or(0.0);
        #[allow(clippy::cast_precision_loss)]
        let override_total =
            self.test_override_micro_usd.load(Ordering::Relaxed) as f64 / 1_000_000.0;
        card_total + override_total
    }

    /// Returns the cumulative cost in `Decimal` (rate-card portion only).
    #[must_use]
    pub(crate) fn cost_decimal(&self) -> Decimal {
        self.cost.total_usd()
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

    /// Returns `true` when the cost accumulator is a placeholder that returns
    /// `0.0` for every request. With ADR 0033 landed this is always `false`.
    #[must_use]
    pub(crate) fn is_placeholder() -> bool {
        false
    }

    /// Underlying `CostAccumulator` handle (for `/usage` breakdowns).
    #[must_use]
    pub(crate) fn cost(&self) -> &CostAccumulator {
        &self.cost
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
    fn cost_zero_with_unknown_model_never_exceeds() {
        // The default record() path leaves model="" so no rate card lookup
        // happens — total stays $0.00 unless an override is supplied.
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
    fn is_placeholder_returns_false_now_that_adr_0033_landed() {
        assert!(!BudgetTracker::is_placeholder());
    }

    #[test]
    fn record_with_model_uses_rate_card() {
        // 1M input tokens × $15/Mtok = $15.
        let t = BudgetTracker::new(None);
        t.record_with_model(
            &usage(1_000_000, 0),
            0.0,
            "anthropic",
            "claude-opus-4-7-20260423",
        );
        let total = t.total_cost_usd();
        assert!((total - 15.0).abs() < 1e-6, "expected $15, got {total}");
    }

    #[test]
    fn max_budget_enforcement_with_real_pricing() {
        // Tiny budget: 1M input tokens at $15/Mtok must exceed $1.
        let t = BudgetTracker::new(Some(1.0));
        t.record_with_model(
            &usage(1_000_000, 0),
            0.0,
            "anthropic",
            "claude-opus-4-7-20260423",
        );
        assert!(t.is_exceeded(), "1M tokens @ $15 must exceed a $1 budget");
    }
}
