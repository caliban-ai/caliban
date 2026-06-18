//! Cost accumulator + rate-card loader.
//!
//! `RateCard` parses `rates.yaml` (vendored at crate root; overridable via
//! `CALIBAN_RATES_YAML`) into a list of per-provider, per-model rules. Each
//! rule's `model_glob` is a simple `*` glob; we resolve a (provider, model)
//! pair by picking the entry whose glob matches and whose `effective_from`
//! is the latest date `<=` "now".
//!
//! `CostAccumulator` is the session-scoped totals object. Records token usage
//! per call, multiplies through the matched rate, and exposes aggregate +
//! per-(model, query_source) breakdowns. All math runs in `rust_decimal::Decimal`
//! so accumulated USD never drifts under float rounding.
//!
//! Unknown (provider, model) pairs match no rule. We emit a single debounced
//! `tracing::warn!` per session per pair and return `$0.00`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::NaiveDate;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive as _;
use serde::Deserialize;

use caliban_provider::{RequestPurpose, Usage};

use crate::error::TelemetryError;

/// Identifier of the request's purpose, projected into the metric attribute
/// set per ADR 0033.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QuerySource {
    /// `RequestPurpose::MainLoop`.
    Main,
    /// `RequestPurpose::SubAgent`.
    SubAgent,
    /// `Summarization` / `FastClassifier` / `Embedding` / `Other`.
    Auxiliary,
}

impl QuerySource {
    /// Map `RequestPurpose` into a query-source bucket.
    #[must_use]
    pub fn from_purpose(p: Option<RequestPurpose>) -> Self {
        match p {
            Some(RequestPurpose::MainLoop) | None => Self::Main,
            Some(RequestPurpose::SubAgent) => Self::SubAgent,
            Some(
                RequestPurpose::Summarization
                | RequestPurpose::FastClassifier
                | RequestPurpose::Embedding
                | RequestPurpose::Other,
            ) => Self::Auxiliary,
        }
    }

    /// Stable string representation for OTel attributes.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::SubAgent => "subagent",
            Self::Auxiliary => "auxiliary",
        }
    }
}

// ---------------------------------------------------------------------------
// Rate-card schema
// ---------------------------------------------------------------------------

/// Top-level YAML schema. Versioned so we can evolve.
#[derive(Debug, Clone, Deserialize)]
pub struct RateCardFile {
    /// Schema version (currently `1`).
    pub version: u32,
    /// Map of provider name → ordered list of rules.
    pub providers: BTreeMap<String, Vec<RateRule>>,
}

/// A single rate rule for one (provider, model_glob) pair.
#[derive(Debug, Clone, Deserialize)]
pub struct RateRule {
    /// Glob (`*` only) for the model name.
    pub model_glob: String,
    /// Date this rule became effective.
    pub effective_from: NaiveDate,
    /// USD per million input tokens.
    pub input_per_mtok: f64,
    /// USD per million output tokens.
    pub output_per_mtok: f64,
    /// USD per million tokens read from the prompt cache.
    #[serde(default)]
    pub cache_read_per_mtok: Option<f64>,
    /// USD per million tokens written to the prompt cache.
    #[serde(default)]
    pub cache_creation_per_mtok: Option<f64>,
}

/// Loaded rate card, ready for lookups.
#[derive(Debug, Clone)]
pub struct RateCard {
    file: RateCardFile,
}

impl RateCard {
    /// Returns the rate card vendored with the crate (`rates.yaml`).
    ///
    /// # Errors
    /// Returns `TelemetryError::InvalidRates` when the embedded YAML cannot
    /// be parsed — fatal misconfiguration during build.
    pub fn embedded() -> Result<Self, TelemetryError> {
        const EMBEDDED: &str = include_str!("../rates.yaml");
        let file = serde_yaml::from_str::<RateCardFile>(EMBEDDED).map_err(|source| {
            TelemetryError::InvalidRates {
                path: PathBuf::from("(embedded rates.yaml)"),
                source,
            }
        })?;
        Ok(Self { file })
    }

