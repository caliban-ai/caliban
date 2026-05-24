//! Permission rules — Hook-layered tool gating with TOML rule files.
//!
//! See `docs/superpowers/specs/2026-05-23-permissions-design.md` and
//! `adrs/0020-permission-rules.md`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use crate::error::Result;
use crate::hooks::{HookDecision, Hooks, PermCtx, ToolCtx};

// ---------------------------------------------------------------------------
// Rule + Action
// ---------------------------------------------------------------------------

/// The outcome of matching a rule against a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// Run the tool without prompting.
    Allow,
    /// Reject the tool call.
    Deny,
    /// Defer to an interactive prompt via the [`AskHandler`].
    Ask,
}

/// One rule from a TOML file or CLI flag.
#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    /// Pattern of the form `Tool` or `Tool:first-arg-glob`.
    pub tool: String,
    /// Action to take when the pattern matches.
    pub action: Action,
    /// Optional comment displayed in the Ask modal.
    #[serde(default)]
    pub comment: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RulesFile {
    #[serde(default, rename = "rule")]
    rules: Vec<Rule>,
}

// ---------------------------------------------------------------------------
// Glob matcher (`*`, `?`)
// ---------------------------------------------------------------------------

/// Match `pattern` against `value`. Supports `*` (zero or more chars) and `?`
/// (exactly one char). Intentionally narrow; if requirements grow, switch to
/// the `globset` crate.
#[must_use]
pub fn matches_glob(pattern: &str, value: &str) -> bool {
    let pattern_bytes = pattern.as_bytes();
    let value_bytes = value.as_bytes();
    let mut p = 0_usize;
    let mut v = 0_usize;
    let mut star: Option<usize> = None;
    let mut star_v: usize = 0;

    while v < value_bytes.len() {
        if p < pattern_bytes.len()
            && (pattern_bytes[p] == b'?' || pattern_bytes[p] == value_bytes[v])
        {
            p += 1;
            v += 1;
        } else if p < pattern_bytes.len() && pattern_bytes[p] == b'*' {
            star = Some(p);
            star_v = v;
            p += 1;
        } else if let Some(s) = star {
            p = s + 1;
            star_v += 1;
            v = star_v;
        } else {
            return false;
        }
    }
    while p < pattern_bytes.len() && pattern_bytes[p] == b'*' {
        p += 1;
    }
    p == pattern_bytes.len()
}

// ---------------------------------------------------------------------------
// Pattern parsing
// ---------------------------------------------------------------------------

/// `(tool_pattern, first_arg_pattern)` — the second is `None` when the rule
/// pattern has no `:`.
fn split_pattern(pattern: &str) -> (&str, Option<&str>) {
    pattern
        .split_once(':')
        .map_or((pattern, None), |(name, glob)| (name, Some(glob)))
}

