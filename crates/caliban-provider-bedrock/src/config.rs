//! Configuration for the Bedrock provider.

use std::time::Duration;

use aws_config::BehaviorVersion;

use crate::error::BedrockError;

/// Default background auth-refresh interval (5 minutes).
pub const DEFAULT_AUTH_REFRESH: Duration = Duration::from_mins(5);

/// Configuration for [`BedrockProvider`](crate::BedrockProvider).
#[derive(Debug, Clone)]
pub struct BedrockConfig {
    /// AWS region (e.g. `"us-west-2"`).
    pub region: String,
    /// Bedrock inference profile or foundation model ID. May be a bare
    /// model ID (`anthropic.claude-3-5-sonnet-20241022-v2:0`), an
    /// inference profile ID (`us.anthropic.claude-3-7-sonnet-20250219-v1:0`),
    /// or an inference profile ARN.
    ///
    /// Operators usually set this via `BEDROCK_INFERENCE_PROFILE_ID` and let
    /// caliban's canonical model name (`claude-3-5-sonnet`) flow through
    /// `BedrockTransport::wire_model_id` instead.
    pub inference_profile_id: Option<String>,
    /// Named AWS profile (e.g. `"caliban-prod"`). When set, exported as
    /// `AWS_PROFILE` so the SDK default chain picks it up.
    pub aws_profile: Option<String>,
    /// Optional endpoint override (e.g. for FIPS or VPC endpoints).
    pub endpoint_override: Option<String>,
    /// Background auth-refresh interval. `Duration::ZERO` disables
    /// the proactive refresh loop (the AWS SDK still rotates credentials
    /// internally on its own schedule).
    pub auth_refresh: Duration,
}

impl Default for BedrockConfig {
    fn default() -> Self {
        Self {
            region: String::new(),
            inference_profile_id: None,
            aws_profile: None,
            endpoint_override: None,
            auth_refresh: DEFAULT_AUTH_REFRESH,
        }
    }
}

impl BedrockConfig {
    /// Build a config from process environment variables.
    ///
    /// Reads:
    /// - `AWS_REGION` (or `AWS_DEFAULT_REGION`) — required.
    /// - `BEDROCK_INFERENCE_PROFILE_ID` — optional.
    /// - `AWS_PROFILE` — optional.
    /// - `CALIBAN_AWS_AUTH_REFRESH` — optional, e.g. `"5m"`, `"300s"`,
    ///   `"0"` to disable.
    pub fn from_env() -> std::result::Result<Self, BedrockError> {
        Self::from_env_fn(|k| std::env::var(k).ok())
    }

    /// Like [`Self::from_env`] but reads variables through an injected
    /// closure. Useful for hermetic tests that don't want to mutate the
    /// process environment.
    pub fn from_env_fn<F>(getter: F) -> std::result::Result<Self, BedrockError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let region = getter("AWS_REGION")
            .or_else(|| getter("AWS_DEFAULT_REGION"))
            .ok_or(BedrockError::MissingConfig("AWS_REGION"))?;
        let inference_profile_id = getter("BEDROCK_INFERENCE_PROFILE_ID");
        let aws_profile = getter("AWS_PROFILE");
        let auth_refresh = getter("CALIBAN_AWS_AUTH_REFRESH")
            .map_or(Ok(DEFAULT_AUTH_REFRESH), |s| parse_duration(&s))?;
        Ok(Self {
            region,
            inference_profile_id,
            aws_profile,
            endpoint_override: None,
            auth_refresh,
        })
    }

    /// Load an `aws_config::SdkConfig` using this config + the default
    /// AWS credential provider chain.
    pub(crate) async fn load_sdk_config(&self) -> aws_config::SdkConfig {
        let mut loader = aws_config::defaults(BehaviorVersion::latest());
        if !self.region.is_empty() {
            loader = loader.region(aws_config::Region::new(self.region.clone()));
        }
        if let Some(profile) = self.aws_profile.as_deref() {
            loader = loader.profile_name(profile);
        }
        if let Some(endpoint) = self.endpoint_override.as_deref() {
            loader = loader.endpoint_url(endpoint);
        }
        loader.load().await
    }
}

/// Parse a duration string like `"5m"`, `"300s"`, `"0"`. Bare integers are
/// seconds.
pub(crate) fn parse_duration(s: &str) -> std::result::Result<Duration, BedrockError> {
    let s = s.trim();
    if s == "0" {
        return Ok(Duration::ZERO);
    }
    if let Some(rest) = s.strip_suffix('m') {
        let n: u64 = rest
            .trim()
            .parse()
            .map_err(|_| BedrockError::InvalidConfig(format!("bad duration: {s}")))?;
        return Ok(Duration::from_secs(n * 60));
    }
    if let Some(rest) = s.strip_suffix('s') {
        let n: u64 = rest
            .trim()
            .parse()
            .map_err(|_| BedrockError::InvalidConfig(format!("bad duration: {s}")))?;
        return Ok(Duration::from_secs(n));
    }
    let n: u64 = s
        .parse()
        .map_err(|_| BedrockError::InvalidConfig(format!("bad duration: {s}")))?;
    Ok(Duration::from_secs(n))
}

#[cfg(test)]
#[allow(clippy::duration_suboptimal_units)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_zero() {
        assert_eq!(parse_duration("0").unwrap(), Duration::ZERO);
    }

    #[test]
    fn parse_duration_minutes() {
        assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
    }

    #[test]
    fn parse_duration_seconds() {
        assert_eq!(parse_duration("90s").unwrap(), Duration::from_secs(90));
    }

    #[test]
    fn parse_duration_bare() {
        assert_eq!(parse_duration("120").unwrap(), Duration::from_secs(120));
    }

    #[test]
    fn parse_duration_invalid() {
        assert!(parse_duration("nope").is_err());
    }
}