    /// Load a rate card from `path`.
    ///
    /// # Errors
    /// `TelemetryError::RatesIo` on read failure, `TelemetryError::InvalidRates`
    /// on parse failure.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, TelemetryError> {
        let path_buf = path.as_ref().to_path_buf();
        let text =
            std::fs::read_to_string(&path_buf).map_err(|source| TelemetryError::RatesIo {
                path: path_buf.clone(),
                source,
            })?;
        let file = serde_yaml::from_str::<RateCardFile>(&text).map_err(|source| {
            TelemetryError::InvalidRates {
                path: path_buf,
                source,
            }
        })?;
        Ok(Self { file })
    }

    /// Construct from a parsed file (used by tests).
    #[must_use]
    pub fn from_file(file: RateCardFile) -> Self {
        Self { file }
    }

    /// Resolve a (provider, model) pair to an effective rule. The "as of"
    /// date is the date used to filter `effective_from`; pass `today()` in
    /// production.
    #[must_use]
    pub fn resolve(&self, provider: &str, model: &str, as_of: NaiveDate) -> Option<&RateRule> {
        let rules = self.file.providers.get(provider)?;
        let mut best: Option<&RateRule> = None;
        for r in rules {
            if !glob_match(&r.model_glob, model) {
                continue;
            }
            if r.effective_from > as_of {
                continue;
            }
            if best.is_none_or(|b| r.effective_from > b.effective_from) {
                best = Some(r);
            }
        }
        best
    }

    /// Number of providers in this card. Used by tests.
    #[must_use]
    pub fn provider_count(&self) -> usize {
        self.file.providers.len()
    }
}

/// Simple `*` glob match: at most one `*` allowed; matches any prefix/suffix.
/// `*` alone matches everything. No `?` or character classes.
fn glob_match(pattern: &str, candidate: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(idx) = pattern.find('*') {
        let prefix = &pattern[..idx];
        let suffix = &pattern[idx + 1..];
        candidate.starts_with(prefix)
            && candidate.ends_with(suffix)
            && candidate.len() >= prefix.len() + suffix.len()
    } else {
        pattern == candidate
    }
}

// ---------------------------------------------------------------------------
// CostAccumulator
// ---------------------------------------------------------------------------

/// Per-model + per-`query_source` running totals.
#[derive(Debug, Default, Clone)]
pub struct ModelCost {
    /// Provider name (`anthropic` etc.).
    pub provider: String,
    /// Model id (`claude-opus-4-7-20260423` etc.).
    pub model: String,
    /// Input tokens summed across all calls.
    pub input_tokens: u64,
    /// Output tokens summed across all calls.
    pub output_tokens: u64,
    /// Cache-read tokens.
    pub cache_read_tokens: u64,
    /// Cache-creation tokens.
    pub cache_creation_tokens: u64,
    /// USD spent against this (provider, model).
    pub usd: Decimal,
}

/// Aggregate breakdown returned by [`CostAccumulator::breakdown`].
#[derive(Debug, Clone)]
pub struct CostBreakdown {
    /// Grand total in USD.
    pub total_usd: Decimal,
    /// Per-(provider, model) rows.
    pub by_model: Vec<ModelCost>,
    /// Per-`QuerySource` rows.
    pub by_query_source: BTreeMap<QuerySource, Decimal>,
    /// Cache savings (vs. paying input rate for cache_read tokens).
    pub cache_savings_usd: Decimal,
}

/// Inner state of the accumulator.
#[derive(Debug, Default)]
struct CostInner {
    by_model: BTreeMap<(String, String), ModelCost>,
    by_query_source: BTreeMap<QuerySource, Decimal>,
    total_usd: Decimal,
    cache_savings_usd: Decimal,
    warned_unknown: BTreeSet<(String, String)>,
}

/// Session-scoped cumulative cost ledger. Thread-safe; clone the `Arc` to share.
#[derive(Debug)]
pub struct CostAccumulator {
    card: RateCard,
    inner: Mutex<CostInner>,
}

impl CostAccumulator {
    /// Construct with a rate card.
    #[must_use]
    pub fn new(card: RateCard) -> Self {
        Self {
            card,
            inner: Mutex::new(CostInner::default()),
        }
    }

    /// Convenience: construct from the embedded rate card.
    ///
    /// # Errors
    /// Surfaces parse failures from `RateCard::embedded`.
    pub fn with_embedded_card() -> Result<Self, TelemetryError> {
        Ok(Self::new(RateCard::embedded()?))
    }

