//! End-to-end tests of the env-var contract.
//!
//! These tests mutate process env (`std::env::set_var`) so they MUST run
//! serially. We achieve that with a process-wide mutex.

use std::sync::{LazyLock, Mutex, MutexGuard};

use caliban_telemetry::{Telemetry, TelemetryConfig, env_truthy_default, privacy_opt_out};

/// Process-wide mutex serializing env mutations. Lock for the whole test.
static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn env_guard() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Helper that scrubs every env var the telemetry contract reads. Run inside
/// each test under `env_guard` so order independence is preserved.
fn clean_env() {
    let keys = [
        "CALIBAN_ENABLE_TELEMETRY",
        "DISABLE_TELEMETRY",
        "DO_NOT_TRACK",
        "OTEL_EXPORTER_OTLP_ENDPOINT",
        "OTEL_EXPORTER_OTLP_PROTOCOL",
        "OTEL_EXPORTER_OTLP_HEADERS",
        "OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE",
        "OTEL_EXPORTER_OTLP_CLIENT_KEY",
        "OTEL_EXPORTER_OTLP_PRIVATE_KEY",
        "OTEL_EXPORTER_OTLP_CERTIFICATE",
        "OTEL_EXPORTER_OTLP_CA_CERTIFICATE",
        "OTEL_METRIC_EXPORT_INTERVAL",
        "OTEL_LOGS_EXPORTER",
        "OTEL_METRICS_EXPORTER",
        "OTEL_TRACES_EXPORTER",
        "OTEL_METRICS_INCLUDE_SESSION_ID",
        "OTEL_METRICS_INCLUDE_VERSION",
        "OTEL_METRICS_INCLUDE_ACCOUNT_UUID",
        "OTEL_LOG_USER_PROMPTS",
        "OTEL_LOG_TOOL_DETAILS",
        "OTEL_LOG_TOOL_CONTENT",
        "OTEL_LOG_RAW_API_BODIES",
        "CALIBAN_OTEL_HEADERS_HELPER",
        "CALIBAN_RATES_YAML",
    ];
    // SAFETY: We hold ENV_LOCK across all callers, serializing env mutations.
    // Tests are the only consumer of these vars. Edition-2024 requires
    // `unsafe` around `set/remove_var`; we wrap in a localized #[allow] so the
    // workspace's `unsafe_code = "deny"` stays in force everywhere else.
    #[allow(unsafe_code)]
    // SAFETY: All env mutations in this test file are serialized via ENV_LOCK
    // (taken in the test entry points). No other thread is reading these vars
    // because telemetry init is local to each test.
    unsafe {
        for k in keys {
            std::env::remove_var(k);
        }
    }
}

#[allow(unsafe_code)]
// SAFETY: Same justification as `clean_env`: we only set env vars while
// holding `ENV_LOCK`, and only inside this test file.
fn set_env(key: &str, val: &str) {
    // SAFETY: serialized through ENV_LOCK at the test entry points.
    unsafe {
        std::env::set_var(key, val);
    }
}

#[test]
fn telemetry_disabled_by_default() {
    let _g = env_guard();
    clean_env();
    let cfg = TelemetryConfig::from_env();
    assert!(!cfg.enabled);
}

#[test]
fn telemetry_enabled_when_env_set() {
    let _g = env_guard();
    clean_env();
    set_env("CALIBAN_ENABLE_TELEMETRY", "1");
    let cfg = TelemetryConfig::from_env();
    assert!(cfg.enabled);
    clean_env();
}

#[test]
fn disable_telemetry_overrides_enable() {
    let _g = env_guard();
    clean_env();
    set_env("CALIBAN_ENABLE_TELEMETRY", "1");
    set_env("DISABLE_TELEMETRY", "1");
    let cfg = TelemetryConfig::from_env();
    assert!(!cfg.enabled, "DISABLE_TELEMETRY must force-disable");
    assert!(privacy_opt_out());
    clean_env();
}

#[test]
fn do_not_track_overrides_enable() {
    let _g = env_guard();
    clean_env();
    set_env("CALIBAN_ENABLE_TELEMETRY", "1");
    set_env("DO_NOT_TRACK", "1");
    let cfg = TelemetryConfig::from_env();
    assert!(!cfg.enabled, "DO_NOT_TRACK must force-disable");
    clean_env();
}

#[test]
fn otel_endpoint_and_protocol_pass_through() {
    let _g = env_guard();
    clean_env();
    set_env("CALIBAN_ENABLE_TELEMETRY", "1");
    set_env(
        "OTEL_EXPORTER_OTLP_ENDPOINT",
        "https://collector.example.com:4318",
    );
    set_env("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf");
    set_env(
        "OTEL_EXPORTER_OTLP_HEADERS",
        "Authorization=Bearer t1,X-Tenant=acme",
    );
    let cfg = TelemetryConfig::from_env();
    assert_eq!(
        cfg.endpoint.as_deref(),
        Some("https://collector.example.com:4318")
    );
    assert_eq!(cfg.protocol, "http/protobuf");
    assert_eq!(
        cfg.headers.get("Authorization").map(String::as_str),
        Some("Bearer t1")
    );
    assert_eq!(
        cfg.headers.get("X-Tenant").map(String::as_str),
        Some("acme")
    );
    clean_env();
}

