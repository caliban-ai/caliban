//! Auto-mode classifier — fast-model consult that labels each tool call
//! `allow`/`soft_deny`/`hard_deny` (ADR 0029).
//!
//! Static rule pre-pass walks `hard_deny → soft_deny → allow → environment`
//! pattern lists in declaration order before falling through to the model.
//! The model call goes through the configured `Provider` (typically a router
//! resolving `RequestPurpose::FastClassifier` to a Haiku-class model).
//!
//! Results are cached for the session under `(tool_name,
//! sha256(canonical_input))` to avoid re-classifying identical calls.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use caliban_provider::{
    CompletionRequest, ContentBlock, Message, Provider, RequestMetadata, RequestPurpose, ToolChoice,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::hooks::ToolCtx;
use crate::permissions::matches_glob;

// ---------------------------------------------------------------------------
// Verdict / Decision
// ---------------------------------------------------------------------------

/// What the classifier says about a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoVerdict {
    /// Allow the call.
    Allow,
    /// Fall through to the Ask modal.
    SoftDeny,
    /// Reject without prompting.
    HardDeny,
}

/// Where the [`AutoVerdict`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionSource {
    /// Matched an [`AutoModeConfig`] static rule.
    StaticRule,
    /// Model classifier returned this verdict.
    Classifier,
    /// LRU cache hit on `(tool, input)`.
    Cached,
    /// Auto-mode disabled via setting/env; always soft-denies.
    DisabledFallback,
    /// Classifier malformed / provider error; soft-deny fallthrough.
    Fallback,
}

/// Full decision returned by [`AutoModeClassifier::classify`].
#[derive(Debug, Clone)]
pub struct AutoModeDecision {
    /// The verdict to apply.
    pub verdict: AutoVerdict,
    /// Short reason string (≤120 chars when classifier-supplied).
    pub reason: String,
    /// Provenance of [`Self::verdict`].
    pub source: DecisionSource,
}

// ---------------------------------------------------------------------------
// AutoModeConfig — pattern lists
// ---------------------------------------------------------------------------

/// Sentinel string accepted inside any [`AutoModeConfig`] list. Expands to
/// a curated, version-pinned default list returned by [`default_patterns`].
pub const DEFAULTS_TOKEN: &str = "$defaults";

/// Pattern lists for the auto-mode static pre-pass.
///
/// Match order at evaluation time: `hard_deny` → `soft_deny` → `allow` →
/// `environment`. First match wins. The `$defaults` sentinel string in any
/// list is replaced with the matching curated default list.
#[derive(Debug, Clone, Default)]
pub struct AutoModeConfig {
    /// Tool patterns rejected outright.
    pub hard_deny: Vec<String>,
    /// Tool patterns that fall through to the Ask modal.
    pub soft_deny: Vec<String>,
    /// Additional always-allow patterns (beyond `environment`).
    pub allow: Vec<String>,
    /// Read-only / environment-inspection tool patterns auto-allowed
    /// without calling the classifier.
    pub environment: Vec<String>,
    /// When `true`, every call routed to the classifier returns
    /// [`DecisionSource::DisabledFallback`].
    pub disabled: bool,
}

/// The curated, version-pinned default rules baked into the binary. Returned
/// when a list contains the [`DEFAULTS_TOKEN`] sentinel.
///
/// The hand-picked list is deliberately tight — these are the patterns we
/// will reject regardless of operator overrides.
#[must_use]
pub fn default_patterns(kind: DefaultsKind) -> Vec<&'static str> {
    match kind {
        DefaultsKind::Environment => vec!["Read", "Glob", "Grep"],
        DefaultsKind::Allow => vec![
            "Bash:cargo test*",
            "Bash:cargo check*",
            "Bash:cargo clippy*",
        ],
        DefaultsKind::SoftDeny => vec!["Bash:rm *", "Bash:mv *", "Write:**/.env*"],
        DefaultsKind::HardDeny => vec![
            "Bash:sudo *",
            "Bash:rm -rf /*",
            "Bash:curl * | sh*",
            "Bash:* | sh*",
            "WebFetch:http://*",
        ],
    }
}

