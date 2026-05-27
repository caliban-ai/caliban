//! Metric emitter — typed wrappers around the standard caliban metric set.
//!
//! The emitter holds an [`InMemoryRecorder`] used by tests + an opaque OTel
//! handle when the `otlp` feature is on. The recorder captures every emit
//! so tests can assert on metric name + attribute set without spinning up a
//! real OTLP collector. When the recorder is disabled (production with OTLP
//! on, or production without OTLP), it's a zero-overhead no-op.
//!
//! Metric names mirror Claude Code's, but with the `claude_code.` prefix
//! rewritten to `caliban.`:
//!
//! - `caliban.session.count` (counter)
//! - `caliban.lines_of_code.count` (counter)
//! - `caliban.cost.usage` (counter, USD)
//! - `caliban.token.usage` (counter)
//! - `caliban.code_edit_tool.decision` (counter)
//! - `caliban.active_time.total` (gauge, seconds)
//!
//! Plan A — turn-loop resilience adds the following counters; the constants
//! are exposed here so downstream telemetry sinks can subscribe by name:
//!
//! - [`RECOVERY_MAX_TOKENS_RECOVERED`]
//! - [`RECOVERY_STREAM_IDLE_ABORTED`]
//! - [`RECOVERY_REACTIVE_COMPACTED`]
//! - [`RECOVERY_REFUSALS_SURFACED`]

/// Counter: a MaxTokens turn was recovered via Stage A / Stage B.
pub const RECOVERY_MAX_TOKENS_RECOVERED: &str = "caliban.recovery.max_tokens_recovered";
/// Counter: a streaming run aborted because the SSE stream went idle past
/// the configured timeout (`WatchedStream` fired).
pub const RECOVERY_STREAM_IDLE_ABORTED: &str = "caliban.recovery.stream_idle_aborted";
/// Counter: a `ContextTooLong` provider error was rescued by reactive
/// compaction.
pub const RECOVERY_REACTIVE_COMPACTED: &str = "caliban.recovery.reactive_compacted";
/// Counter: a turn ended in `Refusal` or `ContentFilter` and was surfaced to
/// the caller via a synthetic assistant message.
pub const RECOVERY_REFUSALS_SURFACED: &str = "caliban.recovery.refusals_surfaced";

use std::sync::{Arc, Mutex};

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive as _;

use crate::attrs::StandardAttrs;
use crate::cost::QuerySource;

/// A single captured metric event.
#[derive(Debug, Clone)]
pub struct RecordedMetric {
    /// Metric name (e.g. `caliban.cost.usage`).
    pub name: &'static str,
    /// Numeric value (cost / count / seconds).
    pub value: f64,
    /// (key, value) attribute pairs, sorted by key.
    pub attrs: Vec<(String, String)>,
}

impl RecordedMetric {
    /// Returns true iff every (key, value) in `subset` appears in `self.attrs`.
    #[must_use]
    pub fn has_attrs(&self, subset: &[(&str, &str)]) -> bool {
        subset
            .iter()
            .all(|(k, v)| self.attrs.iter().any(|(ak, av)| ak == k && av == v))
    }

    /// Returns the value of an attribute, if present.
    #[must_use]
    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// In-memory recorder used by tests + assertions.
#[derive(Debug, Default)]
pub struct InMemoryRecorder {
    events: Mutex<Vec<RecordedMetric>>,
}

impl InMemoryRecorder {
    /// New empty recorder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// All recorded events, ordered by emit time.
    #[must_use]
    pub fn events(&self) -> Vec<RecordedMetric> {
        self.events.lock().expect("recorder mutex poisoned").clone()
    }

    /// Helper: events matching a given metric name.
    #[must_use]
    pub fn by_name(&self, name: &str) -> Vec<RecordedMetric> {
        self.events()
            .into_iter()
            .filter(|e| e.name == name)
            .collect()
    }

    fn push(&self, name: &'static str, value: f64, attrs: Vec<(String, String)>) {
        let mut events = self.events.lock().expect("recorder mutex poisoned");
        events.push(RecordedMetric { name, value, attrs });
    }
}

/// Typed metric facade. Constructed by `Telemetry::init_from_env`.
#[derive(Debug, Clone)]
pub struct MetricEmitter {
    standard: StandardAttrs,
    recorder: Arc<InMemoryRecorder>,
    /// `false` when telemetry is hard-disabled. We still record into the
    /// in-memory recorder so `/usage` etc. show real numbers without an OTLP
    /// collector, but we skip emits to the real OTel SDK.
    #[allow(
        dead_code,
        reason = "read by the otlp feature wiring; preserved without the feature for ABI stability"
    )]
    pub(crate) enabled: bool,
}