#[test]
fn metric_export_interval_parses() {
    let _g = env_guard();
    clean_env();
    set_env("OTEL_METRIC_EXPORT_INTERVAL", "5s");
    let cfg = TelemetryConfig::from_env();
    assert_eq!(
        cfg.metric_export_interval,
        std::time::Duration::from_secs(5)
    );
    clean_env();
}

#[test]
fn content_control_envs_default_off() {
    let _g = env_guard();
    clean_env();
    let cfg = TelemetryConfig::from_env();
    assert!(!cfg.log_user_prompts);
    assert!(!cfg.log_tool_details);
    assert!(!cfg.log_tool_content);
    assert_eq!(cfg.log_raw_api_bodies, "0");
}

#[test]
fn content_control_envs_can_be_enabled() {
    let _g = env_guard();
    clean_env();
    set_env("OTEL_LOG_USER_PROMPTS", "1");
    set_env("OTEL_LOG_TOOL_CONTENT", "1");
    let cfg = TelemetryConfig::from_env();
    assert!(cfg.log_user_prompts);
    assert!(cfg.log_tool_content);
    assert!(!cfg.log_tool_details, "OTEL_LOG_TOOL_DETAILS remained off");
    clean_env();
}

#[test]
fn mtls_paths_round_trip() {
    let _g = env_guard();
    clean_env();
    set_env(
        "OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE",
        "/etc/otel/client.pem",
    );
    set_env("OTEL_EXPORTER_OTLP_CLIENT_KEY", "/etc/otel/client.key");
    set_env("OTEL_EXPORTER_OTLP_CA_CERTIFICATE", "/etc/otel/ca.pem");
    let cfg = TelemetryConfig::from_env();
    assert_eq!(
        cfg.client_cert.as_deref().and_then(|p| p.to_str()),
        Some("/etc/otel/client.pem")
    );
    assert_eq!(
        cfg.client_key.as_deref().and_then(|p| p.to_str()),
        Some("/etc/otel/client.key")
    );
    assert_eq!(
        cfg.ca_cert.as_deref().and_then(|p| p.to_str()),
        Some("/etc/otel/ca.pem")
    );
    clean_env();
}

#[test]
fn cardinality_knob_strips_session_id() {
    let _g = env_guard();
    clean_env();
    set_env("OTEL_METRICS_INCLUDE_SESSION_ID", "0");
    let t = Telemetry::disabled_for_tests("sess-x").unwrap();
    // The cardinality knob is read inside StandardAttrs::from_env, which
    // disabled_for_tests calls — verify session.id is omitted from metric attrs.
    let attrs = t.standard.metric_attrs();
    let keys: Vec<_> = attrs.iter().map(|(k, _)| *k).collect();
    assert!(!keys.contains(&"session.id"));
    assert!(keys.contains(&"app.name"));
    clean_env();
}

#[test]
fn init_from_env_emits_session_start_when_enabled() {
    let _g = env_guard();
    clean_env();
    set_env("CALIBAN_ENABLE_TELEMETRY", "1");
    let t = Telemetry::init_from_env("sess-init").unwrap();
    assert!(t.enabled);
    let starts = t.metrics.recorder().by_name("caliban.session.count");
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0].attr("phase"), Some("start"));
    // app.version should be CARGO_PKG_VERSION of the crate.
    assert_eq!(
        starts[0].attr("app.version"),
        Some(env!("CARGO_PKG_VERSION"))
    );
    // session.id passes through to attrs.
    assert_eq!(starts[0].attr("session.id"), Some("sess-init"));
    clean_env();
}

#[test]
fn init_from_env_does_not_emit_when_disabled() {
    let _g = env_guard();
    clean_env();
    let t = Telemetry::init_from_env("sess-init").unwrap();
    assert!(!t.enabled);
    let starts = t.metrics.recorder().by_name("caliban.session.count");
    assert!(starts.is_empty());
}

#[test]
fn env_truthy_default_falls_back() {
    let _g = env_guard();
    clean_env();
    // Unset var: default returned.
    assert!(env_truthy_default("ABSENT_KEY_FOR_TEST_XYZ", true));
    assert!(!env_truthy_default("ABSENT_KEY_FOR_TEST_XYZ", false));
    set_env("CALIBAN_TEST_TRUTHY", "1");
    assert!(env_truthy_default("CALIBAN_TEST_TRUTHY", false));
    clean_env();
}
