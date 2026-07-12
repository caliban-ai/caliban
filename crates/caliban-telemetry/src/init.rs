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

/// A `tracing_subscriber::Layer` erased behind a `Box` so the exact OTLP
/// tracer type doesn't leak into the caliban binary's registry generics.
#[cfg(feature = "otlp")]
pub type BoxedLayer<S> = Box<dyn tracing_subscriber::Layer<S> + Send + Sync>;

/// The OTLP transport implied by an `OTEL_EXPORTER_OTLP_PROTOCOL` value.
/// Kept feature-independent (no OTLP types) so protocol mapping is unit
/// testable without compiling the exporter stack.
#[cfg(feature = "otlp")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OtlpProtocol {
    /// `grpc` — tonic transport.
    Grpc,
    /// `http/protobuf` — binary payloads over HTTP.
    HttpBinary,
    /// `http/json` — JSON payloads over HTTP.
    HttpJson,
    /// Any other (unrecognized) protocol string.
    Unsupported,
}

/// Classify an `OTEL_EXPORTER_OTLP_PROTOCOL` string into an [`OtlpProtocol`].
#[cfg(feature = "otlp")]
pub(crate) fn classify_protocol(protocol: &str) -> OtlpProtocol {
    match protocol.trim() {
        "grpc" => OtlpProtocol::Grpc,
        "http/protobuf" => OtlpProtocol::HttpBinary,
        "http/json" => OtlpProtocol::HttpJson,
        _ => OtlpProtocol::Unsupported,
    }
}

/// Real OTLP span-export pipeline. Isolated in a feature-gated module so the
/// facade compiles unchanged when the exporter is not built in.
#[cfg(feature = "otlp")]
mod otlp_pipeline {
    use super::{OtlpProtocol, TelemetryConfig, classify_protocol};
    use crate::error::TelemetryError;

    use opentelemetry::KeyValue;
    use opentelemetry_sdk::Resource;
    use opentelemetry_sdk::trace::TracerProvider;

    /// Build the `service.*` resource stamped onto every exported span.
    pub(super) fn caliban_resource(app_version: &str) -> Resource {
        Resource::new(vec![
            KeyValue::new("service.name", "caliban"),
            KeyValue::new("service.version", app_version.to_string()),
        ])
    }

    /// Select and build the OTLP span exporter for the configured protocol.
    /// Returns a clear [`TelemetryError`] (never panics) when the protocol is
    /// unrecognized or its transport feature is not compiled in.
    pub(super) fn build_span_exporter(
        config: &TelemetryConfig,
    ) -> Result<opentelemetry_otlp::SpanExporter, TelemetryError> {
        match classify_protocol(&config.protocol) {
            OtlpProtocol::Grpc => build_grpc(config),
            OtlpProtocol::HttpBinary | OtlpProtocol::HttpJson => build_http(config),
            OtlpProtocol::Unsupported => Err(TelemetryError::OtlpExporter(format!(
                "unsupported OTEL_EXPORTER_OTLP_PROTOCOL {:?} \
                 (expected grpc | http/protobuf | http/json)",
                config.protocol
            ))),
        }
    }

    #[cfg(feature = "otlp-grpc")]
    fn build_grpc(
        config: &TelemetryConfig,
    ) -> Result<opentelemetry_otlp::SpanExporter, TelemetryError> {
        use opentelemetry_otlp::{SpanExporter, WithExportConfig, WithTonicConfig};
        let mut builder = SpanExporter::builder().with_tonic();
        if let Some(endpoint) = config.endpoint.as_deref() {
            builder = builder.with_endpoint(endpoint);
        }
        // Apply auth headers as gRPC metadata. The tonic transport uses a
        // different API from HTTP's `with_headers`, so without this
        // `OTEL_EXPORTER_OTLP_HEADERS` (the ADR-0033 auth mechanism) were
        // silently dropped on the gRPC path — the collector 401s and only a
        // flush-time warn surfaced (#426 T1).
        if let Some(metadata) = tonic_metadata_from_headers(&config.headers) {
            builder = builder.with_metadata(metadata);
        }
        builder
            .build()
            .map_err(|e| TelemetryError::OtlpExporter(e.to_string()))
    }