/// Extract the "first arg" string for a tool input, per the design spec.
/// Returns `None` when the tool has no first-arg accessor or the JSON shape
/// doesn't match.
#[must_use]
pub fn first_arg(tool_name: &str, input: &serde_json::Value) -> Option<String> {
    let key = match tool_name {
        "Bash" => "command",
        "WebFetch" => "url",
        "Read" | "Write" | "Edit" => "path",
        _ => return None,
    };
    input.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

fn rule_matches(rule: &Rule, ctx: &ToolCtx<'_>) -> bool {
    let (tool_pat, arg_pat) = split_pattern(&rule.tool);
    if tool_pat != "*" && !matches_glob(tool_pat, ctx.tool_name) {
        return false;
    }
    match arg_pat {
        None => true,
        Some(glob) => first_arg(ctx.tool_name, ctx.input)
            .as_deref()
            .is_some_and(|arg| matches_glob(glob, arg)),
    }
}

// ---------------------------------------------------------------------------
// Built-in defaults
// ---------------------------------------------------------------------------

/// Built-in default rules applied at the lowest priority. Read-only tools
/// Allow; mutating tools Ask; catch-all is Ask.
#[must_use]
pub fn default_rules() -> Vec<Rule> {
    [
        ("Read", Action::Allow),
        ("Grep", Action::Allow),
        ("Glob", Action::Allow),
        ("WebFetch", Action::Ask),
        ("Bash", Action::Ask),
        ("Write", Action::Ask),
        ("Edit", Action::Ask),
        ("TodoWrite", Action::Allow),
        ("EnterPlanMode", Action::Allow),
        ("ExitPlanMode", Action::Allow),
        ("*", Action::Ask),
    ]
    .into_iter()
    .map(|(t, a)| Rule {
        tool: t.into(),
        action: a,
        comment: None,
    })
    .collect()
}

// ---------------------------------------------------------------------------
// Loaders
// ---------------------------------------------------------------------------

/// Errors emitted by the permissions loader.
#[derive(thiserror::Error, Debug)]
pub enum PermissionsLoadError {
    /// IO failure reading a permissions file.
    #[error("permissions: io error reading {path}: {source}")]
    Io {
        /// Path that failed to read.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// TOML parse error.
    #[error("permissions: parse error in {path}: {source}")]
    Parse {
        /// Path that failed to parse.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: toml::de::Error,
    },
}

/// Load rules from a TOML file. Missing file → `Ok(vec![])`.
///
/// # Errors
/// Returns [`PermissionsLoadError::Io`] on read errors other than `NotFound`,
/// and [`PermissionsLoadError::Parse`] on malformed TOML.
pub fn load_rules_file(path: &Path) -> std::result::Result<Vec<Rule>, PermissionsLoadError> {
    let body = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(PermissionsLoadError::Io {
                path: path.to_path_buf(),
                source: e,
            });
        }
    };
    let parsed: RulesFile =
        toml::from_str(&body).map_err(|source| PermissionsLoadError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(parsed.rules)
}

/// Resolve and load rules from the standard locations:
/// 1. CLI rules (highest priority — caller-supplied).
/// 2. Project file `<workspace>/.caliban/permissions.toml`.
/// 3. User file `$XDG_CONFIG_HOME/caliban/permissions.toml`.
/// 4. Built-in defaults.
///
/// Rules from higher-priority sources are placed first; first-match-wins
/// at evaluation time.
///
/// # Errors
/// Propagates [`PermissionsLoadError`] from the project or user file readers.
pub fn load_rules(
    cli_rules: Vec<Rule>,
    workspace_root: &Path,
) -> std::result::Result<Vec<Rule>, PermissionsLoadError> {
    let mut all = cli_rules;

    let project_file = workspace_root.join(".caliban/permissions.toml");
    all.extend(load_rules_file(&project_file)?);

    let user_dir = dirs::config_dir().map(|d| d.join("caliban/permissions.toml"));
    if let Some(p) = user_dir {
        all.extend(load_rules_file(&p)?);
    }

    all.extend(default_rules());
    Ok(all)
}

// ---------------------------------------------------------------------------
// AskHandler + PermissionsHook
// ---------------------------------------------------------------------------

/// Pluggable backend for `Ask` rules. TUI provides an interactive modal; CLI
/// falls back to the [`NonInteractiveAskHandler`] which converts `Ask` to
/// either `Allow` (when `--auto-allow`) or `Deny`.
#[async_trait]
pub trait AskHandler: Send + Sync {
    /// Decide what to do with a tool call whose matched rule is `Ask`.
    async fn prompt(&self, ctx: &ToolCtx<'_>) -> HookDecision;
}

/// `AskHandler` used in non-interactive mode. Default is `Deny` (the safer
/// fallback when no human is in the loop). When `auto_allow` is `true`,
/// every `Ask` becomes `Allow` — documented loudly in CLI help as
/// `--auto-allow`.
#[derive(Debug)]
pub struct NonInteractiveAskHandler {
    /// When `true`, treat `Ask` decisions as `Allow`.
    pub auto_allow: bool,
}

#[async_trait]
impl AskHandler for NonInteractiveAskHandler {
    async fn prompt(&self, ctx: &ToolCtx<'_>) -> HookDecision {
        if self.auto_allow {
            HookDecision::Allow
        } else {
            HookDecision::Deny(format!(
                "permission denied: '{}' requires interactive approval (no TTY)",
                ctx.tool_name
            ))
        }
    }
}

/// Pluggable [`Hooks`] impl that gates tool dispatch via the rule set + an
/// [`AskHandler`]. Wraps an inner `Hooks` so existing hook implementations
/// (e.g. logging, audit) compose naturally.
pub struct PermissionsHook {
    rules: Vec<Rule>,
    ask: Arc<dyn AskHandler>,
    inner: Arc<dyn Hooks>,
}

impl std::fmt::Debug for PermissionsHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PermissionsHook")
            .field("rules", &self.rules.len())
            .finish_non_exhaustive()
    }
}

