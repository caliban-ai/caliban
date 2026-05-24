//! `hooks.toml` parser + in-memory config model (ADR 0024).
//!
//! Resolves config from two locations (lower priority wins):
//! 1. `<workspace>/.caliban/hooks.toml` (project scope)
//! 2. `$XDG_CONFIG_HOME/caliban/hooks.toml` (user scope)
//!
//! The unified `settings.json` (ADR 0026) lands later; this module is the
//! authoritative loader until then.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors emitted by the hooks-config loader.
#[derive(thiserror::Error, Debug)]
pub enum HooksConfigError {
    /// IO failure reading a config file.
    #[error("hooks-config: io error reading {path}: {source}")]
    Io {
        /// Path that failed to read.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// TOML parse error.
    #[error("hooks-config: parse error in {path}: {source}")]
    Parse {
        /// Path that failed to parse.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: toml::de::Error,
    },
    /// Validation error (handler missing required field, etc.).
    #[error("hooks-config: invalid handler in {path}: {message}")]
    Invalid {
        /// Path of the offending file.
        path: PathBuf,
        /// Human-readable problem description.
        message: String,
    },
}

// ---------------------------------------------------------------------------
// Public model
// ---------------------------------------------------------------------------

/// Top-level handler kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookHandlerType {
    /// Spawn a child process and read stdin/stdout/exit codes.
    Command,
    /// POST to a URL.
    Http,
    /// Call an MCP server's tool.
    Mcp,
    /// Call the LLM via the model router.
    Prompt,
    /// Delegate to a sub-agent (async-only).
    Agent,
}

impl HookHandlerType {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "command" => Some(Self::Command),
            "http" => Some(Self::Http),
            "mcp" => Some(Self::Mcp),
            "prompt" => Some(Self::Prompt),
            "agent" => Some(Self::Agent),
            _ => None,
        }
    }
}

/// A single parsed handler entry from `hooks.toml`.
#[derive(Debug, Clone)]
pub struct HookHandlerConfig {
    /// Handler dispatch type.
    pub kind: HookHandlerType,
    /// Matcher (tool-name glob or `"*"`).
    pub matcher: String,
    /// Optional permission-rule-style filter (e.g. `Bash:rm *`).
    pub if_pattern: Option<String>,
    /// Hard timeout. Default: 30s.
    pub timeout: Duration,
    /// When `true`, run on a background pool; the decision is ignored.
    pub asynchronous: bool,
    /// `command` field (Command kind).
    pub command: Option<String>,
    /// Extra argv after `command`.
    pub args: Vec<String>,
    /// Extra env to pass.
    pub env: BTreeMap<String, String>,
    /// `url` field (Http kind).
    pub url: Option<String>,
    /// Static request headers (Http kind).
    pub headers: BTreeMap<String, String>,
    /// MCP server name.
    pub mcp_server: Option<String>,
    /// MCP tool name.
    pub mcp_tool: Option<String>,
    /// Sub-agent name.
    pub agent: Option<String>,
    /// Prompt text for the prompt handler.
    pub prompt: Option<String>,
    /// Model identifier for the prompt handler.
    pub model: Option<String>,
    /// JSON schema string for structured-output prompt handlers.
    pub schema: Option<String>,
}

/// Parsed `hooks.toml` configuration.
#[derive(Debug, Clone, Default)]
pub struct HooksConfig {
    /// Top-level kill switch.
    pub disable_all_hooks: bool,
    /// When true, ignore all non-managed-scope hooks.
    pub allow_managed_hooks_only: bool,
    /// HTTP URL allowlist (glob).
    pub allowed_http_hook_urls: Vec<String>,
    /// Env-var allowlist for `${VAR}` expansion in HTTP/command handlers.
    pub http_hook_allowed_env_vars: Vec<String>,
    /// Event-name → ordered handler list. Event names are the `PascalCase`
    /// taxonomy from the ADR (e.g. `"SessionStart"`).
    pub events: BTreeMap<String, Vec<HookHandlerConfig>>,
}

impl HooksConfig {
    /// Load the project + user `hooks.toml` and merge. Project overrides user
    /// for non-array fields; arrays concatenate (project first, then user).
    /// Missing files are not an error.
    ///
    /// # Errors
    /// Returns [`HooksConfigError`] if either file fails to read or parse.
    pub fn load(workspace_root: &Path) -> Result<Self, HooksConfigError> {
        let project = Self::load_one(&workspace_root.join(".caliban/hooks.toml"))?;
        let user = if let Some(d) = dirs::config_dir() {
            Self::load_one(&d.join("caliban/hooks.toml"))?
        } else {
            Self::default()
        };
        Ok(Self::merge(project, user))
    }