    /// Convert the merged OTLP header set into a tonic [`MetadataMap`]. Keys are
    /// lowercased (gRPC metadata keys are case-insensitive and stored lower);
    /// entries whose name or value isn't valid ASCII metadata are skipped with a
    /// warning rather than aborting export.
    #[cfg(feature = "otlp-grpc")]
    pub(super) fn tonic_metadata_from_headers(
        headers: &std::collections::BTreeMap<String, String>,
    ) -> Option<tonic::metadata::MetadataMap> {
        use tonic::metadata::{MetadataKey, MetadataMap, MetadataValue};
        if headers.is_empty() {
            return None;
        }
        let mut md = MetadataMap::with_capacity(headers.len());
        for (k, v) in headers {
            if let (Ok(key), Ok(val)) = (
                MetadataKey::from_bytes(k.to_ascii_lowercase().as_bytes()),
                MetadataValue::try_from(v.as_str()),
            ) {
                md.insert(key, val);
            } else {
                tracing::warn!(
                    target: caliban_common::tracing_targets::TARGET_TELEMETRY,
                    header = %k,
                    "skipping OTLP gRPC header with invalid metadata name or value",
                );
            }
        }
        Some(md)
    }

    #[cfg(not(feature = "otlp-grpc"))]
    fn build_grpc(
        _config: &TelemetryConfig,
    ) -> Result<opentelemetry_otlp::SpanExporter, TelemetryError> {
        Err(TelemetryError::OtlpExporter(
            "OTEL_EXPORTER_OTLP_PROTOCOL=grpc requested but the `otlp-grpc` \
             transport feature is not compiled in"
                .into(),
        ))
    }

    #[cfg(feature = "otlp-http")]
    fn build_http(
        config: &TelemetryConfig,
    ) -> Result<opentelemetry_otlp::SpanExporter, TelemetryError> {
        use opentelemetry_otlp::{Protocol, SpanExporter, WithExportConfig, WithHttpConfig};
        let protocol = match classify_protocol(&config.protocol) {
            OtlpProtocol::HttpJson => Protocol::HttpJson,
            _ => Protocol::HttpBinary,
        };
        let mut builder = SpanExporter::builder().with_http().with_protocol(protocol);
        if let Some(endpoint) = config.endpoint.as_deref() {
            builder = builder.with_endpoint(endpoint);
        }
        if !config.headers.is_empty() {
            let headers: std::collections::HashMap<String, String> = config
                .headers
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            builder = builder.with_headers(headers);
        }
        builder
            .build()
            .map_err(|e| TelemetryError::OtlpExporter(e.to_string()))
    }

    #[cfg(not(feature = "otlp-http"))]
    fn build_http(
        _config: &TelemetryConfig,
    ) -> Result<opentelemetry_otlp::SpanExporter, TelemetryError> {
        Err(TelemetryError::OtlpExporter(
            "OTEL_EXPORTER_OTLP_PROTOCOL=http/* requested but the `otlp-http` \
             transport feature is not compiled in"
                .into(),
        ))
    }

    /// Build a batch-exporting `TracerProvider` from the parsed config.
    /// Must be called inside a Tokio runtime (the batch processor spawns a
    /// background task via `runtime::Tokio`).
    pub(super) fn build_tracer_provider(
        config: &TelemetryConfig,
        app_version: &str,
    ) -> Result<TracerProvider, TelemetryError> {
        use opentelemetry_sdk::runtime;
        let exporter = build_span_exporter(config)?;
        let provider = TracerProvider::builder()
            .with_batch_exporter(exporter, runtime::Tokio)
            .with_resource(caliban_resource(app_version))
            .build();
        Ok(provider)
    }

    /// Select and build the OTLP **metric** exporter for the configured
    /// protocol, mirroring [`build_span_exporter`] (endpoint + auth headers).
    #[cfg(feature = "otlp")]
    fn build_metric_exporter(
        config: &TelemetryConfig,
    ) -> Result<opentelemetry_otlp::MetricExporter, TelemetryError> {
        match classify_protocol(&config.protocol) {
            OtlpProtocol::Grpc => build_metric_grpc(config),
            OtlpProtocol::HttpBinary | OtlpProtocol::HttpJson => build_metric_http(config),
            OtlpProtocol::Unsupported => Err(TelemetryError::OtlpExporter(format!(
                "unsupported OTEL_EXPORTER_OTLP_PROTOCOL {:?} \
                 (expected grpc | http/protobuf | http/json)",
                config.protocol
            ))),
        }
    }

