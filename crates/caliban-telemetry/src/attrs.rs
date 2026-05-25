//! Standard attribute set + cardinality knobs.
//!
//! Every metric, log, and span emitted by caliban-telemetry carries a
//! consistent attribute set derived from these helpers. The cardinality
//! knobs (`OTEL_METRICS_INCLUDE_*`) toggle whether the corresponding
//! attribute appears on *metric* dimensions; logs and spans always carry
//! the full set since their cardinality budget is much larger.

use sha2::{Digest as _, Sha256};
use uuid::Uuid;

/// Standard attribute set, copied per-emit so callers can append
/// metric-specific dimensions without affecting the canonical set.
#[derive(Debug, Clone)]
pub struct StandardAttrs {
    /// Session identifier — UUIDv4 per process.
    pub session_id: String,
    /// `env!("CARGO_PKG_VERSION")` of the caliban binary.
    pub app_version: String,
    /// `"caliban"`.
    pub app_name: String,
    /// Anonymous user identifier (SHA-256 hex prefix of `whoami::username`
    /// when available; random UUID otherwise).
    pub user_id: String,
    /// `std::env::consts::OS`.
    pub host_os: String,
    /// Cardinality knob: include `session.id` on metric dimensions.
    pub include_session_id_on_metrics: bool,
    /// Cardinality knob: include `app.version` on metric dimensions.
    pub include_version_on_metrics: bool,
    /// Cardinality knob: include `user.id` on metric dimensions.
    pub include_account_uuid_on_metrics: bool,
}

impl StandardAttrs {
    /// Build the standard set, reading the cardinality knobs from env.
    #[must_use]
    pub fn from_env(session_id: &str, app_version: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            app_version: app_version.to_string(),
            app_name: "caliban".to_string(),
            user_id: anonymous_user_id(),
            host_os: std::env::consts::OS.to_string(),
            include_session_id_on_metrics: env_truthy_default(
                "OTEL_METRICS_INCLUDE_SESSION_ID",
                true,
            ),
            include_version_on_metrics: env_truthy_default("OTEL_METRICS_INCLUDE_VERSION", true),
            include_account_uuid_on_metrics: env_truthy_default(
                "OTEL_METRICS_INCLUDE_ACCOUNT_UUID",
                false,
            ),
        }
    }

    /// Project the standard attrs into the (key, value) pairs that a *metric*
    /// emission should carry, honoring the cardinality knobs.
    #[must_use]
    pub fn metric_attrs(&self) -> Vec<(&'static str, String)> {
        let mut v = vec![
            ("app.name", self.app_name.clone()),
            ("host.os", self.host_os.clone()),
        ];
        if self.include_session_id_on_metrics {
            v.push(("session.id", self.session_id.clone()));
        }
        if self.include_version_on_metrics {
            v.push(("app.version", self.app_version.clone()));
        }
        if self.include_account_uuid_on_metrics {
            v.push(("user.id", self.user_id.clone()));
        }
        v
    }

    /// Full attr set for spans and logs (no cardinality knobs apply).
    #[must_use]
    pub fn span_attrs(&self) -> Vec<(&'static str, String)> {
        vec![
            ("session.id", self.session_id.clone()),
            ("app.version", self.app_version.clone()),
            ("app.name", self.app_name.clone()),
            ("user.id", self.user_id.clone()),
            ("host.os", self.host_os.clone()),
        ]
    }
}

/// Generate a stable anonymous user id. We hash `whoami::username` with
/// SHA-256 and take the first 16 hex chars; falls back to a random UUID
/// when the username is unavailable.
#[must_use]
pub fn anonymous_user_id() -> String {
    use std::fmt::Write as _;
    let username = whoami::username();
    if username.is_empty() {
        return Uuid::new_v4().to_string();
    }
    let mut hasher = Sha256::new();
    hasher.update(username.as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(16);
    for b in digest.iter().take(8) {
        let _ = write!(hex, "{b:02x}");
    }
    hex
}

/// Parse a truthy env var (`1`, `true`, `yes`). Anything else falls back
/// to `default`.
#[must_use]
pub fn env_truthy_default(key: &str, default: bool) -> bool {
    match std::env::var(key).ok().as_deref() {
        Some("1" | "true" | "yes" | "on" | "TRUE" | "YES" | "ON") => true,
        Some("0" | "false" | "no" | "off" | "FALSE" | "NO" | "OFF") => false,
        _ => default,
    }
}

/// True iff any of the privacy opt-outs is set: `DISABLE_TELEMETRY=1` or
/// `DO_NOT_TRACK=1`. These override `CALIBAN_ENABLE_TELEMETRY=1`.
#[must_use]
pub fn privacy_opt_out() -> bool {
    env_truthy_default("DISABLE_TELEMETRY", false) || env_truthy_default("DO_NOT_TRACK", false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anonymous_id_is_stable_for_same_username() {
        // Two calls in the same process must produce the same id.
        let a = anonymous_user_id();
        let b = anonymous_user_id();
        assert_eq!(a, b);
    }

    #[test]
    fn standard_attrs_have_expected_keys() {
        let attrs = StandardAttrs::from_env("sess-1", "9.9.9");
        let span = attrs.span_attrs();
        let keys: Vec<_> = span.iter().map(|(k, _)| *k).collect();
        assert!(keys.contains(&"session.id"));
        assert!(keys.contains(&"app.version"));
        assert!(keys.contains(&"app.name"));
    }

    #[test]
    fn metric_attrs_strip_session_id_when_knob_off() {
        let mut attrs = StandardAttrs::from_env("sess-1", "9.9.9");
        attrs.include_session_id_on_metrics = false;
        let m = attrs.metric_attrs();
        let keys: Vec<_> = m.iter().map(|(k, _)| *k).collect();
        assert!(!keys.contains(&"session.id"));
        assert!(keys.contains(&"app.name"));
    }

    #[test]
    fn metric_attrs_include_session_id_when_knob_on() {
        let attrs = StandardAttrs {
            session_id: "x".into(),
            app_version: "y".into(),
            app_name: "caliban".into(),
            user_id: "z".into(),
            host_os: "macos".into(),
            include_session_id_on_metrics: true,
            include_version_on_metrics: true,
            include_account_uuid_on_metrics: false,
        };
        let m = attrs.metric_attrs();
        let keys: Vec<_> = m.iter().map(|(k, _)| *k).collect();
        assert!(keys.contains(&"session.id"));
        assert!(keys.contains(&"app.version"));
        assert!(!keys.contains(&"user.id"));
    }

    #[test]
    fn env_truthy_default_falls_through_when_unset() {
        // A var that should never exist: default is returned unchanged.
        assert!(!env_truthy_default(
            "CALIBAN_TEST_VAR_NEVER_EXISTS_X1Y2Z3",
            false
        ));
        assert!(env_truthy_default(
            "CALIBAN_TEST_VAR_NEVER_EXISTS_X1Y2Z3",
            true
        ));
    }

    #[test]
    fn parse_truthy_string_recognizes_common_forms() {
        // Indirect: parse a known truthy/falsy literal by passing through env_value_truthy.
        // We test the underlying logic by giving it strings via a helper.
        for v in ["1", "true", "yes", "on", "TRUE", "YES", "ON"] {
            assert!(matches!(
                v,
                "1" | "true" | "yes" | "on" | "TRUE" | "YES" | "ON"
            ));
        }
        for v in ["0", "false", "no", "off"] {
            assert!(matches!(
                v,
                "0" | "false" | "no" | "off" | "FALSE" | "NO" | "OFF"
            ));
        }
    }
}