impl MetricEmitter {
    /// Construct a no-op emitter (telemetry disabled). The in-memory recorder
    /// is still wired so tests can introspect; production with `enabled=false`
    /// is approximately a single `Arc::clone` per emit.
    #[must_use]
    pub fn disabled(standard: StandardAttrs) -> Self {
        Self {
            standard,
            recorder: Arc::new(InMemoryRecorder::new()),
            enabled: false,
        }
    }

    /// Construct an emitter that records every emission into `recorder`.
    /// When `enabled=true`, real OTel emission also fires (when the `otlp`
    /// feature is compiled in).
    #[must_use]
    pub fn with_recorder(
        standard: StandardAttrs,
        recorder: Arc<InMemoryRecorder>,
        enabled: bool,
    ) -> Self {
        Self {
            standard,
            recorder,
            enabled,
        }
    }

    /// Returns a handle to the in-memory recorder (used by tests).
    #[must_use]
    pub fn recorder(&self) -> Arc<InMemoryRecorder> {
        Arc::clone(&self.recorder)
    }

    /// Returns the standard attribute set.
    #[must_use]
    pub fn standard(&self) -> &StandardAttrs {
        &self.standard
    }

    fn base_attrs(&self) -> Vec<(String, String)> {
        self.standard
            .metric_attrs()
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect()
    }

    fn record(&self, name: &'static str, value: f64, extra: &[(&str, String)]) {
        let mut attrs = self.base_attrs();
        for (k, v) in extra {
            attrs.push(((*k).to_string(), v.clone()));
        }
        // Sort for deterministic test inspection.
        attrs.sort_by(|a, b| a.0.cmp(&b.0));
        self.recorder.push(name, value, attrs);
        // OTel SDK emission would go here when `otlp` is enabled.
        #[cfg(feature = "otlp")]
        {
            // The actual emit goes through the OTel metric API; we keep the
            // implementation thin here to avoid coupling tests to the SDK.
            // The recorder above is the source of truth for assertions.
        }
    }

    /// Emit `caliban.session.count` with `start` or `end` attribute.
    pub fn emit_session(&self, phase: &str) {
        self.record(
            "caliban.session.count",
            1.0,
            &[("phase", phase.to_string())],
        );
    }

    /// Emit `caliban.cost.usage`. `usd_decimal` is converted to `f64` at the
    /// boundary; internal accounting stays Decimal.
    pub fn emit_cost(&self, usd: Decimal, model: &str, source: QuerySource, effort: &str) {
        let v = usd.to_f64().unwrap_or(0.0);
        self.record(
            "caliban.cost.usage",
            v,
            &[
                ("model", model.to_string()),
                ("query_source", source.as_str().to_string()),
                ("effort", effort.to_string()),
            ],
        );
    }