    #[cfg(feature = "otlp-grpc")]
    fn build_metric_grpc(
        config: &TelemetryConfig,
    ) -> Result<opentelemetry_otlp::MetricExporter, TelemetryError> {
        use opentelemetry_otlp::{MetricExporter, WithExportConfig, WithTonicConfig};
        let mut builder = MetricExporter::builder().with_tonic();
        if let Some(endpoint) = config.endpoint.as_deref() {
            builder = builder.with_endpoint(endpoint);
        }
        if let Some(metadata) = tonic_metadata_from_headers(&config.headers) {
            builder = builder.with_metadata(metadata);
        }
        builder
            .build()
            .map_err(|e| TelemetryError::OtlpExporter(e.to_string()))
    }

    #[cfg(not(feature = "otlp-grpc"))]
    fn build_metric_grpc(
        _config: &TelemetryConfig,
    ) -> Result<opentelemetry_otlp::MetricExporter, TelemetryError> {
        Err(TelemetryError::OtlpExporter(
            "OTEL_EXPORTER_OTLP_PROTOCOL=grpc requested but the `otlp-grpc` \
             transport feature is not compiled in"
                .into(),
        ))
    }

    #[cfg(feature = "otlp-http")]
    fn build_metric_http(
        config: &TelemetryConfig,
    ) -> Result<opentelemetry_otlp::MetricExporter, TelemetryError> {
        use opentelemetry_otlp::{MetricExporter, Protocol, WithExportConfig, WithHttpConfig};
        let protocol = match classify_protocol(&config.protocol) {
            OtlpProtocol::HttpJson => Protocol::HttpJson,
            _ => Protocol::HttpBinary,
        };
        let mut builder = MetricExporter::builder()
            .with_http()
            .with_protocol(protocol);
        if let Some(endpoint) = config.endpoint.as_deref() {
            builder = builder.with_endpoint(endpoint);
        }
        if !config.headers.is_empty() {
            let headers: std::collections::HashMap<String, String> = config
                .headers
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            builder = builder.with_headers(headers);
        }
        builder
            .build()
            .map_err(|e| TelemetryError::OtlpExporter(e.to_string()))
    }

    #[cfg(not(feature = "otlp-http"))]
    fn build_metric_http(
        _config: &TelemetryConfig,
    ) -> Result<opentelemetry_otlp::MetricExporter, TelemetryError> {
        Err(TelemetryError::OtlpExporter(
            "OTEL_EXPORTER_OTLP_PROTOCOL=http/* requested but the `otlp-http` \
             transport feature is not compiled in"
                .into(),
        ))
    }