impl PermissionsHook {
    /// Build the hook with a rule set, an `AskHandler`, and an inner `Hooks`
    /// the gate composes with (e.g. [`crate::hooks::NoopHooks`]).
    #[must_use]
    pub fn new(rules: Vec<Rule>, ask: Arc<dyn AskHandler>, inner: Arc<dyn Hooks>) -> Self {
        Self { rules, ask, inner }
    }

    /// Find the first matching rule and return its action + comment. `*`
    /// catch-all in `default_rules()` guarantees a match.
    #[must_use]
    pub fn evaluate(&self, ctx: &ToolCtx<'_>) -> Action {
        self.evaluate_with_rule(ctx).0
    }

    /// Like [`Self::evaluate`] but also returns the matched rule's comment
    /// (when any). Used to populate [`PermCtx::rule_comment`] for downstream
    /// hooks.
    #[must_use]
    pub fn evaluate_with_rule(&self, ctx: &ToolCtx<'_>) -> (Action, Option<String>) {
        for r in &self.rules {
            if rule_matches(r, ctx) {
                return (r.action, r.comment.clone());
            }
        }
        // Should be unreachable thanks to the `*` catch-all default; if it
        // somehow happens, deny rather than implicit allow.
        (Action::Deny, None)
    }
}

fn action_str(a: Action) -> &'static str {
    match a {
        Action::Allow => "allow",
        Action::Deny => "deny",
        Action::Ask => "ask",
    }
}

#[async_trait]
impl Hooks for PermissionsHook {
    async fn before_tool(&self, ctx: &ToolCtx<'_>) -> Result<HookDecision> {
        let (action, comment) = self.evaluate_with_rule(ctx);
        match action {
            Action::Allow => self.inner.before_tool(ctx).await,
            Action::Deny => {
                let perm_ctx = PermCtx {
                    turn_index: ctx.turn_index,
                    tool_use_id: ctx.tool_use_id,
                    tool_name: ctx.tool_name,
                    input: ctx.input,
                    rule_action: action_str(Action::Deny),
                    rule_comment: comment.as_deref(),
                };
                // Fire the denied-by-rule observer hook (non-fatal on error).
                if let Err(e) = self.inner.permission_denied(&perm_ctx).await {
                    tracing::warn!(error = %e, "permission_denied hook error (non-fatal)");
                }
                Ok(HookDecision::Deny(format!(
                    "permission denied for tool '{}'",
                    ctx.tool_name
                )))
            }
            Action::Ask => {
                let perm_ctx = PermCtx {
                    turn_index: ctx.turn_index,
                    tool_use_id: ctx.tool_use_id,
                    tool_name: ctx.tool_name,
                    input: ctx.input,
                    rule_action: action_str(Action::Ask),
                    rule_comment: comment.as_deref(),
                };
                if let Err(e) = self.inner.permission_request(&perm_ctx).await {
                    tracing::warn!(error = %e, "permission_request hook error (non-fatal)");
                }
                let decision = self.ask.prompt(ctx).await;
                if matches!(decision, HookDecision::Deny(_))
                    && let Err(e) = self.inner.permission_denied(&perm_ctx).await
                {
                    tracing::warn!(error = %e, "permission_denied hook error (non-fatal)");
                }
                Ok(decision)
            }
        }
    }

    async fn after_tool(
        &self,
        ctx: &ToolCtx<'_>,
        result: &std::result::Result<Vec<caliban_provider::ContentBlock>, crate::tool::ToolError>,
    ) -> Result<()> {
        self.inner.after_tool(ctx, result).await
    }

    async fn before_turn(&self, ctx: &crate::hooks::TurnCtx<'_>) -> Result<()> {
        self.inner.before_turn(ctx).await
    }

    async fn after_turn(
        &self,
        ctx: &crate::hooks::TurnCtx<'_>,
        outcome: &crate::TurnOutcome,
    ) -> Result<()> {
        self.inner.after_turn(ctx, outcome).await
    }

    async fn session_start(&self, ctx: &crate::hooks::SessionCtx<'_>) -> Result<()> {
        self.inner.session_start(ctx).await
    }