    /// Compute the USD cost for a single (provider, model, usage) tuple
    /// without touching internal state. Useful for previews.
    #[must_use]
    pub fn price(&self, provider: &str, model: &str, usage: &Usage, as_of: NaiveDate) -> Decimal {
        let Some(rule) = self.card.resolve(provider, model, as_of) else {
            return Decimal::ZERO;
        };
        compute_usd(rule, usage)
    }

    /// Record one provider response. The price is computed against the
    /// vendored rate card; if no rule matches, contributes `$0.00` and emits
    /// a debounced warning the first time the (provider, model) pair is seen.
    pub fn record(
        &self,
        provider: &str,
        model: &str,
        usage: &Usage,
        purpose: Option<RequestPurpose>,
    ) -> Decimal {
        let today = chrono::Utc::now().date_naive();
        let qs = QuerySource::from_purpose(purpose);
        let mut inner = self.inner.lock().expect("cost mutex poisoned");
        let Some(rule) = self.card.resolve(provider, model, today) else {
            let key = (provider.to_string(), model.to_string());
            if inner.warned_unknown.insert(key.clone()) {
                tracing::warn!(
                    target: caliban_common::tracing_targets::TARGET_COST,
                    provider = provider,
                    model = model,
                    "no rate card entry; pricing as $0.00 (set CALIBAN_RATES_YAML to override)"
                );
            }
            // Still record token usage so /usage shows real numbers.
            let entry = inner.by_model.entry(key).or_insert_with(|| ModelCost {
                provider: provider.to_string(),
                model: model.to_string(),
                ..Default::default()
            });
            entry.input_tokens += u64::from(usage.input_tokens);
            entry.output_tokens += u64::from(usage.output_tokens);
            entry.cache_read_tokens += u64::from(usage.cache_read_input_tokens.unwrap_or(0));
            entry.cache_creation_tokens +=
                u64::from(usage.cache_creation_input_tokens.unwrap_or(0));
            return Decimal::ZERO;
        };

        let usd = compute_usd(rule, usage);
        let savings = compute_cache_savings(rule, usage);

        let key = (provider.to_string(), model.to_string());
        let entry = inner.by_model.entry(key).or_insert_with(|| ModelCost {
            provider: provider.to_string(),
            model: model.to_string(),
            ..Default::default()
        });
        entry.input_tokens += u64::from(usage.input_tokens);
        entry.output_tokens += u64::from(usage.output_tokens);
        entry.cache_read_tokens += u64::from(usage.cache_read_input_tokens.unwrap_or(0));
        entry.cache_creation_tokens += u64::from(usage.cache_creation_input_tokens.unwrap_or(0));
        entry.usd += usd;

        inner.total_usd += usd;
        inner.cache_savings_usd += savings;
        *inner.by_query_source.entry(qs).or_insert(Decimal::ZERO) += usd;
        usd
    }

    /// Return the running total in USD.
    #[must_use]
    pub fn total_usd(&self) -> Decimal {
        self.inner.lock().expect("cost mutex poisoned").total_usd
    }

    /// Convert total to f64; convenience for the OTLP emit boundary and the
    /// existing budget tracker. Rounding is to 6 fractional places.
    #[must_use]
    pub fn total_usd_f64(&self) -> f64 {
        self.total_usd().to_f64().unwrap_or(0.0)
    }

    /// Per-model + per-`query_source` breakdown.
    #[must_use]
    pub fn breakdown(&self) -> CostBreakdown {
        let inner = self.inner.lock().expect("cost mutex poisoned");
        CostBreakdown {
            total_usd: inner.total_usd,
            by_model: inner.by_model.values().cloned().collect(),
            by_query_source: inner.by_query_source.clone(),
            cache_savings_usd: inner.cache_savings_usd,
        }
    }
}