    /// Build the OTLP metrics `SdkMeterProvider`: a `PeriodicReader` that
    /// exports on `OTEL_METRIC_EXPORT_INTERVAL` (the previously-unused
    /// `metric_export_interval`) wrapping the OTLP metric exporter (#427).
    /// Must be called inside a Tokio runtime (the reader spawns a background
    /// export task via `runtime::Tokio`).
    #[cfg(feature = "otlp")]
    pub(super) fn build_meter_provider(
        config: &TelemetryConfig,
        app_version: &str,
    ) -> Result<opentelemetry_sdk::metrics::SdkMeterProvider, TelemetryError> {
        use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
        use opentelemetry_sdk::runtime;
        let exporter = build_metric_exporter(config)?;
        let reader = PeriodicReader::builder(exporter, runtime::Tokio)
            .with_interval(config.metric_export_interval)
            .build();
        let provider = SdkMeterProvider::builder()
            .with_reader(reader)
            .with_resource(caliban_resource(app_version))
            .build();
        Ok(provider)
    }
}

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
    /// Merged headers (env + helper, helper wins), resolved once at startup:
    /// `Telemetry::init_from_env` invokes the headers-helper (if any) via
    /// `refresh_dynamic_headers` and merges its output here before the exporter
    /// is built. There is no background refresh thread (#426 T3).
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
                    target: caliban_common::tracing_targets::TARGET_TELEMETRY,
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
    /// Real OTLP span-export provider, built when telemetry is enabled and the
    /// exporter feature is compiled in. Held so `shutdown` can force-flush the
    /// batch processor and so `otel_layer` can hand a tracer to the
    /// `tracing-opentelemetry` bridge. `None` when disabled or the exporter
    /// setup failed (export off; cost/metrics unaffected). Cheaply clonable —
    /// it wraps an `Arc` internally.
    #[cfg(feature = "otlp")]
    tracer_provider: Option<opentelemetry_sdk::trace::TracerProvider>,
    /// Real OTLP metrics provider, built alongside `tracer_provider` when
    /// telemetry is enabled. Held so `shutdown` can force-flush the periodic
    /// reader before exit. `None` when disabled / setup failed (#427).
    #[cfg(feature = "otlp")]
    meter_provider: Option<opentelemetry_sdk::metrics::SdkMeterProvider>,
}

impl Telemetry {
    /// Read the env and construct telemetry.
    ///
    /// # Errors
    /// Surfaces rate-card parse failures from the embedded YAML — these are
    /// fatal misconfigurations.
    pub fn init_from_env(session_id: &str) -> Result<Self, TelemetryError> {
        let mut config = TelemetryConfig::from_env();
        // Apply the headers-helper (if configured) once, at startup, so its
        // dynamic auth headers actually reach the exporter (#426 T3). Previously
        // `refresh_dynamic_headers` had no callers, so the helper's output was
        // never merged. Only bother when telemetry is enabled.
        if config.enabled {
            config.refresh_dynamic_headers();
        }
        let standard = StandardAttrs::from_env(session_id, env!("CARGO_PKG_VERSION"));

        let card = match std::env::var("CALIBAN_RATES_YAML").ok() {
            Some(p) if !p.is_empty() => RateCard::from_path(&p)?,
            _ => RateCard::embedded()?,
        };
        let cost = Arc::new(CostAccumulator::new(card));
        let context = Arc::new(ContextWindow::new());
        let recorder = Arc::new(InMemoryRecorder::new());

        // Build the OTLP metrics pipeline (MeterProvider + PeriodicReader) when
        // telemetry is enabled, `OTEL_METRICS_EXPORTER` selects otlp, and a
        // Tokio runtime is present (the reader spawns a background export task).
        // Setup failures degrade to metrics-recorded-but-not-exported (#427).
        #[cfg(feature = "otlp")]
        let meter_provider = if config.enabled
            && config.metrics_exporter == "otlp"
            && tokio::runtime::Handle::try_current().is_ok()
        {
            match otlp_pipeline::build_meter_provider(&config, env!("CARGO_PKG_VERSION")) {
                Ok(provider) => {
                    opentelemetry::global::set_meter_provider(provider.clone());
                    Some(provider)
                }
                Err(e) => {
                    tracing::warn!(
                        target: caliban_common::tracing_targets::TARGET_TELEMETRY,
                        error = %e,
                        "otlp metric exporter setup failed; metric export disabled (cost/usage still recorded)",
                    );
                    None
                }
            }
        } else {
            None
        };

        // Attach the OTel instruments to the emitter (when a provider was built)
        // so every emit reaches the collector, then emit session.count{start}.
        let metrics = {
            let base = MetricEmitter::with_recorder(standard.clone(), recorder, config.enabled);
            #[cfg(feature = "otlp")]
            let base = match &meter_provider {
                Some(provider) => {
                    use opentelemetry::metrics::MeterProvider as _;
                    base.with_otel_meter(provider.meter("caliban"))
                }
                None => base,
            };
            base
        };
        if config.enabled {
            metrics.emit_session("start");
        }

        // Build the real OTLP span-export pipeline when telemetry is enabled
        // and the exporter feature is compiled in. The batch span processor
        // spawns a background task via `runtime::Tokio`, so a Tokio runtime must
        // be present — the caliban binary is `#[tokio::main]`. Guard on that so
        // non-async callers (e.g. unit tests) don't panic; they just get no
        // span export. Setup failures degrade to export-off rather than
        // aborting startup — cost accounting and metrics stay live.
        #[cfg(feature = "otlp")]
        let tracer_provider = if config.enabled {
            if tokio::runtime::Handle::try_current().is_ok() {
                match otlp_pipeline::build_tracer_provider(&config, env!("CARGO_PKG_VERSION")) {
                    Ok(provider) => {
                        opentelemetry::global::set_tracer_provider(provider.clone());
                        Some(provider)
                    }
                    Err(e) => {
                        tracing::warn!(
                            target: caliban_common::tracing_targets::TARGET_TELEMETRY,
                            error = %e,
                            "otlp span exporter setup failed; span export disabled (cost/metrics unaffected)",
                        );
                        None
                    }
                }
            } else {
                tracing::warn!(
                    target: caliban_common::tracing_targets::TARGET_TELEMETRY,
                    "telemetry enabled but init ran outside a Tokio runtime; OTLP span export disabled",
                );
                None
            }
        } else {
            None
        };

        Ok(Self {
            enabled: config.enabled,
            standard,
            metrics,
            cost,
            context,
            config,
            #[cfg(feature = "otlp")]
            tracer_provider,
            #[cfg(feature = "otlp")]
            meter_provider,
        })
    }