    async fn session_end(
        &self,
        ctx: &crate::hooks::SessionCtx<'_>,
        outcome: &crate::hooks::SessionOutcome,
    ) -> Result<()> {
        self.inner.session_end(ctx, outcome).await
    }

    async fn user_prompt_submit(&self, ctx: &crate::hooks::PromptCtx<'_>) -> Result<HookDecision> {
        self.inner.user_prompt_submit(ctx).await
    }

    async fn pre_compact(&self, ctx: &crate::hooks::CompactCtx<'_>) -> Result<()> {
        self.inner.pre_compact(ctx).await
    }

    async fn post_compact(
        &self,
        ctx: &crate::hooks::CompactCtx<'_>,
        outcome: &crate::hooks::CompactOutcome,
    ) -> Result<()> {
        self.inner.post_compact(ctx, outcome).await
    }

    async fn config_change(&self, ctx: &crate::hooks::ConfigChangeCtx<'_>) -> Result<()> {
        self.inner.config_change(ctx).await
    }

    async fn cwd_changed(&self, ctx: &crate::hooks::CwdChangedCtx<'_>) -> Result<()> {
        self.inner.cwd_changed(ctx).await
    }

    async fn file_changed(&self, ctx: &crate::hooks::FileChangedCtx<'_>) -> Result<()> {
        self.inner.file_changed(ctx).await
    }

    async fn permission_request(&self, ctx: &PermCtx<'_>) -> Result<()> {
        self.inner.permission_request(ctx).await
    }

    async fn permission_denied(&self, ctx: &PermCtx<'_>) -> Result<()> {
        self.inner.permission_denied(ctx).await
    }

    async fn notification(&self, ctx: &crate::hooks::NotificationCtx<'_>) -> Result<()> {
        self.inner.notification(ctx).await
    }

    async fn subagent_start(&self, ctx: &crate::hooks::SubagentCtx<'_>) -> Result<()> {
        self.inner.subagent_start(ctx).await
    }

    async fn subagent_stop(
        &self,
        ctx: &crate::hooks::SubagentCtx<'_>,
        outcome: &crate::hooks::SubagentOutcome,
    ) -> Result<()> {
        self.inner.subagent_stop(ctx, outcome).await
    }

    async fn task_created(&self, ctx: &crate::hooks::TaskCtx<'_>) -> Result<()> {
        self.inner.task_created(ctx).await
    }

