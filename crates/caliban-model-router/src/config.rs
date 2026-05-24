//! `RouterConfig` — TOML schema for the model router.
//!
//! v2 extends v1 with `id`, `requires`, `fallback`, `hedge`, `breaker`,
//! `effort`, and per-route `effort_map` fields, plus global `[router.breaker]`
//! / `[router.hedge]` defaults and `[provider.X]` adapter blocks.

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;

use caliban_provider::RequestPurpose;

/// Effort level abstract over per-adapter reasoning knobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffortLevel {
    /// Lowest effort: short responses, minimal reasoning budget.
    Low,
    /// Medium effort (default for most routes).
    Medium,
    /// High effort: deep reasoning / extended thinking enabled.
    High,
}

impl EffortLevel {
    /// Stable string slug used in `effort_map` table lookups.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            EffortLevel::Low => "low",
            EffortLevel::Medium => "medium",
            EffortLevel::High => "high",
        }
    }
}

/// Per-route effort → provider-specific knob mapping. Keys are the strings
/// `"low"`, `"medium"`, `"high"`; values are opaque tokens the adapter
/// interprets (e.g. Anthropic thinking budget, OpenAI reasoning_effort).
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct EffortMap {
    /// Knob string for `EffortLevel::Low`.
    #[serde(default)]
    pub low: Option<String>,
    /// Knob string for `EffortLevel::Medium`.
    #[serde(default)]
    pub medium: Option<String>,
    /// Knob string for `EffortLevel::High`.
    #[serde(default)]
    pub high: Option<String>,
}

impl EffortMap {
    /// Look up the knob string for a level. Returns the level's name as a
    /// fallback when no mapping is configured.
    #[must_use]
    pub fn for_level(&self, level: EffortLevel) -> &str {
        match level {
            EffortLevel::Low => self.low.as_deref().unwrap_or("low"),
            EffortLevel::Medium => self.medium.as_deref().unwrap_or("medium"),
            EffortLevel::High => self.high.as_deref().unwrap_or("high"),
        }
    }

    /// `true` if any field is set.
    #[must_use]
    pub fn is_configured(&self) -> bool {
        self.low.is_some() || self.medium.is_some() || self.high.is_some()
    }
}

/// Declared capability requirements a route imposes. Routes only see requests
/// that satisfy these.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
pub struct CapabilityRequirements {
    /// Route only accepts requests with image content.
    #[serde(default)]
    pub vision: bool,
    /// Route only accepts requests with a thinking budget.
    #[serde(default)]
    pub thinking: bool,
    /// Route only accepts requests with non-empty tool declarations.
    #[serde(default)]
    pub tool_use: bool,
}

/// Hedge policy: a configurable race-on-delay between the primary route and
/// hedge targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HedgePolicy {
    /// Hedging disabled (default).
    #[default]
    Disabled,
    /// Race after `hedge_after`; launch at most `max_hedges` extra
    /// candidates beyond the primary.
    Race {
        /// Delay before launching the next candidate.
        hedge_after: Duration,
        /// Maximum number of additional candidates launched.
        max_hedges: u8,
    },
}

/// Wire-format for hedge policy. Accepts `false` to mean Disabled.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum HedgeWire {
    /// Either `hedge = false` (disabled) or `hedge = true` (use default).
    Toggle(bool),
    /// Inline table form.
    Table {
        #[serde(default)]
        hedge_after_ms: Option<u64>,
        #[serde(default)]
        max_hedges: Option<u8>,
        #[serde(default, rename = "max")]
        max_alias: Option<u8>,
    },
}

/// Circuit breaker policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BreakerPolicy {
    /// Trip after this many failures within `window`.
    pub failure_threshold: u32,
    /// Sliding window the failures must occur within.
    pub window: Duration,
    /// How long the breaker stays Tripped before moving to HalfOpen.
    pub cooldown: Duration,
    /// Number of successful probes required to close from HalfOpen.
    pub half_open_probes: u32,
}

impl BreakerPolicy {
    /// "Effectively disabled" — needs an unreachable number of failures.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            failure_threshold: u32::MAX,
            window: Duration::from_secs(60),
            cooldown: Duration::from_secs(0),
            half_open_probes: 1,
        }
    }

    /// `true` if the policy is the disabled sentinel.
    #[must_use]
    pub fn is_disabled(&self) -> bool {
        self.failure_threshold == u32::MAX
    }
}