/// Which default list the [`DEFAULTS_TOKEN`] sentinel expands to.
#[derive(Debug, Clone, Copy)]
pub enum DefaultsKind {
    /// Read-only / environment-inspection.
    Environment,
    /// Always-allow extras.
    Allow,
    /// Fall-through-to-Ask patterns.
    SoftDeny,
    /// Hard reject.
    HardDeny,
}

impl AutoModeConfig {
    /// Replace `$defaults` sentinels with the curated lists for each
    /// category. Idempotent.
    #[must_use]
    pub fn with_defaults_expanded(mut self) -> Self {
        expand_defaults_in_place(&mut self.environment, DefaultsKind::Environment);
        expand_defaults_in_place(&mut self.allow, DefaultsKind::Allow);
        expand_defaults_in_place(&mut self.soft_deny, DefaultsKind::SoftDeny);
        expand_defaults_in_place(&mut self.hard_deny, DefaultsKind::HardDeny);
        self
    }

    /// Static pre-pass: walk `hard_deny → soft_deny → allow → environment`
    /// in declaration order; return the first matching verdict.
    pub(crate) fn static_match(&self, ctx: &ToolCtx<'_>) -> Option<AutoVerdict> {
        if pattern_list_matches(&self.hard_deny, ctx) {
            return Some(AutoVerdict::HardDeny);
        }
        if pattern_list_matches(&self.soft_deny, ctx) {
            return Some(AutoVerdict::SoftDeny);
        }
        if pattern_list_matches(&self.allow, ctx) || pattern_list_matches(&self.environment, ctx) {
            return Some(AutoVerdict::Allow);
        }
        None
    }
}

fn expand_defaults_in_place(list: &mut Vec<String>, kind: DefaultsKind) {
    let mut expanded: Vec<String> = Vec::with_capacity(list.len());
    for entry in list.drain(..) {
        if entry == DEFAULTS_TOKEN {
            for d in default_patterns(kind) {
                expanded.push(d.to_string());
            }
        } else {
            expanded.push(entry);
        }
    }
    *list = expanded;
}

fn pattern_list_matches(list: &[String], ctx: &ToolCtx<'_>) -> bool {
    list.iter().any(|pat| pattern_matches(pat, ctx))
}

fn pattern_matches(pattern: &str, ctx: &ToolCtx<'_>) -> bool {
    let (tool_pat, arg_pat) = match pattern.split_once(':') {
        Some((t, a)) => (t, Some(a)),
        None => (pattern, None),
    };
    if tool_pat != "*" && !matches_glob(tool_pat, ctx.tool_name) {
        return false;
    }
    match arg_pat {
        None => true,
        Some(glob) => crate::permissions::first_arg(ctx.tool_name, ctx.input)
            .as_deref()
            .is_some_and(|arg| matches_glob(glob, arg)),
    }
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

const CACHE_CAPACITY: usize = 256;

#[derive(Debug, Default)]
struct ClassifierCache {
    /// `(tool, sha256(canonical_input))` → decision. We use a `HashMap`
    /// with a soft cap (insertion drops the oldest key when full) — true
    /// LRU is overkill for a 256-entry per-session cache.
    entries: HashMap<(String, [u8; 32]), AutoModeDecision>,
    /// Insertion order so we can drop the oldest when over capacity.
    order: std::collections::VecDeque<(String, [u8; 32])>,
}

impl ClassifierCache {
    fn get(&self, key: &(String, [u8; 32])) -> Option<AutoModeDecision> {
        self.entries.get(key).cloned()
    }

    fn put(&mut self, key: (String, [u8; 32]), decision: AutoModeDecision) {
        if !self.entries.contains_key(&key) {
            self.order.push_back(key.clone());
            if self.order.len() > CACHE_CAPACITY
                && let Some(old) = self.order.pop_front()
            {
                self.entries.remove(&old);
            }
        }
        self.entries.insert(key, decision);
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }
}

/// Build the cache key for a tool call. Inputs are JSON-canonicalized
/// (object keys sorted) before hashing so semantically-identical calls
/// hit the same entry.
fn cache_key(tool_name: &str, input: &serde_json::Value) -> (String, [u8; 32]) {
    let canonical = canonical_json(input);
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 32];
    bytes.copy_from_slice(&digest);
    (tool_name.to_string(), bytes)
}