    /// Build a `tracing-opentelemetry` layer that feeds emitted `tracing`
    /// spans into the OTLP export pipeline. Returns `None` when telemetry is
    /// disabled or no span exporter was built. The caller attaches the layer
    /// to its `tracing_subscriber` registry via `.with(...)`.
    #[cfg(feature = "otlp")]
    #[must_use]
    pub fn otel_layer<S>(&self) -> Option<BoxedLayer<S>>
    where
        S: tracing::Subscriber
            + for<'span> tracing_subscriber::registry::LookupSpan<'span>
            + Send
            + Sync,
    {
        use opentelemetry::trace::TracerProvider as _;
        let provider = self.tracer_provider.as_ref()?;
        let tracer = provider.tracer("caliban");
        Some(Box::new(
            tracing_opentelemetry::layer::<S>().with_tracer(tracer),
        ))
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
            #[cfg(feature = "otlp")]
            tracer_provider: None,
            #[cfg(feature = "otlp")]
            meter_provider: None,
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
    /// Currently never returns an error. Exporter flush/shutdown faults are
    /// logged and swallowed (best-effort at process exit); the signature
    /// reserves `Err` for future fatal cases.
    pub fn shutdown(self) -> Result<(), TelemetryError> {
        if self.enabled {
            self.metrics.emit_session("end");
        }
        // Force-flush + stop the metrics pipeline so the final interval's
        // metrics (incl. session.count{end} just emitted) reach the collector
        // before exit (#427).
        #[cfg(feature = "otlp")]
        if let Some(provider) = self.meter_provider.as_ref() {
            if let Err(e) = provider.force_flush() {
                tracing::warn!(
                    target: caliban_common::tracing_targets::TARGET_TELEMETRY,
                    error = %e,
                    "otlp metric force-flush error during shutdown",
                );
            }
            if let Err(e) = provider.shutdown() {
                tracing::warn!(
                    target: caliban_common::tracing_targets::TARGET_TELEMETRY,
                    error = %e,
                    "otlp meter provider shutdown error",
                );
            }
        }
        // Force-flush batched spans and stop the exporter so nothing is lost
        // before the process exits.
        #[cfg(feature = "otlp")]
        if let Some(provider) = self.tracer_provider.as_ref() {
            for result in provider.force_flush() {
                if let Err(e) = result {
                    tracing::warn!(
                        target: caliban_common::tracing_targets::TARGET_TELEMETRY,
                        error = %e,
                        "otlp span force-flush error during shutdown",
                    );
                }
            }
            if let Err(e) = provider.shutdown() {
                tracing::warn!(
                    target: caliban_common::tracing_targets::TARGET_TELEMETRY,
                    error = %e,
                    "otlp tracer provider shutdown error",
                );
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
            #[cfg(feature = "otlp")]
            tracer_provider: None,
            #[cfg(feature = "otlp")]
            meter_provider: None,
        };
        telemetry.shutdown().unwrap();
        let evts = recorder.by_name("caliban.session.count");
        assert!(
            evts.iter().any(|e| e.attr("phase") == Some("end")),
            "shutdown emits session.count{{phase=end}}",
        );
    }
}

// Hermetic tests for the real OTLP span-export pipeline. Compiled only when the
// exporter feature is on — which `cargo test --workspace` turns on via the
// caliban binary's feature selection. No network: the span-export test uses the
// SDK's in-memory exporter, and the builder tests only construct exporters
// (OTLP connects lazily, on first export).
#[cfg(all(test, feature = "otlp"))]
mod otlp_tests {
    use super::*;

    /// A parsed config that does not touch the process env, so tests are
    /// order-independent and hermetic.
    fn test_config(protocol: &str) -> TelemetryConfig {
        TelemetryConfig {
            enabled: true,
            endpoint: None,
            protocol: protocol.to_string(),
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
        }
    }

    #[cfg(unix)]
    #[test]
    fn refresh_dynamic_headers_merges_helper_output() {
        // #426 T3: the headers-helper output must actually reach `config.headers`
        // (helper wins on collision). `init_from_env` invokes this at startup.
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("helper.sh");
        let mut f = std::fs::File::create(&script).unwrap();
        writeln!(f, "#!/bin/sh\necho 'authorization=Bearer dynamic'").unwrap();
        f.flush().unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        drop(f);

        let mut config = test_config("grpc");
        config
            .headers
            .insert("authorization".into(), "Bearer static".into());
        config.headers_helper = Some(crate::headers::HeadersHelperConfig::new(script));
        config.refresh_dynamic_headers();

        assert_eq!(
            config.headers.get("authorization").map(String::as_str),
            Some("Bearer dynamic"),
            "helper header should override the static env header",
        );
    }

    // (c) config → transport mapping.
    #[test]
    fn classify_protocol_maps_known_and_unknown() {
        assert_eq!(classify_protocol("grpc"), OtlpProtocol::Grpc);
        assert_eq!(classify_protocol("http/protobuf"), OtlpProtocol::HttpBinary);
        assert_eq!(classify_protocol("http/json"), OtlpProtocol::HttpJson);
        assert_eq!(classify_protocol("  grpc  "), OtlpProtocol::Grpc);
        assert_eq!(classify_protocol("thrift"), OtlpProtocol::Unsupported);
        assert_eq!(classify_protocol(""), OtlpProtocol::Unsupported);
    }

    // Resource carries service.name = caliban (+ version).
    #[test]
    fn resource_carries_service_name() {
        use opentelemetry::Key;
        let resource = otlp_pipeline::caliban_resource("9.9.9");
        assert_eq!(
            resource
                .get(Key::from_static_str("service.name"))
                .map(|v| v.as_str().into_owned()),
            Some("caliban".to_string()),
        );
        assert_eq!(
            resource
                .get(Key::from_static_str("service.version"))
                .map(|v| v.as_str().into_owned()),
            Some("9.9.9".to_string()),
        );
    }

    // (b) an unrecognized protocol returns an error rather than panicking. The
    // "transport feature compiled out" branch returns the same error variant by
    // construction (see `build_grpc`/`build_http` cfg pairs); it is only
    // reachable in a build where that transport is off, which the workspace
    // build is not, so it is asserted conditionally below.
    #[test]
    fn unsupported_protocol_is_error_not_panic() {
        let err = otlp_pipeline::build_span_exporter(&test_config("thrift"));
        assert!(err.is_err(), "unrecognized protocol must return Err");
    }

    #[cfg(not(feature = "otlp-grpc"))]
    #[test]
    fn grpc_without_transport_feature_errors() {
        assert!(otlp_pipeline::build_span_exporter(&test_config("grpc")).is_err());
    }

    #[cfg(not(feature = "otlp-http"))]
    #[test]
    fn http_without_transport_feature_errors() {
        assert!(otlp_pipeline::build_span_exporter(&test_config("http/protobuf")).is_err());
    }

    // (c) endpoint + headers wiring: the HTTP exporter builder accepts the
    // configured endpoint/headers and constructs (OTLP connects lazily, so no
    // network and no batch runtime is involved here).
    #[cfg(feature = "otlp-http")]
    #[test]
    fn http_exporter_accepts_endpoint_and_headers() {
        for protocol in ["http/protobuf", "http/json"] {
            let mut config = test_config(protocol);
            config.endpoint = Some("http://localhost:4318".into());
            config
                .headers
                .insert("authorization".into(), "Bearer xyz".into());
            let exporter = otlp_pipeline::build_span_exporter(&config);
            assert!(
                exporter.is_ok(),
                "{protocol} exporter should build: {:?}",
                exporter.err()
            );
        }
    }

    // gRPC exporter construction needs a Tokio runtime for its lazy channel;
    // multi-threaded so the batch/channel setup never deadlocks a single
    // worker. No collector is contacted (connect is lazy).
    #[cfg(feature = "otlp-grpc")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn grpc_exporter_accepts_endpoint_and_headers() {
        // #426 T1: the gRPC exporter must carry auth headers (as tonic metadata)
        // — previously they were silently dropped. Building with headers set
        // must succeed (metadata is applied at build time).
        let mut config = test_config("grpc");
        config.endpoint = Some("http://localhost:4317".into());
        config
            .headers
            .insert("authorization".into(), "Bearer xyz".into());
        let exporter = otlp_pipeline::build_span_exporter(&config);
        assert!(
            exporter.is_ok(),
            "grpc exporter should build with headers: {:?}",
            exporter.err()
        );
    }

    #[cfg(feature = "otlp-grpc")]
    #[test]
    fn tonic_metadata_from_headers_lowercases_and_skips_invalid() {
        use std::collections::BTreeMap;
        // Empty → None (no metadata applied).
        assert!(otlp_pipeline::tonic_metadata_from_headers(&BTreeMap::new()).is_none());

        let mut headers = BTreeMap::new();
        // Mixed-case key must be accepted (gRPC keys are stored lowercase).
        headers.insert("Authorization".to_string(), "Bearer tok".to_string());
        // A key with a space is not a valid metadata name → skipped, not fatal.
        headers.insert("bad key".to_string(), "v".to_string());
        let md = otlp_pipeline::tonic_metadata_from_headers(&headers).expect("some metadata");
        assert_eq!(md.len(), 1, "invalid-name header should have been skipped");
        assert_eq!(
            md.get("authorization").map(|v| v.to_str().unwrap()),
            Some("Bearer tok"),
            "auth header missing or key not lowercased",
        );
    }

    // (a) a span emitted through `tracing` reaches the exporter via the layer,
    // and the provider's resource carries service.name = caliban.
    #[test]
    fn tracing_span_exports_through_layer() {
        use opentelemetry::Key;
        use opentelemetry::trace::TracerProvider as _;
        use opentelemetry_sdk::testing::trace::InMemorySpanExporterBuilder;
        use opentelemetry_sdk::trace::TracerProvider;
        use tracing_subscriber::Registry;
        use tracing_subscriber::layer::SubscriberExt as _;

        let exporter = InMemorySpanExporterBuilder::new().build();
        let resource = otlp_pipeline::caliban_resource("9.9.9");
        // Assert the resource we hand to the provider carries service.name.
        assert_eq!(
            resource
                .get(Key::from_static_str("service.name"))
                .map(|v| v.as_str().into_owned()),
            Some("caliban".to_string()),
        );

        let provider = TracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .with_resource(resource)
            .build();
        let tracer = provider.tracer("caliban");
        let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
        let subscriber = Registry::default().with(otel_layer);

        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("agent-run", run.id = 1_u64);
            let _enter = span.enter();
            tracing::info!("inside the agent-run span");
        });

        let spans = exporter.get_finished_spans().expect("in-memory spans");
        assert!(
            spans.iter().any(|s| s.name == "agent-run"),
            "expected the agent-run span to be exported, got: {:?}",
            spans.iter().map(|s| &s.name).collect::<Vec<_>>(),
        );
    }
}