/// Compute the USD cost for one usage record against one rule.
fn compute_usd(rule: &RateRule, usage: &Usage) -> Decimal {
    let mtok = Decimal::from(1_000_000u64);
    let input_rate = Decimal::try_from(rule.input_per_mtok).unwrap_or(Decimal::ZERO);
    let output_rate = Decimal::try_from(rule.output_per_mtok).unwrap_or(Decimal::ZERO);
    let cache_read_rate = rule
        .cache_read_per_mtok
        .and_then(|r| Decimal::try_from(r).ok())
        .unwrap_or(input_rate);
    let cache_creation_rate = rule
        .cache_creation_per_mtok
        .and_then(|r| Decimal::try_from(r).ok())
        .unwrap_or(input_rate);

    let input_tokens = Decimal::from(u64::from(usage.input_tokens));
    let output_tokens = Decimal::from(u64::from(usage.output_tokens));
    let cache_read = Decimal::from(u64::from(usage.cache_read_input_tokens.unwrap_or(0)));
    let cache_creation = Decimal::from(u64::from(usage.cache_creation_input_tokens.unwrap_or(0)));

    let mut total = Decimal::ZERO;
    total += input_tokens * input_rate / mtok;
    total += output_tokens * output_rate / mtok;
    total += cache_read * cache_read_rate / mtok;
    total += cache_creation * cache_creation_rate / mtok;
    total
}