fn canonical_json(value: &serde_json::Value) -> String {
    use serde_json::Value;
    match value {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into()),
        Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(canonical_json).collect();
            format!("[{}]", inner.join(","))
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let inner: Vec<String> = keys
                .iter()
                .map(|k| {
                    let v = map.get(*k).unwrap_or(&Value::Null);
                    format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap_or_else(|_| "\"\"".into()),
                        canonical_json(v)
                    )
                })
                .collect();
            format!("{{{}}}", inner.join(","))
        }
    }
}

// ---------------------------------------------------------------------------
// AutoModeClassifier
// ---------------------------------------------------------------------------

/// Truncation cap on the tool-input JSON the classifier sees in its prompt.
pub const CLASSIFIER_INPUT_CAP: usize = 4096;

/// Fast-model classifier for auto-mode tool calls.
///
/// Construct one per session — the cache is intentionally process-local and
/// cleared on mode-exit by callers.
pub struct AutoModeClassifier {
    provider: Arc<dyn Provider + Send + Sync>,
    model: String,
    config: AutoModeConfig,
    cache: Mutex<ClassifierCache>,
}

impl std::fmt::Debug for AutoModeClassifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AutoModeClassifier")
            .field("model", &self.model)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl AutoModeClassifier {
    /// Build a classifier bound to `provider` (typically a router) and
    /// `model` (typically a Haiku-class id). The provider should resolve
    /// the [`RequestPurpose::FastClassifier`] route when applicable.
    #[must_use]
    pub fn new(
        provider: Arc<dyn Provider + Send + Sync>,
        model: impl Into<String>,
        config: AutoModeConfig,
    ) -> Self {
        Self {
            provider,
            model: model.into(),
            config: config.with_defaults_expanded(),
            cache: Mutex::new(ClassifierCache::default()),
        }
    }

    /// Read-only view of the static-rule configuration.
    #[must_use]
    pub fn config(&self) -> &AutoModeConfig {
        &self.config
    }

    /// Drop every cached decision. Callers invoke this on mode-exit and on
    /// `/clear`.
    pub fn clear_cache(&self) {
        if let Ok(mut c) = self.cache.lock() {
            c.clear();
        }
    }

    /// Decide what to do with a tool call under auto mode.
    pub async fn classify(&self, ctx: &ToolCtx<'_>) -> AutoModeDecision {
        // 1. Disable kill switch.
        if self.config.disabled {
            return AutoModeDecision {
                verdict: AutoVerdict::SoftDeny,
                reason: "auto mode disabled".into(),
                source: DecisionSource::DisabledFallback,
            };
        }

        let key = cache_key(ctx.tool_name, ctx.input);

        // 2. Cache lookup.
        if let Some(cached) = self.cache.lock().ok().and_then(|c| c.get(&key)) {
            return AutoModeDecision {
                source: DecisionSource::Cached,
                ..cached
            };
        }

        // 3. Static rule pre-pass.
        if let Some(v) = self.config.static_match(ctx) {
            let decision = AutoModeDecision {
                verdict: v,
                reason: "static rule".into(),
                source: DecisionSource::StaticRule,
            };
            if let Ok(mut c) = self.cache.lock() {
                c.put(key, decision.clone());
            }
            return decision;
        }

        // 4. Model call.
        let decision = self.classifier_call(ctx).await;
        if let Ok(mut c) = self.cache.lock() {
            c.put(key, decision.clone());
        }
        decision
    }

    async fn classifier_call(&self, ctx: &ToolCtx<'_>) -> AutoModeDecision {
        let prompt = build_prompt(ctx.tool_name, ctx.input);
        let req = CompletionRequest {
            model: self.model.clone(),
            messages: vec![Message::user_text(prompt)],
            tools: vec![],
            tool_choice: ToolChoice::default(),
            max_tokens: 256,
            temperature: Some(0.0),
            top_p: None,
            top_k: None,
            stop_sequences: vec![],
            thinking: caliban_provider::ThinkingSetting::Auto,
            effort: None,
            metadata: RequestMetadata {
                user_id: None,
                purpose: Some(RequestPurpose::FastClassifier),
            },
        };
        match self.provider.complete(req).await {
            Ok(resp) => {
                let text = extract_text(&resp);
                match parse_classifier_response(&text) {
                    Some((verdict, reason)) => AutoModeDecision {
                        verdict,
                        reason,
                        source: DecisionSource::Classifier,
                    },
                    None => AutoModeDecision {
                        verdict: AutoVerdict::SoftDeny,
                        reason: "classifier output malformed".into(),
                        source: DecisionSource::Fallback,
                    },
                }
            }
            Err(e) => AutoModeDecision {
                verdict: AutoVerdict::SoftDeny,
                reason: format!("classifier error: {e}"),
                source: DecisionSource::Fallback,
            },
        }
    }
}