    /// Load a single config file. Missing file returns the default.
    ///
    /// # Errors
    /// Returns [`HooksConfigError::Io`] on read errors other than `NotFound`,
    /// [`HooksConfigError::Parse`] on malformed TOML, or
    /// [`HooksConfigError::Invalid`] on handler validation errors.
    pub fn load_one(path: &Path) -> Result<Self, HooksConfigError> {
        let body = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => {
                return Err(HooksConfigError::Io {
                    path: path.to_path_buf(),
                    source: e,
                });
            }
        };
        Self::from_str(&body, path)
    }

    /// Parse `hooks.toml` from a raw string. `path` is used in error messages.
    ///
    /// # Errors
    /// Returns [`HooksConfigError::Parse`] or [`HooksConfigError::Invalid`].
    pub fn from_str(body: &str, path: &Path) -> Result<Self, HooksConfigError> {
        let raw: RawConfig = toml::from_str(body).map_err(|source| HooksConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        let mut cfg = Self {
            disable_all_hooks: raw.disable_all_hooks,
            allow_managed_hooks_only: raw.allow_managed_hooks_only,
            allowed_http_hook_urls: raw.allowed_http_hook_urls,
            http_hook_allowed_env_vars: raw.http_hook_allowed_env_vars,
            events: BTreeMap::new(),
        };
        for (event_name, groups) in raw.hooks {
            let mut out: Vec<HookHandlerConfig> = Vec::new();
            for group in groups {
                let matcher = group.matcher.unwrap_or_else(|| "*".into());
                let if_pattern = group.if_pattern.clone();
                for handler in group.handlers {
                    let h = build_handler(&event_name, &matcher, if_pattern.clone(), handler)
                        .map_err(|message| HooksConfigError::Invalid {
                            path: path.to_path_buf(),
                            message,
                        })?;
                    out.push(h);
                }
            }
            cfg.events.insert(event_name, out);
        }
        Ok(cfg)
    }

    fn merge(mut project: Self, user: Self) -> Self {
        // Top-level scalar OR over both scopes — operator can enable either.
        project.disable_all_hooks = project.disable_all_hooks || user.disable_all_hooks;
        project.allow_managed_hooks_only =
            project.allow_managed_hooks_only || user.allow_managed_hooks_only;
        // Array fields: project entries take priority but user entries append.
        for u in user.allowed_http_hook_urls {
            if !project.allowed_http_hook_urls.contains(&u) {
                project.allowed_http_hook_urls.push(u);
            }
        }
        for u in user.http_hook_allowed_env_vars {
            if !project.http_hook_allowed_env_vars.contains(&u) {
                project.http_hook_allowed_env_vars.push(u);
            }
        }
        for (event, handlers) in user.events {
            project.events.entry(event).or_default().extend(handlers);
        }
        project
    }

    /// Return the count of handlers configured for `event_name`.
    #[must_use]
    pub fn handler_count(&self, event_name: &str) -> usize {
        self.events.get(event_name).map_or(0, Vec::len)
    }

    /// Total handler count across every event. Used by the `/hooks` overlay.
    #[must_use]
    pub fn total_handler_count(&self) -> usize {
        self.events.values().map(Vec::len).sum()
    }
}