/// Compute the dollars saved by prompt-cache reads, vs. paying input rate.
fn compute_cache_savings(rule: &RateRule, usage: &Usage) -> Decimal {
    let mtok = Decimal::from(1_000_000u64);
    let input_rate = Decimal::try_from(rule.input_per_mtok).unwrap_or(Decimal::ZERO);
    let Some(cache_rate_raw) = rule.cache_read_per_mtok else {
        return Decimal::ZERO;
    };
    let cache_rate = Decimal::try_from(cache_rate_raw).unwrap_or(Decimal::ZERO);
    if cache_rate >= input_rate {
        return Decimal::ZERO;
    }
    let cache_read = Decimal::from(u64::from(usage.cache_read_input_tokens.unwrap_or(0)));
    cache_read * (input_rate - cache_rate) / mtok
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

    fn usage_with_cache(input: u32, output: u32, cache_read: u32, cache_creation: u32) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cache_read_input_tokens: Some(cache_read),
            cache_creation_input_tokens: Some(cache_creation),
        }
    }

    #[test]
    fn embedded_rate_card_parses() {
        let card = RateCard::embedded().expect("embedded rates.yaml must parse");
        // Anthropic + OpenAI + Google + Bedrock + Vertex + Ollama = 6.
        assert_eq!(
            card.provider_count(),
            6,
            "all six shipped providers present"
        );
    }

    #[test]
    fn anthropic_opus_pricing_uses_glob() {
        let card = RateCard::embedded().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 5, 24).unwrap();
        let rule = card
            .resolve("anthropic", "claude-opus-4-7-20260423", today)
            .expect("opus glob must match");
        assert!((rule.input_per_mtok - 15.0).abs() < 1e-9);
        assert!((rule.output_per_mtok - 75.0).abs() < 1e-9);
    }

    #[test]
    fn anthropic_sonnet_pricing() {
        let card = RateCard::embedded().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 5, 24).unwrap();
        let rule = card
            .resolve("anthropic", "claude-sonnet-4-7-20260423", today)
            .expect("sonnet glob must match");
        assert!((rule.input_per_mtok - 3.0).abs() < 1e-9);
    }

    #[test]
    fn anthropic_haiku_pricing() {
        let card = RateCard::embedded().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 5, 24).unwrap();
        let rule = card
            .resolve("anthropic", "claude-haiku-4-5-20260301", today)
            .expect("haiku glob must match");
        assert!((rule.input_per_mtok - 0.80).abs() < 1e-9);
    }

    #[test]
    fn anthropic_current_generation_models_priced() {
        // Regression for #142: the rate-card globs must cover the model ids
        // caliban actually defaults to / ships, not just one pinned generation.
        // `default_model_for(Anthropic)` is `claude-sonnet-4-6` (caliban/src/args.rs);
        // the current flagship is `claude-opus-4-8`. Both must resolve to non-zero.
        let card = RateCard::embedded().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 6, 17).unwrap();

        let sonnet = card
            .resolve("anthropic", "claude-sonnet-4-6", today)
            .expect("default model claude-sonnet-4-6 must be priced");
        assert!((sonnet.input_per_mtok - 3.0).abs() < 1e-9);
        assert!((sonnet.output_per_mtok - 15.0).abs() < 1e-9);

        let opus = card
            .resolve("anthropic", "claude-opus-4-8", today)
            .expect("flagship claude-opus-4-8 must be priced");
        assert!((opus.input_per_mtok - 15.0).abs() < 1e-9);
        assert!((opus.output_per_mtok - 75.0).abs() < 1e-9);
    }

    #[test]
    fn openai_gpt5_pricing() {
        let card = RateCard::embedded().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 5, 24).unwrap();
        let rule = card.resolve("openai", "gpt-5-preview", today).unwrap();
        assert!((rule.input_per_mtok - 5.0).abs() < 1e-9);
    }

    #[test]
    fn openai_gpt4o_pricing() {
        let card = RateCard::embedded().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 5, 24).unwrap();
        let rule = card.resolve("openai", "gpt-4o-2024-08", today).unwrap();
        assert!((rule.input_per_mtok - 2.5).abs() < 1e-9);
    }

    #[test]
    fn google_gemini_pricing() {
        let card = RateCard::embedded().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 5, 24).unwrap();
        let rule = card.resolve("google", "gemini-2.5-pro-exp", today).unwrap();
        assert!((rule.input_per_mtok - 1.25).abs() < 1e-9);
    }

    #[test]
    fn bedrock_pricing() {
        let card = RateCard::embedded().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 5, 24).unwrap();
        let rule = card
            .resolve("bedrock", "anthropic.claude-opus-4-7-v1", today)
            .unwrap();
        assert!((rule.input_per_mtok - 15.0).abs() < 1e-9);
    }

    #[test]
    fn vertex_pricing() {
        let card = RateCard::embedded().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 5, 24).unwrap();
        let rule = card
            .resolve("vertex", "claude-opus-4-7@20260423", today)
            .unwrap();
        assert!((rule.input_per_mtok - 15.0).abs() < 1e-9);
    }

    #[test]
    fn ollama_pricing_is_zero() {
        let card = RateCard::embedded().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 5, 24).unwrap();
        let rule = card.resolve("ollama", "llama3.1:70b", today).unwrap();
        assert!(rule.input_per_mtok.abs() < f64::EPSILON);
        assert!(rule.output_per_mtok.abs() < f64::EPSILON);
    }

    #[test]
    fn unknown_provider_yields_zero_and_warns_once() {
        let card = RateCard::embedded().unwrap();
        let acc = CostAccumulator::new(card);
        let cost = acc.record("nonexistent", "phantom-1", &usage(1000, 1000), None);
        assert_eq!(cost, Decimal::ZERO);
        // Second call: same key → no second warning emitted (debounce). We
        // verify by checking the warned_unknown set has exactly one entry.
        let _ = acc.record("nonexistent", "phantom-1", &usage(1000, 1000), None);
        let inner = acc.inner.lock().unwrap();
        assert_eq!(inner.warned_unknown.len(), 1);
        // Token totals still accumulated.
        let mc = inner.by_model.values().next().unwrap();
        assert_eq!(mc.input_tokens, 2000);
    }

    #[test]
    fn decimal_math_no_float_drift() {
        // 1,234,567 input × $15/Mtok = $18.518505 exactly.
        let card = RateCard::embedded().unwrap();
        let acc = CostAccumulator::new(card);
        let model = "claude-opus-4-7-20260423";
        let cost = acc.record("anthropic", model, &usage(1_234_567, 0), None);
        let expected = Decimal::new(18_518_505, 6);
        assert_eq!(cost, expected, "decimal math must be exact");
    }

    #[test]
    fn accumulator_sums_across_multiple_calls() {
        let card = RateCard::embedded().unwrap();
        let acc = CostAccumulator::new(card);
        let m = "claude-opus-4-7-20260423";
        acc.record("anthropic", m, &usage(1_000_000, 1_000_000), None);
        acc.record("anthropic", m, &usage(500_000, 500_000), None);
        // 1.5M in × $15 = $22.50; 1.5M out × $75 = $112.50 → $135 total.
        assert_eq!(acc.total_usd(), Decimal::new(135_000_000, 6));
    }

    #[test]
    fn cache_savings_computed_correctly() {
        let card = RateCard::embedded().unwrap();
        let acc = CostAccumulator::new(card);
        let m = "claude-opus-4-7-20260423";
        // 1M cache_read tokens. Input rate = $15, cache rate = $1.50 →
        // savings = 1M × ($15 - $1.50) = $13.50.
        acc.record("anthropic", m, &usage_with_cache(0, 0, 1_000_000, 0), None);
        let bd = acc.breakdown();
        assert_eq!(bd.cache_savings_usd, Decimal::new(13_500_000, 6));
    }

    #[test]
    fn breakdown_groups_by_query_source() {
        let card = RateCard::embedded().unwrap();
        let acc = CostAccumulator::new(card);
        let m = "claude-opus-4-7-20260423";
        acc.record(
            "anthropic",
            m,
            &usage(1_000_000, 0),
            Some(RequestPurpose::MainLoop),
        );
        acc.record(
            "anthropic",
            m,
            &usage(1_000_000, 0),
            Some(RequestPurpose::Summarization),
        );
        let bd = acc.breakdown();
        assert_eq!(
            bd.by_query_source.get(&QuerySource::Main).copied(),
            Some(Decimal::new(15_000_000, 6))
        );
        assert_eq!(
            bd.by_query_source.get(&QuerySource::Auxiliary).copied(),
            Some(Decimal::new(15_000_000, 6))
        );
        assert!(!bd.by_query_source.contains_key(&QuerySource::SubAgent));
    }

    #[test]
    fn effective_from_picks_latest_eligible_row() {
        // Two rows for the same glob: older effective_from is overridden
        // when the newer date is reached.
        let yaml = r#"
version: 1
providers:
  testpro:
    - model_glob: "m-*"
      effective_from: 2026-01-01
      input_per_mtok: 1.00
      output_per_mtok: 1.00
    - model_glob: "m-*"
      effective_from: 2026-06-01
      input_per_mtok: 2.00
      output_per_mtok: 2.00
"#;
        let file: RateCardFile = serde_yaml::from_str(yaml).unwrap();
        let card = RateCard::from_file(file);
        let before = card
            .resolve(
                "testpro",
                "m-x",
                NaiveDate::from_ymd_opt(2026, 3, 1).unwrap(),
            )
            .unwrap();
        assert!((before.input_per_mtok - 1.0).abs() < 1e-9);
        let after = card
            .resolve(
                "testpro",
                "m-x",
                NaiveDate::from_ymd_opt(2026, 7, 1).unwrap(),
            )
            .unwrap();
        assert!((after.input_per_mtok - 2.0).abs() < 1e-9);
    }

    #[test]
    fn invalid_yaml_fails_to_parse() {
        let bad = "not: : valid: yaml: at: all";
        let parsed: Result<RateCardFile, _> = serde_yaml::from_str(bad);
        assert!(parsed.is_err());
    }

    #[test]
    fn from_path_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("rates.yaml");
        std::fs::write(
            &p,
            r#"
version: 1
providers:
  custom:
    - model_glob: "*"
      effective_from: 2026-01-01
      input_per_mtok: 99.0
      output_per_mtok: 99.0
"#,
        )
        .unwrap();
        let card = RateCard::from_path(&p).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 5, 24).unwrap();
        let rule = card.resolve("custom", "anything", today).unwrap();
        assert!((rule.input_per_mtok - 99.0).abs() < 1e-9);
    }

    #[test]
    fn glob_match_basic_cases() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("foo*", "foobar"));
        assert!(glob_match("*bar", "foobar"));
        assert!(glob_match("foo*bar", "foo_baz_bar"));
        assert!(!glob_match("foo*", "fobar"));
        assert!(glob_match("exact", "exact"));
        assert!(!glob_match("exact", "exactly"));
    }

    #[test]
    fn query_source_from_purpose_maps_correctly() {
        assert_eq!(QuerySource::from_purpose(None), QuerySource::Main);
        assert_eq!(
            QuerySource::from_purpose(Some(RequestPurpose::MainLoop)),
            QuerySource::Main,
        );
        assert_eq!(
            QuerySource::from_purpose(Some(RequestPurpose::SubAgent)),
            QuerySource::SubAgent,
        );
        assert_eq!(
            QuerySource::from_purpose(Some(RequestPurpose::Summarization)),
            QuerySource::Auxiliary,
        );
        assert_eq!(
            QuerySource::from_purpose(Some(RequestPurpose::Embedding)),
            QuerySource::Auxiliary,
        );
        assert_eq!(QuerySource::Main.as_str(), "main");
        assert_eq!(QuerySource::SubAgent.as_str(), "subagent");
        assert_eq!(QuerySource::Auxiliary.as_str(), "auxiliary");
    }
}
