//! `RouterConfig` — TOML schema for the model router.

use serde::Deserialize;

use caliban_provider::RequestPurpose;

/// One entry in the router config: which provider+model handles which purpose.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RouteEntry {
    /// The request category this route applies to.
    pub purpose: RequestPurpose,
    /// Logical name of the provider to dispatch to (must appear in the
    /// `providers` map handed to `ModelRouter::build`).
    pub provider: String,
    /// Model id passed through to the chosen provider.
    pub model: String,
}

/// Top-level router config (the `[router]` section of `caliban.toml`).
#[derive(Debug, Clone, Deserialize)]
pub struct RouterConfig {
    /// Purpose used when the request's `metadata.purpose` is `None`.
    pub default_purpose: RequestPurpose,
    /// Route entries in declaration order. Routes earlier in the vec take
    /// priority for their purpose; later entries with the same purpose form
    /// the (v2) fallback chain.
    #[serde(default, rename = "route")]
    pub routes: Vec<RouteEntry>,
}

#[derive(Debug, Deserialize)]
struct CalibanFile {
    router: Option<RouterConfig>,
}

/// Parse a `caliban.toml` body, returning the `[router]` section if present.
///
/// # Errors
/// Returns a `toml::de::Error` if the body cannot be parsed.
pub fn parse_router_config(body: &str) -> Result<Option<RouterConfig>, toml::de::Error> {
    let file: CalibanFile = toml::from_str(body)?;
    Ok(file.router)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_config() {
        let body = r#"
[router]
default_purpose = "main_loop"

[[router.route]]
purpose = "main_loop"
provider = "anthropic"
model = "claude-3-5-sonnet"
"#;
        let cfg = parse_router_config(body).unwrap().unwrap();
        assert_eq!(cfg.default_purpose, RequestPurpose::MainLoop);
        assert_eq!(cfg.routes.len(), 1);
        assert_eq!(cfg.routes[0].provider, "anthropic");
    }

    #[test]
    fn parses_multi_purpose_config() {
        let body = r#"
[router]
default_purpose = "main_loop"

[[router.route]]
purpose = "main_loop"
provider = "anthropic"
model = "claude-3-5-sonnet"

[[router.route]]
purpose = "summarization"
provider = "anthropic"
model = "claude-3-5-haiku"

[[router.route]]
purpose = "fast_classifier"
provider = "ollama"
model = "llama3.2:3b"
"#;
        let cfg = parse_router_config(body).unwrap().unwrap();
        assert_eq!(cfg.routes.len(), 3);
        assert_eq!(cfg.routes[1].purpose, RequestPurpose::Summarization);
        assert_eq!(cfg.routes[2].provider, "ollama");
    }

    #[test]
    fn absent_router_section_returns_none() {
        let body = "[other]\nfoo = 1\n";
        let cfg = parse_router_config(body).unwrap();
        assert!(cfg.is_none());
    }

    #[test]
    fn invalid_purpose_errors() {
        let body = r#"
[router]
default_purpose = "not_a_real_purpose"
"#;
        assert!(parse_router_config(body).is_err());
    }
}
