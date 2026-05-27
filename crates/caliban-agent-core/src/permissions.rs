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

/// Runtime-only rule added during a session via the "Always allow/reject"
/// Ask modal branches. Composes with config rules under existing
/// precedence (runtime > project > user > managed); session-scoped only,
/// never persisted to disk.
#[derive(Debug, Clone)]
pub struct RuntimeRule {
    /// Pattern of the form `Tool` or `Tool:first-arg-glob`.
    pub pattern: String,
    /// Action to take when the pattern matches. Only `Allow` and `Deny`
    /// are meaningful here — `Ask` would loop back into the modal.
    pub action: Action,
}

/// Session-scoped runtime-rule store. Interior-mutable so existing
/// `Arc<dyn Hooks>` callers can add rules without re-building the hook
/// chain. Always consulted before config rules so an "Always allow"
/// added in the modal beats a project-level `Ask`.
#[derive(Debug, Default)]
pub struct RuntimeRuleStore {
    rules: std::sync::Mutex<Vec<RuntimeRule>>,
}

impl RuntimeRuleStore {
    /// Empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a runtime rule. Newer rules win when patterns overlap
    /// because [`Self::evaluate`] checks them in insertion order from
    /// the back of the list.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned (a previous panic
    /// while holding the lock). In practice this only happens if a
    /// modal-side caller panicked mid-add.
    pub fn add(&self, rule: RuntimeRule) {
        self.rules
            .lock()
            .expect("RuntimeRuleStore mutex poisoned")
            .push(rule);
    }

    /// Evaluate against a `ToolCtx`. Returns `Some(Action::Allow|Deny)`
    /// if a runtime rule matches; `None` otherwise. The caller falls
    /// back to the config rule set when this returns `None`.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    #[must_use]
    pub fn evaluate(&self, ctx: &ToolCtx<'_>) -> Option<Action> {
        let rules = self.rules.lock().expect("RuntimeRuleStore mutex poisoned");
        for r in rules.iter().rev() {
            let synth = Rule {
                tool: r.pattern.clone(),
                action: r.action,
                comment: None,
            };
            if rule_matches(&synth, ctx) {
                return Some(r.action);
            }
        }
        None
    }
}

/// Derive an "Always-allow / Always-reject" rule pattern from one
/// invocation. The plan calls for one canonical shape per tool kind so
/// the modal can show what the user is committing to before they
/// confirm. Pure function; unit-tested.
#[must_use]
pub fn derive_pattern(tool: &str, input: &serde_json::Value) -> String {
    if tool == "Bash" {
        let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
        let first = cmd.split_whitespace().next().unwrap_or("*");
        return format!("Bash({first} *)");
    }
    if matches!(tool, "Edit" | "Read" | "Write") {
        let path = input
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("/*");
        let dir = std::path::Path::new(path)
            .parent()
            .map_or_else(|| "/".into(), |p| p.display().to_string());
        return format!("{tool}({dir}/*)");
    }
    if tool.starts_with("mcp__") {
        return format!("{tool}(*)");
    }
    format!("{tool}(*)")
}

#[derive(Debug, Deserialize)]
struct RulesFile {
    #[serde(default, rename = "rule")]
    rules: Vec<Rule>,
}

// ---------------------------------------------------------------------------
// Glob matcher (`*`, `?`) — implementation moved to caliban-common
// ---------------------------------------------------------------------------

/// Match `pattern` against `value`. Supports `*` (zero or more chars) and `?`
/// (exactly one char). Re-exported from
/// [`caliban_common::glob_match::matches_glob`] — kept here for back-compat;
/// new consumers should depend on `caliban-common` directly.
#[deprecated(
    since = "0.0.0",
    note = "use `caliban_common::glob_match::matches_glob` instead"
)]
pub use caliban_common::glob_match::matches_glob;

/// Extract the "first arg" string for a tool input, per the permissions
/// design spec. Re-exported from
/// [`caliban_common::glob_match::first_arg`].
#[deprecated(
    since = "0.0.0",
    note = "use `caliban_common::glob_match::first_arg` instead"
)]
pub use caliban_common::glob_match::first_arg;

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