fn extract_text(resp: &caliban_provider::CompletionResponse) -> String {
    let mut out = String::new();
    for block in &resp.message.content {
        if let ContentBlock::Text(t) = block {
            out.push_str(&t.text);
        }
    }
    out
}

/// Parse the classifier's strict JSON shape `{ "decision": "...", "reason":
/// "..." }`. Accepts the `verdict` key as an alias for `decision` to match
/// older prompt drafts. Returns `None` when the body isn't JSON-parseable or
/// is missing the required keys.
pub fn parse_classifier_response(body: &str) -> Option<(AutoVerdict, String)> {
    let trimmed = body.trim();
    // The model occasionally wraps JSON in a ```json fence. Strip simple
    // surrounding fences before parsing.
    let cleaned = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map_or(trimmed, str::trim_start)
        .strip_suffix("```")
        .map_or(trimmed, str::trim_end);
    let value: serde_json::Value = serde_json::from_str(cleaned).ok()?;
    let decision_str = value
        .get("decision")
        .or_else(|| value.get("verdict"))
        .and_then(|v| v.as_str())?;
    let verdict = match decision_str {
        "allow" => AutoVerdict::Allow,
        "soft_deny" => AutoVerdict::SoftDeny,
        "hard_deny" => AutoVerdict::HardDeny,
        _ => return None,
    };
    let reason = value
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Some((verdict, reason))
}