impl Default for BreakerPolicy {
    fn default() -> Self {
        Self::disabled()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum BreakerWire {
    Toggle(bool),
    Table {
        #[serde(default)]
        failure_threshold: Option<u32>,
        #[serde(default)]
        window_secs: Option<u64>,
        #[serde(default)]
        cooldown_secs: Option<u64>,
        #[serde(default)]
        half_open_probes: Option<u32>,
    },
}

#[derive(Debug, Clone, Default, Deserialize)]
struct BreakerDefaults {
    #[serde(default)]
    failure_threshold: Option<u32>,
    #[serde(default)]
    window_secs: Option<u64>,
    #[serde(default)]
    cooldown_secs: Option<u64>,
    #[serde(default)]
    half_open_probes: Option<u32>,
}

impl BreakerDefaults {
    fn to_policy(&self) -> BreakerPolicy {
        BreakerPolicy {
            failure_threshold: self.failure_threshold.unwrap_or(u32::MAX),
            window: Duration::from_secs(self.window_secs.unwrap_or(60)),
            cooldown: Duration::from_secs(self.cooldown_secs.unwrap_or(30)),
            half_open_probes: self.half_open_probes.unwrap_or(1),
        }
    }

    fn merge_with(&self, override_wire: &BreakerWire) -> BreakerPolicy {
        match override_wire {
            BreakerWire::Toggle(false) => BreakerPolicy::disabled(),
            BreakerWire::Toggle(true) => self.to_policy(),
            BreakerWire::Table {
                failure_threshold,
                window_secs,
                cooldown_secs,
                half_open_probes,
            } => BreakerPolicy {
                failure_threshold: failure_threshold
                    .or(self.failure_threshold)
                    .unwrap_or(u32::MAX),
                window: Duration::from_secs(window_secs.or(self.window_secs).unwrap_or(60)),
                cooldown: Duration::from_secs(cooldown_secs.or(self.cooldown_secs).unwrap_or(30)),
                half_open_probes: half_open_probes.or(self.half_open_probes).unwrap_or(1),
            },
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
struct HedgeDefaults {
    #[serde(default)]
    hedge_after_ms: Option<u64>,
    #[serde(default)]
    max_hedges: Option<u8>,
}

impl HedgeDefaults {
    fn to_policy(&self) -> HedgePolicy {
        match (self.hedge_after_ms, self.max_hedges) {
            (None, None) => HedgePolicy::Disabled,
            (after, max) => HedgePolicy::Race {
                hedge_after: Duration::from_millis(after.unwrap_or(1000)),
                max_hedges: max.unwrap_or(1),
            },
        }
    }

    fn merge_with(&self, override_wire: &HedgeWire) -> HedgePolicy {
        match override_wire {
            HedgeWire::Toggle(false) => HedgePolicy::Disabled,
            HedgeWire::Toggle(true) => self.to_policy(),
            HedgeWire::Table {
                hedge_after_ms,
                max_hedges,
                max_alias,
            } => HedgePolicy::Race {
                hedge_after: Duration::from_millis(
                    hedge_after_ms.or(self.hedge_after_ms).unwrap_or(1000),
                ),
                max_hedges: max_hedges.or(*max_alias).or(self.max_hedges).unwrap_or(1),
            },
        }
    }
}

/// Wire-format raw route entry parsed from TOML.
#[derive(Debug, Clone, Deserialize)]
#[allow(clippy::struct_field_names)] // matches the schema name `effort_map`.
struct RawRoute {
    #[serde(default)]
    id: Option<String>,
    purpose: RequestPurpose,
    provider: String,
    model: String,
    #[serde(default)]
    requires: Option<CapabilityRequirements>,
    #[serde(default)]
    fallback: Option<Vec<String>>,
    #[serde(default)]
    hedge: Option<HedgeWire>,
    #[serde(default)]
    breaker: Option<BreakerWire>,
    #[serde(default)]
    effort: Option<EffortLevel>,
    #[serde(default)]
    effort_map: Option<EffortMap>,
}

/// One entry in the router config: which provider+model handles which purpose,
/// plus optional fallback/hedge/breaker/capability policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteEntry {
    /// Stable route id (defaults to `{provider}:{model}:{purpose-slug}` when
    /// omitted at the config level).
    pub id: String,
    /// The request category this route applies to.
    pub purpose: RequestPurpose,
    /// Logical name of the provider to dispatch to (must appear in the
    /// `providers` map handed to `ModelRouter::build`).
    pub provider: String,
    /// Model id passed through to the chosen provider.
    pub model: String,
    /// Declared capability requirements (the route only accepts requests that
    /// satisfy these).
    pub requires: CapabilityRequirements,
    /// Explicit fallback list (ordered route ids). `None` means "implicit:
    /// declaration order over the same purpose". `Some(vec![])` explicitly
    /// disables fallback.
    pub fallback: Option<Vec<String>>,
    /// Hedge policy (resolved with global defaults).
    pub hedge: HedgePolicy,
    /// Breaker policy (resolved with global defaults).
    pub breaker: BreakerPolicy,
    /// Default effort level for this route (caller can override per-request).
    pub effort: Option<EffortLevel>,
    /// Per-route effort knob map.
    pub effort_map: EffortMap,
}

impl RouteEntry {
    /// Pick the effort knob string for a level, falling back to the level
    /// name when the route's `effort_map` is missing the entry.
    #[must_use]
    pub fn effort_knob_for(&self, level: EffortLevel) -> &str {
        self.effort_map.for_level(level)
    }
}

/// Provider construction block (`[provider.X]`).
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct ProviderBlock {
    /// Env var name to read the API key from (overrides the adapter's default).
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Base URL override (e.g. for proxies / local-host backends).
    #[serde(default)]
    pub base_url: Option<String>,
}

/// Top-level router config (the `[router]` section of `caliban.toml`).
#[derive(Debug, Clone)]
pub struct RouterConfig {
    /// Purpose used when the request's `metadata.purpose` is `None`.
    pub default_purpose: RequestPurpose,
    /// Route entries in declaration order.
    pub routes: Vec<RouteEntry>,
}

/// Resolved view of the full `caliban.toml` content relevant to the router.
#[derive(Debug, Clone)]
pub struct CalibanConfig {
    /// `[router]` + `[[router.route]]`, if present.
    pub router: Option<RouterConfig>,
    /// `[provider.X]` blocks, keyed by provider name.
    pub providers: HashMap<String, ProviderBlock>,
}

// -- raw deserialization ---------------------------------------------------

#[derive(Debug, Deserialize)]
struct CalibanFile {
    #[serde(default)]
    router: Option<RouterSection>,
    #[serde(default)]
    provider: HashMap<String, ProviderBlock>,
}

#[derive(Debug, Deserialize)]
struct RouterSection {
    default_purpose: RequestPurpose,
    #[serde(default)]
    breaker: BreakerDefaults,
    #[serde(default)]
    hedge: HedgeDefaults,
    #[serde(default, rename = "route")]
    routes: Vec<RawRoute>,
}

fn purpose_slug(p: RequestPurpose) -> &'static str {
    match p {
        RequestPurpose::MainLoop => "main_loop",
        RequestPurpose::Summarization => "summarization",
        RequestPurpose::FastClassifier => "fast_classifier",
        RequestPurpose::SubAgent => "sub_agent",
        RequestPurpose::Embedding => "embedding",
        RequestPurpose::Other => "other",
    }
}

fn derive_id(raw: &RawRoute) -> String {
    raw.id.clone().unwrap_or_else(|| {
        format!(
            "{}:{}:{}",
            raw.provider,
            raw.model,
            purpose_slug(raw.purpose)
        )
    })
}

impl RouterConfig {
    fn from_section(s: RouterSection) -> Self {
        let routes: Vec<RouteEntry> = s
            .routes
            .into_iter()
            .map(|raw| {
                let id = derive_id(&raw);
                let hedge = match raw.hedge.as_ref() {
                    Some(w) => s.hedge.merge_with(w),
                    None => s.hedge.to_policy(),
                };
                let breaker = match raw.breaker.as_ref() {
                    Some(w) => s.breaker.merge_with(w),
                    None => s.breaker.to_policy(),
                };
                RouteEntry {
                    id,
                    purpose: raw.purpose,
                    provider: raw.provider,
                    model: raw.model,
                    requires: raw.requires.unwrap_or_default(),
                    fallback: raw.fallback,
                    hedge,
                    breaker,
                    effort: raw.effort,
                    effort_map: raw.effort_map.unwrap_or_default(),
                }
            })
            .collect();
        RouterConfig {
            default_purpose: s.default_purpose,
            routes,
        }
    }
}

/// Parse a `caliban.toml` body, returning the `[router]` section if present.
///
/// # Errors
/// Returns a `toml::de::Error` if the body cannot be parsed.
pub fn parse_router_config(body: &str) -> Result<Option<RouterConfig>, toml::de::Error> {
    let file: CalibanFile = toml::from_str(body)?;
    Ok(file.router.map(RouterConfig::from_section))
}

/// Parse a `caliban.toml` body into the full caliban-config view.
///
/// # Errors
/// Returns a `toml::de::Error` if the body cannot be parsed.
pub fn parse_caliban_config(body: &str) -> Result<CalibanConfig, toml::de::Error> {
    let file: CalibanFile = toml::from_str(body)?;
    Ok(CalibanConfig {
        router: file.router.map(RouterConfig::from_section),
        providers: file.provider,
    })
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
        assert_eq!(cfg.routes[0].id, "anthropic:claude-3-5-sonnet:main_loop");
        assert!(cfg.routes[0].breaker.is_disabled());
        assert!(matches!(cfg.routes[0].hedge, HedgePolicy::Disabled));
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

    #[test]
    fn parses_full_v2_config_with_fallback_hedge_breaker_requires() {
        let body = r#"
[router]
default_purpose = "main_loop"

[router.breaker]
failure_threshold = 5
window_secs = 60
cooldown_secs = 30
half_open_probes = 1

[router.hedge]
hedge_after_ms = 1000
max_hedges = 1

[[router.route]]
id = "main-primary"
purpose = "main_loop"
provider = "anthropic"
model = "claude-sonnet"
requires = { vision = true, tool_use = true }
fallback = ["main-openai"]
effort = "high"

[[router.route]]
id = "main-openai"
purpose = "main_loop"
provider = "openai"
model = "gpt-5"
"#;
        let cfg = parse_router_config(body).unwrap().unwrap();
        assert_eq!(cfg.routes.len(), 2);
        assert_eq!(cfg.routes[0].id, "main-primary");
        assert_eq!(
            cfg.routes[0].fallback.as_deref(),
            Some(&["main-openai".to_string()][..])
        );
        assert!(cfg.routes[0].requires.vision);
        assert!(cfg.routes[0].requires.tool_use);
        assert_eq!(cfg.routes[0].effort, Some(EffortLevel::High));
        // Inherits global hedge / breaker defaults.
        assert!(matches!(cfg.routes[0].hedge, HedgePolicy::Race { .. }));
        assert!(!cfg.routes[0].breaker.is_disabled());
        assert_eq!(cfg.routes[0].breaker.failure_threshold, 5);
    }

    #[test]
    fn per_route_breaker_disables_with_false() {
        let body = r#"
[router]
default_purpose = "main_loop"

[router.breaker]
failure_threshold = 5

[[router.route]]
purpose = "main_loop"
provider = "anthropic"
model = "x"
breaker = false
"#;
        let cfg = parse_router_config(body).unwrap().unwrap();
        assert!(cfg.routes[0].breaker.is_disabled());
    }

    #[test]
    fn per_route_hedge_overrides_global() {
        let body = r#"
[router]
default_purpose = "main_loop"

[router.hedge]
hedge_after_ms = 1000
max_hedges = 1

[[router.route]]
purpose = "main_loop"
provider = "anthropic"
model = "x"
hedge = { hedge_after_ms = 300, max = 2 }
"#;
        let cfg = parse_router_config(body).unwrap().unwrap();
        let HedgePolicy::Race {
            hedge_after,
            max_hedges,
        } = cfg.routes[0].hedge
        else {
            panic!("expected race");
        };
        assert_eq!(hedge_after, Duration::from_millis(300));
        assert_eq!(max_hedges, 2);
    }

    #[test]
    fn parses_provider_blocks() {
        let body = r#"
[router]
default_purpose = "main_loop"

[[router.route]]
purpose = "main_loop"
provider = "openai"
model = "gpt"

[provider.openai]
api_key_env = "OPENAI_API_KEY_DEV"
base_url = "https://oai.example.test"

[provider.ollama]
base_url = "http://localhost:11434"
"#;
        let cfg = parse_caliban_config(body).unwrap();
        assert_eq!(cfg.providers.len(), 2);
        assert_eq!(
            cfg.providers["openai"].api_key_env.as_deref(),
            Some("OPENAI_API_KEY_DEV")
        );
        assert_eq!(
            cfg.providers["ollama"].base_url.as_deref(),
            Some("http://localhost:11434")
        );
    }

    #[test]
    fn effort_map_lookups_have_sane_fallback() {
        let m = EffortMap {
            low: None,
            medium: Some("medium-knob".into()),
            high: None,
        };
        assert_eq!(m.for_level(EffortLevel::Low), "low");
        assert_eq!(m.for_level(EffortLevel::Medium), "medium-knob");
        assert_eq!(m.for_level(EffortLevel::High), "high");
    }
}