    async fn task_completed(
        &self,
        ctx: &crate::hooks::TaskCtx<'_>,
        outcome: &crate::hooks::TaskOutcome,
    ) -> Result<()> {
        self.inner.task_completed(ctx, outcome).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NoopHooks;

    fn ctx<'a>(name: &'a str, input: &'a serde_json::Value) -> ToolCtx<'a> {
        ToolCtx {
            turn_index: 0,
            tool_use_id: "t1",
            tool_name: name,
            input,
        }
    }

    fn rule(tool: &str, action: Action) -> Rule {
        Rule {
            tool: tool.into(),
            action,
            comment: None,
        }
    }

    fn hook(rules: Vec<Rule>) -> PermissionsHook {
        PermissionsHook::new(
            rules,
            Arc::new(NonInteractiveAskHandler { auto_allow: false }),
            Arc::new(NoopHooks),
        )
    }

    // --- matcher tests ---

    #[test]
    fn glob_star_matches_anything() {
        assert!(matches_glob("*", ""));
        assert!(matches_glob("*", "anything"));
    }

    #[test]
    fn glob_q_matches_one_char() {
        assert!(matches_glob("a?c", "abc"));
        assert!(!matches_glob("a?c", "abbc"));
    }

    #[test]
    fn glob_no_special_chars_is_literal() {
        assert!(matches_glob("hello", "hello"));
        assert!(!matches_glob("hello", "hella"));
    }

    #[test]
    fn glob_star_prefix() {
        assert!(matches_glob("git *", "git status"));
        assert!(!matches_glob("git *", "gitk"));
    }

    #[test]
    fn glob_rm_prefix_does_not_match_sudo_rm() {
        assert!(!matches_glob("rm *", "sudo rm -rf /"));
    }

    // --- defaults ---

    #[test]
    fn default_read_allowed_bash_ask() {
        let h = hook(default_rules());
        let i = serde_json::json!({});
        assert_eq!(h.evaluate(&ctx("Read", &i)), Action::Allow);
        assert_eq!(h.evaluate(&ctx("Bash", &i)), Action::Ask);
        assert_eq!(h.evaluate(&ctx("WebFetch", &i)), Action::Ask);
    }

    // --- first-match-wins ---

    #[test]
    fn first_match_wins_within_a_source() {
        let mut rules = vec![rule("Bash:git *", Action::Allow)];
        rules.extend(default_rules());
        let h = hook(rules);
        let i = serde_json::json!({"command": "git push"});
        assert_eq!(h.evaluate(&ctx("Bash", &i)), Action::Allow);
        let i2 = serde_json::json!({"command": "rm -rf /"});
        // Falls through to default Bash → Ask.
        assert_eq!(h.evaluate(&ctx("Bash", &i2)), Action::Ask);
    }

    #[test]
    fn cli_priority_overrides_default() {
        let mut rules = vec![rule("Bash", Action::Allow)];
        rules.extend(default_rules());
        let h = hook(rules);
        let i = serde_json::json!({"command": "anything"});
        assert_eq!(h.evaluate(&ctx("Bash", &i)), Action::Allow);
    }

    #[test]
    fn catchall_star_matches_unknown_tool() {
        let h = hook(default_rules());
        let i = serde_json::json!({});
        assert_eq!(h.evaluate(&ctx("UnknownMcpTool", &i)), Action::Ask);
    }

    #[test]
    fn first_arg_only_matches_when_accessor_known() {
        let mut rules = vec![rule("UnknownMcpTool:foo", Action::Allow)];
        rules.extend(default_rules());
        let h = hook(rules);
        let i = serde_json::json!({"command": "foo"});
        // UnknownMcpTool has no first-arg accessor → arg-pattern can't match.
        assert_eq!(h.evaluate(&ctx("UnknownMcpTool", &i)), Action::Ask);
    }

    // --- async hook ---

    #[tokio::test]
    async fn deny_action_returns_deny_decision() {
        let mut rules = vec![rule("Bash", Action::Deny)];
        rules.extend(default_rules());
        let h = hook(rules);
        let i = serde_json::json!({"command": "x"});
        let d = h.before_tool(&ctx("Bash", &i)).await.unwrap();
        assert!(matches!(d, HookDecision::Deny(_)));
    }

    #[tokio::test]
    async fn ask_without_auto_allow_denies() {
        let h = hook(default_rules());
        let i = serde_json::json!({"command": "x"});
        let d = h.before_tool(&ctx("Bash", &i)).await.unwrap();
        assert!(matches!(d, HookDecision::Deny(_)));
    }

    #[tokio::test]
    async fn ask_with_auto_allow_allows() {
        let h = PermissionsHook::new(
            default_rules(),
            Arc::new(NonInteractiveAskHandler { auto_allow: true }),
            Arc::new(NoopHooks),
        );
        let i = serde_json::json!({"command": "x"});
        let d = h.before_tool(&ctx("Bash", &i)).await.unwrap();
        assert!(matches!(d, HookDecision::Allow));
    }

    // --- TOML loader ---

    #[test]
    fn loader_parses_valid_toml() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("permissions.toml");
        std::fs::write(
            &f,
            r#"
[[rule]]
tool = "Bash"
action = "allow"

[[rule]]
tool = "Bash:rm *"
action = "deny"
comment = "no rm"
"#,
        )
        .unwrap();
        let rules = load_rules_file(&f).unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].action, Action::Allow);
        assert_eq!(rules[1].action, Action::Deny);
        assert_eq!(rules[1].comment.as_deref(), Some("no rm"));
    }

    #[test]
    fn loader_missing_file_returns_empty() {
        let path = std::path::Path::new("/nonexistent/path/permissions.toml");
        let rules = load_rules_file(path).unwrap();
        assert!(rules.is_empty());
    }

    #[test]
    fn loader_invalid_action_errors() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("permissions.toml");
        std::fs::write(
            &f,
            r#"
[[rule]]
tool = "Bash"
action = "bogus"
"#,
        )
        .unwrap();
        let err = load_rules_file(&f).unwrap_err();
        assert!(matches!(err, PermissionsLoadError::Parse { .. }));
    }
}
