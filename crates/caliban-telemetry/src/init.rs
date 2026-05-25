//! `Telemetry` facade — entry point read by the caliban binary at startup.
//!
//! `init_from_env` parses `CALIBAN_ENABLE_TELEMETRY`, the `OTEL_*` contract,
//! and the privacy opt-outs (`DISABLE_TELEMETRY`, `DO_NOT_TRACK`). It returns
//! a single value the binary holds for the life of the process; clones share
//! the cost accumulator + context window + metric emitter via `Arc`.
//!
//! When telemetry is disabled, the returned `Telemetry` is *not* a separate
//! no-op type — the same struct with `enabled = false`. The cost accumulator
//! and context window still work; they're cheap and operator-visible.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use crate::attrs::{StandardAttrs, env_truthy_default, privacy_opt_out};
use crate::context::ContextWindow;
use crate::cost::{CostAccumulator, RateCard};
use crate::error::TelemetryError;
use crate::headers::{HeadersHelperConfig, merge_headers, parse_otlp_headers_env};
use crate::metrics::{InMemoryRecorder, MetricEmitter};

/// Parsed telemetry knobs after applying env + opt-outs.
#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    /// `CALIBAN_ENABLE_TELEMETRY=1` AND no opt-out is set.
    pub enabled: bool,
    /// `OTEL_EXPORTER_OTLP_ENDPOINT` (per-signal overrides not modeled
    /// here; the OTLP exporter consults them directly).
    pub endpoint: Option<String>,
    /// `OTEL_EXPORTER_OTLP_PROTOCOL` (`grpc` / `http/protobuf` / `http/json`).
    pub protocol: String,
    /// Merged headers (env + helper, helper wins). Static at init time;
    /// the helper-refresh thread later updates the dynamic set.
    pub headers: BTreeMap<String, String>,
    /// `OTEL_METRIC_EXPORT_INTERVAL`, parsed humantime → Duration.
    pub metric_export_interval: Duration,
    /// `OTEL_LOGS_EXPORTER` (`otlp` / `console` / `none`).
    pub logs_exporter: String,
    /// `OTEL_METRICS_EXPORTER`.
    pub metrics_exporter: String,
    /// `OTEL_TRACES_EXPORTER`.
    pub traces_exporter: String,
    /// `OTEL_LOG_USER_PROMPTS`.
    pub log_user_prompts: bool,
    /// `OTEL_LOG_TOOL_DETAILS`.
    pub log_tool_details: bool,
    /// `OTEL_LOG_TOOL_CONTENT`.
    pub log_tool_content: bool,
    /// `OTEL_LOG_RAW_API_BODIES` (`0`, `1`, or `file:<dir>`).
    pub log_raw_api_bodies: String,
    /// mTLS client certificate path.
    pub client_cert: Option<PathBuf>,
    /// mTLS client private key path.
    pub client_key: Option<PathBuf>,
    /// mTLS CA certificate path.
    pub ca_cert: Option<PathBuf>,
    /// Optional headers-helper script.
    pub headers_helper: Option<HeadersHelperConfig>,
}

impl TelemetryConfig {
    /// Build from the process env.
    #[must_use]
    pub fn from_env() -> Self {
        let opt_out = privacy_opt_out();
        let enabled = env_truthy_default("CALIBAN_ENABLE_TELEMETRY", false) && !opt_out;

        let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();
        let protocol =
            std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL").unwrap_or_else(|_| "grpc".into());

        let env_headers = std::env::var("OTEL_EXPORTER_OTLP_HEADERS")
            .ok()
            .map(|s| parse_otlp_headers_env(&s))
            .unwrap_or_default();

        let interval_str =
            std::env::var("OTEL_METRIC_EXPORT_INTERVAL").unwrap_or_else(|_| "60s".into());
        let metric_export_interval =
            parse_duration(&interval_str).unwrap_or(Duration::from_mins(1));

        let logs_exporter = std::env::var("OTEL_LOGS_EXPORTER").unwrap_or_else(|_| "otlp".into());
        let metrics_exporter =
            std::env::var("OTEL_METRICS_EXPORTER").unwrap_or_else(|_| "otlp".into());
        let traces_exporter =
            std::env::var("OTEL_TRACES_EXPORTER").unwrap_or_else(|_| "otlp".into());

        let log_user_prompts = env_truthy_default("OTEL_LOG_USER_PROMPTS", false);
        let log_tool_details = env_truthy_default("OTEL_LOG_TOOL_DETAILS", false);
        let log_tool_content = env_truthy_default("OTEL_LOG_TOOL_CONTENT", false);
        let log_raw_api_bodies =
            std::env::var("OTEL_LOG_RAW_API_BODIES").unwrap_or_else(|_| "0".into());

        let client_cert = std::env::var("OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE")
            .ok()
            .map(PathBuf::from);
        let client_key = std::env::var("OTEL_EXPORTER_OTLP_CLIENT_KEY")
            .ok()
            .or_else(|| std::env::var("OTEL_EXPORTER_OTLP_PRIVATE_KEY").ok())
            .map(PathBuf::from);
        let ca_cert = std::env::var("OTEL_EXPORTER_OTLP_CERTIFICATE")
            .ok()
            .or_else(|| std::env::var("OTEL_EXPORTER_OTLP_CA_CERTIFICATE").ok())
            .map(PathBuf::from);

        // Helper script path lives in settings.toml; for the env-only path
        // we expose CALIBAN_OTEL_HEADERS_HELPER as an escape hatch.
        let headers_helper = std::env::var("CALIBAN_OTEL_HEADERS_HELPER")
            .ok()
            .map(|p| HeadersHelperConfig::new(PathBuf::from(p)));

        Self {
            enabled,
            endpoint,
            protocol,
            headers: env_headers,
            metric_export_interval,
            logs_exporter,
            metrics_exporter,
            traces_exporter,
            log_user_prompts,
            log_tool_details,
            log_tool_content,
            log_raw_api_bodies,
            client_cert,
            client_key,
            ca_cert,
            headers_helper,
        }
    }