/// Build the user-message body the classifier sees. The tool-input JSON is
/// truncated at [`CLASSIFIER_INPUT_CAP`] bytes.
#[must_use]
pub fn build_prompt(tool_name: &str, input: &serde_json::Value) -> String {
    let mut input_text = serde_json::to_string(input).unwrap_or_else(|_| "{}".into());
    if input_text.len() > CLASSIFIER_INPUT_CAP {
        input_text.truncate(CLASSIFIER_INPUT_CAP);
        input_text.push_str("…(truncated)");
    }
    format!(
        "You are a permission classifier for an autonomous coding agent. \
         Label this tool call as `allow`, `soft_deny`, or `hard_deny`. \
         Respond with strict JSON: {{\"decision\":\"allow|soft_deny|hard_deny\",\
         \"reason\":\"…\"}}.\n\
         tool: {tool_name}\ninput: {input_text}",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(name: &'a str, input: &'a serde_json::Value) -> ToolCtx<'a> {
        ToolCtx {
            turn_index: 0,
            tool_use_id: "t1",
            tool_name: name,
            input,
            is_read_only: false,
        }
    }

    // --- defaults expansion ---

    #[test]
    fn defaults_token_expands_hard_deny() {
        let cfg = AutoModeConfig {
            hard_deny: vec![DEFAULTS_TOKEN.to_string(), "Bash:tail *".into()],
            ..AutoModeConfig::default()
        }
        .with_defaults_expanded();
        assert!(cfg.hard_deny.iter().any(|p| p == "Bash:sudo *"));
        assert!(cfg.hard_deny.iter().any(|p| p == "Bash:tail *"));
        // No remaining sentinels.
        assert!(!cfg.hard_deny.iter().any(|p| p == DEFAULTS_TOKEN));
    }

    #[test]
    fn defaults_token_expands_all_lists() {
        let cfg = AutoModeConfig {
            hard_deny: vec![DEFAULTS_TOKEN.into()],
            soft_deny: vec![DEFAULTS_TOKEN.into()],
            allow: vec![DEFAULTS_TOKEN.into()],
            environment: vec![DEFAULTS_TOKEN.into()],
            disabled: false,
        }
        .with_defaults_expanded();
        assert!(!cfg.environment.is_empty());
        assert!(!cfg.allow.is_empty());
        assert!(!cfg.soft_deny.is_empty());
        assert!(!cfg.hard_deny.is_empty());
    }

    // --- static rule match ---

    #[test]
    fn static_match_hard_deny_first() {
        let cfg = AutoModeConfig {
            hard_deny: vec!["Bash:sudo *".into()],
            soft_deny: vec!["Bash:*".into()],
            ..AutoModeConfig::default()
        };
        let input = serde_json::json!({"command": "sudo rm /tmp"});
        assert_eq!(
            cfg.static_match(&ctx("Bash", &input)),
            Some(AutoVerdict::HardDeny)
        );
    }

    #[test]
    fn static_match_environment_short_circuits() {
        let cfg = AutoModeConfig {
            environment: vec!["Read".into()],
            ..AutoModeConfig::default()
        };
        let input = serde_json::json!({"path": "/etc/hosts"});
        assert_eq!(
            cfg.static_match(&ctx("Read", &input)),
            Some(AutoVerdict::Allow)
        );
    }

    #[test]
    fn static_match_none_when_no_rule_matches() {
        let cfg = AutoModeConfig::default();
        let input = serde_json::json!({"command": "ls"});
        assert_eq!(cfg.static_match(&ctx("Bash", &input)), None);
    }

    // --- response parser ---

    #[test]
    fn parser_accepts_well_formed_allow() {
        let (v, reason) =
            parse_classifier_response(r#"{"decision":"allow","reason":"read-only"}"#).unwrap();
        assert_eq!(v, AutoVerdict::Allow);
        assert_eq!(reason, "read-only");
    }

    #[test]
    fn parser_accepts_verdict_alias() {
        let (v, _) = parse_classifier_response(r#"{"verdict":"hard_deny","reason":"x"}"#).unwrap();
        assert_eq!(v, AutoVerdict::HardDeny);
    }

    #[test]
    fn parser_strips_json_fence() {
        let body = "```json\n{\"decision\":\"soft_deny\",\"reason\":\"r\"}\n```";
        let (v, _) = parse_classifier_response(body).unwrap();
        assert_eq!(v, AutoVerdict::SoftDeny);
    }

    #[test]
    fn parser_rejects_malformed_json() {
        assert!(parse_classifier_response("not json").is_none());
        assert!(parse_classifier_response(r#"{"decision":"unknown"}"#).is_none());
    }

    // --- prompt builder ---

    #[test]
    fn prompt_truncates_long_input() {
        let big = "x".repeat(CLASSIFIER_INPUT_CAP * 2);
        let input = serde_json::json!({"command": big});
        let prompt = build_prompt("Bash", &input);
        assert!(prompt.contains("…(truncated)"));
        // Body must remain reasonably bounded even after truncation.
        assert!(prompt.len() < CLASSIFIER_INPUT_CAP * 2);
    }

    #[test]
    fn prompt_contains_tool_name() {
        let input = serde_json::json!({"path": "/tmp/a"});
        let prompt = build_prompt("Read", &input);
        assert!(prompt.contains("tool: Read"));
    }

    // --- canonical JSON ---

    #[test]
    fn canonical_json_sorts_object_keys() {
        let a = canonical_json(&serde_json::json!({"b": 1, "a": 2}));
        let b = canonical_json(&serde_json::json!({"a": 2, "b": 1}));
        assert_eq!(a, b);
    }
}