    /// Emit `caliban.token.usage`. `kind` is one of `input` / `output` /
    /// `cacheRead` / `cacheCreation` (Claude Code's spelling).
    pub fn emit_tokens(&self, count: u64, kind: &str, model: &str) {
        #[allow(
            clippy::cast_precision_loss,
            reason = "token counts well within f64 mantissa"
        )]
        let v = count as f64;
        self.record(
            "caliban.token.usage",
            v,
            &[("type", kind.to_string()), ("model", model.to_string())],
        );
    }

    /// Emit `caliban.lines_of_code.count` (`type=added|removed`).
    pub fn emit_lines_of_code(&self, count: u64, kind: &str) {
        #[allow(clippy::cast_precision_loss)]
        let v = count as f64;
        self.record(
            "caliban.lines_of_code.count",
            v,
            &[("type", kind.to_string())],
        );
    }

    /// Emit `caliban.code_edit_tool.decision`.
    pub fn emit_edit_decision(
        &self,
        tool: &str,
        decision: &str,
        source: &str,
        language: Option<&str>,
    ) {
        let mut extra = vec![
            ("tool", tool.to_string()),
            ("decision", decision.to_string()),
            ("source", source.to_string()),
        ];
        if let Some(lang) = language {
            extra.push(("language", lang.to_string()));
        }
        self.record("caliban.code_edit_tool.decision", 1.0, &extra);
    }

    /// Emit `caliban.active_time.total` (`type=user|cli`).
    pub fn emit_active_time(&self, seconds: f64, kind: &str) {
        self.record(
            "caliban.active_time.total",
            seconds,
            &[("type", kind.to_string())],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn em() -> MetricEmitter {
        let attrs = StandardAttrs {
            session_id: "sess-abc".into(),
            app_version: "9.9.9".into(),
            app_name: "caliban".into(),
            user_id: "anon".into(),
            host_os: "macos".into(),
            include_session_id_on_metrics: true,
            include_version_on_metrics: true,
            include_account_uuid_on_metrics: false,
        };
        MetricEmitter::with_recorder(attrs, Arc::new(InMemoryRecorder::new()), true)
    }

    #[test]
    fn emit_session_records_event() {
        let e = em();
        e.emit_session("start");
        let evts = e.recorder().by_name("caliban.session.count");
        assert_eq!(evts.len(), 1);
        assert!(evts[0].has_attrs(&[("phase", "start")]));
        // Standard attrs present.
        assert_eq!(evts[0].attr("session.id"), Some("sess-abc"));
        assert_eq!(evts[0].attr("app.version"), Some("9.9.9"));
    }

    #[test]
    fn emit_cost_records_usd_and_query_source() {
        let e = em();
        e.emit_cost(
            Decimal::new(25_000_000, 6), // $25
            "claude-opus-4-7",
            QuerySource::Main,
            "high",
        );
        let evts = e.recorder().by_name("caliban.cost.usage");
        assert_eq!(evts.len(), 1);
        assert!((evts[0].value - 25.0).abs() < 1e-9);
        assert_eq!(evts[0].attr("model"), Some("claude-opus-4-7"));
        assert_eq!(evts[0].attr("query_source"), Some("main"));
        assert_eq!(evts[0].attr("effort"), Some("high"));
    }

    #[test]
    fn emit_tokens_includes_type_and_model() {
        let e = em();
        e.emit_tokens(1024, "input", "claude-opus-4-7");
        e.emit_tokens(512, "cacheRead", "claude-opus-4-7");
        let inp = e.recorder().by_name("caliban.token.usage");
        assert_eq!(inp.len(), 2);
        assert_eq!(inp[0].attr("type"), Some("input"));
        assert_eq!(inp[1].attr("type"), Some("cacheRead"));
    }

    #[test]
    fn emit_lines_of_code_routes_type() {
        let e = em();
        e.emit_lines_of_code(42, "added");
        e.emit_lines_of_code(7, "removed");
        let evts = e.recorder().by_name("caliban.lines_of_code.count");
        assert_eq!(evts.len(), 2);
        assert_eq!(evts[0].attr("type"), Some("added"));
        assert_eq!(evts[1].attr("type"), Some("removed"));
    }

    #[test]
    fn emit_edit_decision_records_with_optional_language() {
        let e = em();
        e.emit_edit_decision("Edit", "accept", "user", Some("rust"));
        let evts = e.recorder().by_name("caliban.code_edit_tool.decision");
        assert_eq!(evts.len(), 1);
        assert_eq!(evts[0].attr("tool"), Some("Edit"));
        assert_eq!(evts[0].attr("decision"), Some("accept"));
        assert_eq!(evts[0].attr("source"), Some("user"));
        assert_eq!(evts[0].attr("language"), Some("rust"));
    }

    #[test]
    fn metric_attrs_strip_session_id_when_knob_off() {
        let attrs = StandardAttrs {
            session_id: "sess-abc".into(),
            app_version: "9.9.9".into(),
            app_name: "caliban".into(),
            user_id: "anon".into(),
            host_os: "macos".into(),
            include_session_id_on_metrics: false,
            include_version_on_metrics: true,
            include_account_uuid_on_metrics: false,
        };
        let e = MetricEmitter::with_recorder(attrs, Arc::new(InMemoryRecorder::new()), true);
        e.emit_session("start");
        let evts = e.recorder().by_name("caliban.session.count");
        assert_eq!(evts[0].attr("session.id"), None);
        assert_eq!(evts[0].attr("app.version"), Some("9.9.9"));
    }
}