    /// Apply the headers-helper script (if any) to merge dynamic headers
    /// into `self.headers`. Best-effort: helper failures log a warning and
    /// keep the existing header set.
    pub fn refresh_dynamic_headers(&mut self) {
        let Some(helper) = self.headers_helper.clone() else {
            return;
        };
        match crate::headers::invoke_helper(&helper.path) {
            Ok(helper_headers) => {
                self.headers = merge_headers(&self.headers, &helper_headers);
            }
            Err(e) => {
                tracing::warn!(
                    target: "caliban::telemetry",
                    error = %e,
                    "otel_headers_helper failed; reusing previous headers",
                );
            }
        }
    }
}

/// Parse a duration string in the form `60s` / `5m` / `500ms`.
fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    if let Some(num) = s.strip_suffix("ms") {
        return num.parse::<u64>().ok().map(Duration::from_millis);
    }
    if let Some(num) = s.strip_suffix('s') {
        return num.parse::<u64>().ok().map(Duration::from_secs);
    }
    if let Some(num) = s.strip_suffix('m') {
        return num.parse::<u64>().ok().map(|n| Duration::from_secs(n * 60));
    }
    if let Some(num) = s.strip_suffix('h') {
        return num
            .parse::<u64>()
            .ok()
            .map(|n| Duration::from_secs(n * 3600));
    }
    // Bare number → seconds.
    s.parse::<u64>().ok().map(Duration::from_secs)
}

/// Top-level telemetry handle. Clone freely; all internals are `Arc`'d.
#[derive(Debug, Clone)]
pub struct Telemetry {
    /// True iff OTLP emission is actually wired up.
    pub enabled: bool,
    /// Standard attribute set for every emit.
    pub standard: StandardAttrs,
    /// Metric emitter.
    pub metrics: MetricEmitter,
    /// Session-scoped cost ledger.
    pub cost: Arc<CostAccumulator>,
    /// Session-scoped context window.
    pub context: Arc<ContextWindow>,
    /// Parsed config (kept so consumers can inspect knobs).
    pub config: TelemetryConfig,
}

impl Telemetry {
    /// Read the env and construct telemetry.
    ///
    /// # Errors
    /// Surfaces rate-card parse failures from the embedded YAML — these are
    /// fatal misconfigurations.
    pub fn init_from_env(session_id: &str) -> Result<Self, TelemetryError> {
        let config = TelemetryConfig::from_env();
        let standard = StandardAttrs::from_env(session_id, env!("CARGO_PKG_VERSION"));

        let card = match std::env::var("CALIBAN_RATES_YAML").ok() {
            Some(p) if !p.is_empty() => RateCard::from_path(&p)?,
            _ => RateCard::embedded()?,
        };
        let cost = Arc::new(CostAccumulator::new(card));
        let context = Arc::new(ContextWindow::new());
        let recorder = Arc::new(InMemoryRecorder::new());
        let metrics = MetricEmitter::with_recorder(standard.clone(), recorder, config.enabled);

        // Emit session.count{start} if enabled.
        if config.enabled {
            metrics.emit_session("start");
        }

        Ok(Self {
            enabled: config.enabled,
            standard,
            metrics,
            cost,
            context,
            config,
        })
    }