// ---------------------------------------------------------------------------
// Raw TOML shapes (private; only used by the parser)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawConfig {
    disable_all_hooks: bool,
    allow_managed_hooks_only: bool,
    allowed_http_hook_urls: Vec<String>,
    http_hook_allowed_env_vars: Vec<String>,
    hooks: BTreeMap<String, Vec<RawMatcherGroup>>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawMatcherGroup {
    matcher: Option<String>,
    #[serde(rename = "if")]
    if_pattern: Option<String>,
    handlers: Vec<RawHandler>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RawHandler {
    #[serde(rename = "type")]
    kind: Option<String>,
    /// Optional per-handler matcher override (rarely used; primarily inherited
    /// from the surrounding matcher group).
    matcher: Option<String>,
    /// Optional per-handler if-pattern.
    #[serde(rename = "if")]
    if_pattern: Option<String>,
    timeout: Option<String>,
    #[serde(rename = "async")]
    asynchronous: bool,
    command: Option<String>,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    url: Option<String>,
    headers: BTreeMap<String, String>,
    mcp: Option<String>,
    tool: Option<String>,
    agent: Option<String>,
    prompt: Option<String>,
    model: Option<String>,
    schema: Option<String>,
}

fn parse_timeout(raw: &str) -> Result<Duration, String> {
    // Accept simple suffixes: "5s", "200ms", "1m", "30" (seconds), "1h".
    let s = raw.trim();
    if s.is_empty() {
        return Err("empty timeout".into());
    }
    if let Some(num_part) = s.strip_suffix("ms") {
        let n: u64 = num_part
            .trim()
            .parse()
            .map_err(|e| format!("invalid timeout `{raw}`: {e}"))?;
        return Ok(Duration::from_millis(n));
    }
    if let Some(num_part) = s.strip_suffix('s') {
        let n: u64 = num_part
            .trim()
            .parse()
            .map_err(|e| format!("invalid timeout `{raw}`: {e}"))?;
        return Ok(Duration::from_secs(n));
    }
    if let Some(num_part) = s.strip_suffix('m') {
        let n: u64 = num_part
            .trim()
            .parse()
            .map_err(|e| format!("invalid timeout `{raw}`: {e}"))?;
        return Ok(Duration::from_secs(n * 60));
    }
    if let Some(num_part) = s.strip_suffix('h') {
        let n: u64 = num_part
            .trim()
            .parse()
            .map_err(|e| format!("invalid timeout `{raw}`: {e}"))?;
        return Ok(Duration::from_secs(n * 3600));
    }
    // Bare integer = seconds.
    let n: u64 = s
        .parse()
        .map_err(|e| format!("invalid timeout `{raw}`: {e}"))?;
    Ok(Duration::from_secs(n))
}

fn build_handler(
    event_name: &str,
    group_matcher: &str,
    group_if_pattern: Option<String>,
    raw: RawHandler,
) -> Result<HookHandlerConfig, String> {
    let kind_str = raw
        .kind
        .ok_or_else(|| format!("handler for {event_name}: missing `type`"))?;
    let kind = HookHandlerType::from_str(&kind_str)
        .ok_or_else(|| format!("handler for {event_name}: unknown type `{kind_str}`"))?;

    let timeout = if let Some(t) = raw.timeout.as_deref() {
        parse_timeout(t)?
    } else {
        Duration::from_secs(30)
    };

    let matcher = raw.matcher.unwrap_or_else(|| group_matcher.to_string());
    let if_pattern = raw.if_pattern.or(group_if_pattern);

    let asynchronous = raw.asynchronous;

    match kind {
        HookHandlerType::Command => {
            if raw.command.is_none() {
                return Err(format!(
                    "handler for {event_name}: command handler missing `command`"
                ));
            }
        }
        HookHandlerType::Http => {
            if raw.url.is_none() {
                return Err(format!(
                    "handler for {event_name}: http handler missing `url`"
                ));
            }
        }
        HookHandlerType::Mcp => {
            if raw.mcp.is_none() || raw.tool.is_none() {
                return Err(format!(
                    "handler for {event_name}: mcp handler requires `mcp` and `tool`"
                ));
            }
        }
        HookHandlerType::Prompt => {
            if raw.prompt.is_none() {
                return Err(format!(
                    "handler for {event_name}: prompt handler missing `prompt`"
                ));
            }
        }
        HookHandlerType::Agent => {
            if raw.agent.is_none() {
                return Err(format!(
                    "handler for {event_name}: agent handler missing `agent`"
                ));
            }
            if !asynchronous {
                return Err(format!(
                    "handler for {event_name}: agent handlers must be async = true"
                ));
            }
        }
    }

    Ok(HookHandlerConfig {
        kind,
        matcher,
        if_pattern,
        timeout,
        asynchronous,
        command: raw.command,
        args: raw.args,
        env: raw.env,
        url: raw.url,
        headers: raw.headers,
        mcp_server: raw.mcp,
        mcp_tool: raw.tool,
        agent: raw.agent,
        prompt: raw.prompt,
        model: raw.model,
        schema: raw.schema,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_returns_default() {
        let path = std::path::Path::new("/definitely/does/not/exist/hooks.toml");
        let cfg = HooksConfig::load_one(path).unwrap();
        assert!(!cfg.disable_all_hooks);
        assert_eq!(cfg.total_handler_count(), 0);
    }

    #[test]
    fn parses_kill_switch() {
        let body = "disable_all_hooks = true\n";
        let cfg = HooksConfig::from_str(body, std::path::Path::new("h.toml")).unwrap();
        assert!(cfg.disable_all_hooks);
    }

    #[test]
    fn parses_command_handler() {
        let body = r#"
[[hooks.SessionStart]]
matcher = "*"
[[hooks.SessionStart.handlers]]
type = "command"
command = "/bin/true"
timeout = "5s"
"#;
        let cfg = HooksConfig::from_str(body, std::path::Path::new("h.toml")).unwrap();
        assert_eq!(cfg.handler_count("SessionStart"), 1);
        let h = &cfg.events["SessionStart"][0];
        assert_eq!(h.kind, HookHandlerType::Command);
        assert_eq!(h.command.as_deref(), Some("/bin/true"));
        assert_eq!(h.timeout, Duration::from_secs(5));
    }

    #[test]
    fn rejects_unknown_handler_type() {
        let body = r#"
[[hooks.SessionStart]]
matcher = "*"
[[hooks.SessionStart.handlers]]
type = "bogus"
"#;
        let err = HooksConfig::from_str(body, std::path::Path::new("h.toml")).unwrap_err();
        assert!(matches!(err, HooksConfigError::Invalid { .. }));
    }

    #[test]
    fn command_handler_missing_command_errors() {
        let body = r#"
[[hooks.PreToolUse]]
matcher = "Bash"
[[hooks.PreToolUse.handlers]]
type = "command"
"#;
        let err = HooksConfig::from_str(body, std::path::Path::new("h.toml")).unwrap_err();
        assert!(matches!(err, HooksConfigError::Invalid { .. }));
    }

    #[test]
    fn http_handler_missing_url_errors() {
        let body = r#"
[[hooks.PreToolUse]]
matcher = "WebFetch"
[[hooks.PreToolUse.handlers]]
type = "http"
"#;
        let err = HooksConfig::from_str(body, std::path::Path::new("h.toml")).unwrap_err();
        assert!(matches!(err, HooksConfigError::Invalid { .. }));
    }

    #[test]
    fn mcp_handler_missing_fields_errors() {
        let body = r#"
[[hooks.PostToolUse]]
matcher = "*"
[[hooks.PostToolUse.handlers]]
type = "mcp"
mcp = "audit-server"
"#;
        let err = HooksConfig::from_str(body, std::path::Path::new("h.toml")).unwrap_err();
        assert!(matches!(err, HooksConfigError::Invalid { .. }));
    }

    #[test]
    fn prompt_handler_missing_prompt_errors() {
        let body = r#"
[[hooks.UserPromptSubmit]]
matcher = "*"
[[hooks.UserPromptSubmit.handlers]]
type = "prompt"
"#;
        let err = HooksConfig::from_str(body, std::path::Path::new("h.toml")).unwrap_err();
        assert!(matches!(err, HooksConfigError::Invalid { .. }));
    }

    #[test]
    fn agent_handler_must_be_async() {
        let body = r#"
[[hooks.FileChanged]]
matcher = "*.rs"
[[hooks.FileChanged.handlers]]
type = "agent"
agent = "code-review"
async = false
"#;
        let err = HooksConfig::from_str(body, std::path::Path::new("h.toml")).unwrap_err();
        assert!(matches!(err, HooksConfigError::Invalid { .. }));
    }

    #[test]
    fn agent_handler_async_ok() {
        let body = r#"
[[hooks.FileChanged]]
matcher = "*.rs"
[[hooks.FileChanged.handlers]]
type = "agent"
agent = "code-review"
async = true
"#;
        let cfg = HooksConfig::from_str(body, std::path::Path::new("h.toml")).unwrap();
        assert_eq!(cfg.handler_count("FileChanged"), 1);
        assert!(cfg.events["FileChanged"][0].asynchronous);
    }

    #[test]
    fn parses_allowed_http_urls() {
        let body = r#"
allowed_http_hook_urls = ["https://hooks.example.com/*"]
http_hook_allowed_env_vars = ["AUDIT_TOKEN"]
"#;
        let cfg = HooksConfig::from_str(body, std::path::Path::new("h.toml")).unwrap();
        assert_eq!(cfg.allowed_http_hook_urls.len(), 1);
        assert_eq!(cfg.http_hook_allowed_env_vars, vec!["AUDIT_TOKEN"]);
    }

    #[test]
    #[allow(clippy::duration_suboptimal_units)]
    fn timeout_parsing_units() {
        assert_eq!(parse_timeout("5s").unwrap(), Duration::from_secs(5));
        assert_eq!(parse_timeout("200ms").unwrap(), Duration::from_millis(200));
        assert_eq!(parse_timeout("2m").unwrap(), Duration::from_secs(120));
        assert_eq!(parse_timeout("1h").unwrap(), Duration::from_secs(3600));
        assert_eq!(parse_timeout("30").unwrap(), Duration::from_secs(30));
        assert!(parse_timeout("bogus").is_err());
    }
}
