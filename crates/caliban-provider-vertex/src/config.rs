//! Configuration for the Vertex provider.

use std::time::Duration;

use crate::error::VertexError;

/// Default background auth-refresh interval (5 minutes).
pub const DEFAULT_AUTH_REFRESH: Duration = Duration::from_mins(5);

/// Vertex publisher path (currently always `"anthropic"`).
pub const VERTEX_PUBLISHER: &str = "anthropic";

/// Configuration for [`VertexProvider`](crate::VertexProvider).
#[derive(Debug, Clone)]
pub struct VertexConfig {
    /// GCP project ID.
    pub project_id: String,
    /// Vertex region (e.g. `"us-east5"`, `"europe-west1"`).
    pub region: String,
    /// Optional path to a service-account JSON file. When `None`, the
    /// `gcp_auth` default chain is used (ADC, gcloud user creds, GCE
    /// metadata server). When `Some`, exposed via
    /// `GOOGLE_APPLICATION_CREDENTIALS`.
    pub service_account_key_path: Option<String>,
    /// Background auth-refresh interval. `Duration::ZERO` disables the
    /// proactive refresh loop.
    pub auth_refresh: Duration,
}

impl Default for VertexConfig {
    fn default() -> Self {
        Self {
            project_id: String::new(),
            region: String::new(),
            service_account_key_path: None,
            auth_refresh: DEFAULT_AUTH_REFRESH,
        }
    }
}

impl VertexConfig {
    /// Build a config from process environment variables.
    ///
    /// Reads:
    /// - `VERTEX_PROJECT_ID` (or `GOOGLE_CLOUD_PROJECT`) — required.
    /// - `VERTEX_REGION` — required.
    /// - `GOOGLE_APPLICATION_CREDENTIALS` — optional.
    /// - `CALIBAN_GCP_AUTH_REFRESH` — optional duration string.
    pub fn from_env() -> std::result::Result<Self, VertexError> {
        Self::from_env_fn(|k| std::env::var(k).ok())
    }

    /// Like [`Self::from_env`] but reads variables through an injected
    /// closure. Useful for hermetic tests that don't want to mutate the
    /// process environment.
    pub fn from_env_fn<F>(getter: F) -> std::result::Result<Self, VertexError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let project_id = getter("VERTEX_PROJECT_ID")
            .or_else(|| getter("GOOGLE_CLOUD_PROJECT"))
            .ok_or(VertexError::MissingConfig("VERTEX_PROJECT_ID"))?;
        let region = getter("VERTEX_REGION").ok_or(VertexError::MissingConfig("VERTEX_REGION"))?;
        let service_account_key_path = getter("GOOGLE_APPLICATION_CREDENTIALS");
        let auth_refresh = getter("CALIBAN_GCP_AUTH_REFRESH")
            .map_or(Ok(DEFAULT_AUTH_REFRESH), |s| parse_duration(&s))?;
        Ok(Self {
            project_id,
            region,
            service_account_key_path,
            auth_refresh,
        })
    }
}

pub(crate) fn parse_duration(s: &str) -> std::result::Result<Duration, VertexError> {
    let s = s.trim();
    if s == "0" {
        return Ok(Duration::ZERO);
    }
    if let Some(rest) = s.strip_suffix('m') {
        let n: u64 = rest
            .trim()
            .parse()
            .map_err(|_| VertexError::InvalidConfig(format!("bad duration: {s}")))?;
        return Ok(Duration::from_secs(n * 60));
    }
    if let Some(rest) = s.strip_suffix('s') {
        let n: u64 = rest
            .trim()
            .parse()
            .map_err(|_| VertexError::InvalidConfig(format!("bad duration: {s}")))?;
        return Ok(Duration::from_secs(n));
    }
    let n: u64 = s
        .parse()
        .map_err(|_| VertexError::InvalidConfig(format!("bad duration: {s}")))?;
    Ok(Duration::from_secs(n))
}

#[cfg(test)]
#[allow(clippy::duration_suboptimal_units)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_minutes() {
        assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
    }

    #[test]
    fn parse_duration_seconds() {
        assert_eq!(parse_duration("45s").unwrap(), Duration::from_secs(45));
    }

    #[test]
    fn parse_duration_zero_disables() {
        assert_eq!(parse_duration("0").unwrap(), Duration::ZERO);
    }

    #[test]
    fn parse_duration_invalid() {
        assert!(parse_duration("x").is_err());
    }
}