    /// Construct a fully-disabled telemetry handle. Useful for tests that
    /// want the cost accumulator without touching env.
    ///
    /// # Errors
    /// Surfaces rate-card parse failures.
    pub fn disabled_for_tests(session_id: &str) -> Result<Self, TelemetryError> {
        let standard = StandardAttrs::from_env(session_id, env!("CARGO_PKG_VERSION"));
        let card = RateCard::embedded()?;
        let cost = Arc::new(CostAccumulator::new(card));
        let context = Arc::new(ContextWindow::new());
        let metrics = MetricEmitter::disabled(standard.clone());
        Ok(Self {
            enabled: false,
            standard,
            metrics,
            cost,
            context,
            config: TelemetryConfig {
                enabled: false,
                endpoint: None,
                protocol: "grpc".into(),
                headers: BTreeMap::new(),
                metric_export_interval: Duration::from_mins(1),
                logs_exporter: "otlp".into(),
                metrics_exporter: "otlp".into(),
                traces_exporter: "otlp".into(),
                log_user_prompts: false,
                log_tool_details: false,
                log_tool_content: false,
                log_raw_api_bodies: "0".into(),
                client_cert: None,
                client_key: None,
                ca_cert: None,
                headers_helper: None,
            },
        })
    }

    /// Generate a fresh session id (UUIDv4 string).
    #[must_use]
    pub fn new_session_id() -> String {
        Uuid::new_v4().to_string()
    }

    /// Flush pending exporter batches. Calls into the OTLP SDK when wired.
    /// Always returns `Ok(())` when telemetry is disabled.
    ///
    /// # Errors
    /// Currently never returns an error. The signature reserves it for the
    /// SDK integration.
    pub fn shutdown(self) -> Result<(), TelemetryError> {
        if self.enabled {
            self.metrics.emit_session("end");
            #[cfg(feature = "otlp")]
            {
                // Force-flush exporters here when the feature is on.
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_handles_units() {
        assert_eq!(parse_duration("500ms"), Some(Duration::from_millis(500)));
        assert_eq!(parse_duration("30s"), Some(Duration::from_secs(30)));
        assert_eq!(parse_duration("5m"), Some(Duration::from_mins(5)));
        assert_eq!(parse_duration("1h"), Some(Duration::from_hours(1)));
        assert_eq!(parse_duration("60"), Some(Duration::from_mins(1)));
        assert_eq!(parse_duration("nope"), None);
    }

    #[test]
    fn disabled_for_tests_works_without_env() {
        let t = Telemetry::disabled_for_tests("sess-1").unwrap();
        assert!(!t.enabled);
        assert_eq!(t.cost.total_usd(), rust_decimal::Decimal::ZERO);
        assert_eq!(t.context.capacity(), 0);
        assert_eq!(t.standard.session_id, "sess-1");
        assert_eq!(t.standard.app_version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn new_session_id_is_uuid_v4_shaped() {
        let id = Telemetry::new_session_id();
        // UUID string: 32 hex + 4 dashes = 36 chars.
        assert_eq!(id.len(), 36);
        assert_eq!(id.matches('-').count(), 4);
    }

    #[test]
    fn shutdown_emits_session_end_when_enabled() {
        let standard = StandardAttrs {
            session_id: "sess-1".into(),
            app_version: "9.9.9".into(),
            app_name: "caliban".into(),
            user_id: "anon".into(),
            host_os: "macos".into(),
            include_session_id_on_metrics: true,
            include_version_on_metrics: true,
            include_account_uuid_on_metrics: false,
        };
        let recorder = Arc::new(InMemoryRecorder::new());
        let metrics = MetricEmitter::with_recorder(standard.clone(), Arc::clone(&recorder), true);
        let card = RateCard::embedded().unwrap();
        let telemetry = Telemetry {
            enabled: true,
            standard,
            metrics,
            cost: Arc::new(CostAccumulator::new(card)),
            context: Arc::new(ContextWindow::new()),
            config: TelemetryConfig {
                enabled: true,
                endpoint: None,
                protocol: "grpc".into(),
                headers: BTreeMap::new(),
                metric_export_interval: Duration::from_mins(1),
                logs_exporter: "otlp".into(),
                metrics_exporter: "otlp".into(),
                traces_exporter: "otlp".into(),
                log_user_prompts: false,
                log_tool_details: false,
                log_tool_content: false,
                log_raw_api_bodies: "0".into(),
                client_cert: None,
                client_key: None,
                ca_cert: None,
                headers_helper: None,
            },
        };
        telemetry.shutdown().unwrap();
        let evts = recorder.by_name("caliban.session.count");
        assert!(
            evts.iter().any(|e| e.attr("phase") == Some("end")),
            "shutdown emits session.count{{phase=end}}",
        );
    }
}