fn rule_matches(rule: &Rule, ctx: &ToolCtx<'_>) -> bool {
    use caliban_common::glob_match::{first_arg as common_first_arg, matches_glob as common_glob};
    let (tool_pat, arg_pat) = split_pattern(&rule.tool);
    if tool_pat != "*" && !common_glob(tool_pat, ctx.tool_name) {
        return false;
    }
    match arg_pat {
        None => true,
        Some(glob) => common_first_arg(ctx.tool_name, ctx.input)
            .as_deref()
            .is_some_and(|arg| common_glob(glob, arg)),
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
#[deprecated(
    since = "0.0.1",
    note = "load via caliban-settings; legacy loaders remove in v0.2"
)]
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
#[deprecated(
    since = "0.0.1",
    note = "load via caliban-settings; legacy loaders remove in v0.2"
)]
pub fn load_rules(
    cli_rules: Vec<Rule>,
    workspace_root: &Path,
) -> std::result::Result<Vec<Rule>, PermissionsLoadError> {
    let mut all = cli_rules;

    let project_file = workspace_root.join(".caliban/permissions.toml");
    #[allow(deprecated)]
    all.extend(load_rules_file(&project_file)?);

    let user_dir = dirs::config_dir().map(|d| d.join("caliban/permissions.toml"));
    if let Some(p) = user_dir {
        #[allow(deprecated)]
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
    ) -> Result<crate::hooks::TurnDecision> {
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
    //
    // The glob/first-arg unit tests now live in `caliban-common`. Coverage
    // here is via the higher-level `rule_matches` paths exercised by the
    // rule-evaluation tests below.

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
    #[allow(deprecated)]
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
    #[allow(deprecated)]
    fn loader_missing_file_returns_empty() {
        let path = std::path::Path::new("/nonexistent/path/permissions.toml");
        let rules = load_rules_file(path).unwrap();
        assert!(rules.is_empty());
    }

    #[test]
    #[allow(deprecated)]
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

#[cfg(test)]
mod derive_pattern_tests {
    use super::*;

    #[test]
    fn bash_pattern_derives_first_token() {
        let p = derive_pattern("Bash", &serde_json::json!({"command": "gh pr view 42"}));
        assert_eq!(p, "Bash(gh *)");
    }

    #[test]
    fn read_pattern_uses_parent_dir() {
        let p = derive_pattern(
            "Read",
            &serde_json::json!({"file_path": "/home/me/proj/src/foo.rs"}),
        );
        assert!(p.starts_with("Read(/home/me/proj/src/"));
        assert!(p.ends_with("/*)"));
    }

    #[test]
    fn mcp_pattern_glob_all_args() {
        let p = derive_pattern("mcp__server__tool", &serde_json::json!({}));
        assert_eq!(p, "mcp__server__tool(*)");
    }

    #[test]
    fn other_pattern_glob_all_args() {
        let p = derive_pattern("WebFetch", &serde_json::json!({"url": "https://x"}));
        assert_eq!(p, "WebFetch(*)");
    }
}

#[cfg(test)]
mod runtime_rule_tests {
    use super::*;

    fn ctx<'a>(name: &'a str, input: &'a serde_json::Value) -> ToolCtx<'a> {
        ToolCtx {
            turn_index: 0,
            tool_use_id: "t",
            tool_name: name,
            input,
        }
    }

    #[test]
    fn empty_store_returns_none() {
        let store = RuntimeRuleStore::new();
        let input = serde_json::json!({"command": "ls"});
        assert!(store.evaluate(&ctx("Bash", &input)).is_none());
    }

    #[test]
    fn always_allow_matches_subsequent_invocation() {
        let store = RuntimeRuleStore::new();
        store.add(RuntimeRule {
            pattern: "Bash:ls *".into(),
            action: Action::Allow,
        });
        let input = serde_json::json!({"command": "ls -al"});
        let outcome = store.evaluate(&ctx("Bash", &input));
        assert_eq!(outcome, Some(Action::Allow));
    }

    #[test]
    fn always_reject_overrides_a_later_allow() {
        let store = RuntimeRuleStore::new();
        // First add an allow, then a deny — most recent wins.
        store.add(RuntimeRule {
            pattern: "Bash:rm *".into(),
            action: Action::Allow,
        });
        store.add(RuntimeRule {
            pattern: "Bash:rm *".into(),
            action: Action::Deny,
        });
        let input = serde_json::json!({"command": "rm -rf /tmp"});
        let outcome = store.evaluate(&ctx("Bash", &input));
        assert_eq!(outcome, Some(Action::Deny));
    }
}
