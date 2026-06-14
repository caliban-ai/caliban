//! Permission rules — Hook-layered tool gating with TOML rule files.
//!
//! See `docs/superpowers/specs/2026-05-23-permissions-design.md` and
//! `adrs/0020-permission-rules.md`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::hooks::{HookDecision, Hooks, PermCtx, ToolCtx};

// ---------------------------------------------------------------------------
// Rule + Action
// ---------------------------------------------------------------------------

/// The outcome of matching a rule against a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
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
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Rule {
    /// Pattern of the form `Tool` or `Tool:first-arg-glob`.
    pub tool: String,
    /// Action to take when the pattern matches.
    pub action: Action,
    /// Optional comment displayed in the Ask modal + audit log; never seen by the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Deny-only; surfaces to the model in place of the generic
    /// "permission denied" message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Reserved for v3 time-bounded rules; v2 parses but ignores at evaluation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
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

    /// Snapshot the current rules in insertion order. Used by the
    /// `/permissions` overlay to render the live runtime-rule list.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    #[must_use]
    pub fn snapshot(&self) -> Vec<RuntimeRule> {
        self.rules
            .lock()
            .expect("RuntimeRuleStore mutex poisoned")
            .clone()
    }

    /// Remove and return the rule at `index` (in insertion order, the
    /// same order [`Self::snapshot`] returns). Returns `None` for an
    /// out-of-bounds index; the store is left unchanged in that case.
    /// Used by the `/permissions` overlay's `d` keybind to delete a
    /// selected runtime rule.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    pub fn remove(&self, index: usize) -> Option<RuntimeRule> {
        let mut rules = self.rules.lock().expect("RuntimeRuleStore mutex poisoned");
        if index < rules.len() {
            Some(rules.remove(index))
        } else {
            None
        }
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
                reason: None,
                expires_at: None,
            };
            if rule_matches(&synth, ctx) {
                return Some(r.action);
            }
        }
        None
    }
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
// Pattern matching — delegated to permissions_matcher
// ---------------------------------------------------------------------------

fn rule_matches(rule: &Rule, ctx: &ToolCtx<'_>) -> bool {
    crate::permissions_matcher::matches(&rule.tool, ctx)
}

/// Free function used by the CLI (`caliban perms test/explain`) and the
/// `/permissions` test pane — runs the matcher against a borrowed rule
/// list and returns the first match.
#[must_use]
pub fn evaluate_rules<'a>(rules: &'a [Rule], ctx: &ToolCtx<'_>) -> Option<&'a Rule> {
    rules.iter().find(|r| rule_matches(r, ctx))
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
        reason: None,
        expires_at: None,
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
            HookDecision::Deny(non_interactive_deny_message(ctx.tool_name))
        }
    }
}

/// Build the deny message a [`NonInteractiveAskHandler`] returns when an
/// `Ask` rule fires without a TTY. The message names a concrete CLI
/// remediation tailored to the tool class so operators don't have to
/// guess between `--auto-allow`, `--permission-mode acceptEdits`, and a
/// targeted `--allow` rule.
fn non_interactive_deny_message(tool_name: &str) -> String {
    let head = format!("permission denied: '{tool_name}' requires interactive approval (no TTY)");
    let hint = if crate::permission_mode::is_file_edit_tool(tool_name) {
        "re-run with `--permission-mode acceptEdits` to auto-allow file edits, \
         or `--allow '<Tool>(<glob>)'` for a narrower rule"
    } else if tool_name == "Bash" {
        "re-run with `--allow 'Bash(<glob>)'` to allow specific commands, \
         or `--auto-allow` to allow all Ask-rule tools (dangerous)"
    } else {
        "re-run with `--allow '<Tool>'` to allow this tool, \
         or `--auto-allow` to allow all Ask-rule tools (dangerous)"
    };
    format!("{head}; {hint}")
}

/// Pluggable [`Hooks`] impl that gates tool dispatch via the rule set + an
/// [`AskHandler`]. Wraps an inner `Hooks` so existing hook implementations
/// (e.g. logging, audit) compose naturally.
pub struct PermissionsHook {
    rules: Vec<Rule>,
    /// Session-scoped runtime overlay, consulted *before* `rules` so an
    /// "Always allow/deny" added in the TUI modal takes effect on the next
    /// tool call without rebuilding the hook or restarting (the modal and
    /// this gate share the same `Arc`). Empty by default — wired via
    /// [`Self::with_runtime_rules`].
    runtime: Arc<RuntimeRuleStore>,
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
        Self {
            rules,
            runtime: Arc::new(RuntimeRuleStore::new()),
            ask,
            inner,
        }
    }

    /// Wire a shared [`RuntimeRuleStore`] consulted before the static rule
    /// set. The TUI passes the same `Arc` it appends to from the Ask
    /// modal's "Always allow/deny" branches, so a freshly-added rule gates
    /// the very next tool call (fixes the "rule saved but still re-prompts"
    /// bug for both session and file scopes).
    #[must_use]
    pub fn with_runtime_rules(mut self, runtime: Arc<RuntimeRuleStore>) -> Self {
        self.runtime = runtime;
        self
    }

    /// Find the first matching rule and return its action + comment. `*`
    /// catch-all in `default_rules()` guarantees a match.
    #[must_use]
    pub fn evaluate(&self, ctx: &ToolCtx<'_>) -> Action {
        self.evaluate_with_rule(ctx).0
    }

    /// Like [`Self::evaluate`] but also returns the matched rule's comment
    /// and reason (when any). Used to populate [`PermCtx::rule_comment`] for
    /// downstream hooks and to surface the deny reason to the model.
    #[must_use]
    pub fn evaluate_with_rule(
        &self,
        ctx: &ToolCtx<'_>,
    ) -> (Action, Option<String>, Option<String>) {
        // Session runtime overlay wins over the static rule set so an
        // "Always allow/deny" picked in the modal beats a config/default
        // rule on the next call. Runtime rules carry no comment/reason.
        if let Some(action) = self.runtime.evaluate(ctx) {
            return (action, None, None);
        }
        for r in &self.rules {
            if rule_matches(r, ctx) {
                return (r.action, r.comment.clone(), r.reason.clone());
            }
        }
        // Should be unreachable thanks to the `*` catch-all default; if it
        // somehow happens, deny rather than implicit allow.
        (Action::Deny, None, None)
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
        let (action, comment, reason) = self.evaluate_with_rule(ctx);
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
                let deny_msg = reason
                    .unwrap_or_else(|| format!("permission denied for tool '{}'", ctx.tool_name));
                Ok(HookDecision::Deny(deny_msg))
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

    async fn session_start(
        &self,
        ctx: &crate::hooks::SessionCtx<'_>,
    ) -> Result<crate::hooks::SessionStartOutcome> {
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
            reason: None,
            expires_at: None,
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
    fn runtime_rule_overrides_config_ask_live() {
        // Regression (#55): a rule added to the shared RuntimeRuleStore
        // *after* the hook is built must take effect on the very next
        // evaluation, beating a config/default `Ask`. This is the live
        // "Always allow" path — without it every "Always allow" (session
        // OR file scope) re-prompts until the process restarts.
        let store = Arc::new(RuntimeRuleStore::new());
        let h = hook(default_rules()).with_runtime_rules(Arc::clone(&store));
        let i = serde_json::json!({"command": "ls -F"});
        // Base rules: Bash → Ask.
        assert_eq!(h.evaluate(&ctx("Bash", &i)), Action::Ask);
        // Operator picks "Always allow" → rule lands in the shared store.
        store.add(RuntimeRule {
            pattern: "Bash:ls *".into(),
            action: Action::Allow,
        });
        // Next evaluation must see it live — no rebuild, no restart.
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
    async fn deny_action_surfaces_reason_to_model() {
        let mut rules = vec![Rule {
            tool: "Bash".into(),
            action: Action::Deny,
            comment: None,
            reason: Some("no shell, use Edit".into()),
            expires_at: None,
        }];
        rules.extend(default_rules());
        let h = hook(rules);
        let i = serde_json::json!({"command": "ls"});
        let d = h.before_tool(&ctx("Bash", &i)).await.unwrap();
        match d {
            HookDecision::Deny(msg) => assert!(
                msg.contains("no shell, use Edit"),
                "deny message must surface rule.reason — got: {msg}"
            ),
            other => panic!("expected Deny, got {other:?}"),
        }
    }

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

    /// File-edit tools should suggest `--permission-mode acceptEdits` because
    /// that is the existing one-flag remediation. `--auto-allow` is broader
    /// and shouldn't be the first thing we recommend for the common edit case.
    #[tokio::test]
    async fn non_interactive_deny_for_file_edit_suggests_accept_edits() {
        let h = hook(default_rules());
        let i = serde_json::json!({"file_path": "/tmp/x", "content": "y"});
        let d = h.before_tool(&ctx("Write", &i)).await.unwrap();
        let HookDecision::Deny(msg) = d else {
            panic!("expected Deny, got {d:?}");
        };
        assert!(msg.contains("--permission-mode acceptEdits"), "got: {msg}");
        assert!(msg.contains("'Write'"), "got: {msg}");
    }

    /// Bash should suggest a narrowly-scoped `--allow 'Bash(<glob>)'` rule,
    /// not `--permission-mode acceptEdits` (which doesn't cover Bash) and
    /// flag `--auto-allow` as dangerous.
    #[tokio::test]
    async fn non_interactive_deny_for_bash_suggests_targeted_allow_rule() {
        let h = hook(default_rules());
        let i = serde_json::json!({"command": "ls"});
        let d = h.before_tool(&ctx("Bash", &i)).await.unwrap();
        let HookDecision::Deny(msg) = d else {
            panic!("expected Deny, got {d:?}");
        };
        assert!(msg.contains("--allow 'Bash"), "got: {msg}");
        assert!(msg.contains("dangerous"), "got: {msg}");
        assert!(
            !msg.contains("acceptEdits"),
            "acceptEdits doesn't cover Bash; got: {msg}"
        );
    }

    /// Other tools (anything not file-edit and not Bash) should get a
    /// generic `--allow '<Tool>'` suggestion. `WebFetch` is a real built-in
    /// that defaults to Ask, so we use it here rather than a synthetic name.
    #[tokio::test]
    async fn non_interactive_deny_for_other_tool_suggests_generic_allow_rule() {
        let mut rules = vec![rule("WebFetch", Action::Ask)];
        rules.extend(default_rules());
        let h = hook(rules);
        let i = serde_json::json!({"url": "https://example.com"});
        let d = h.before_tool(&ctx("WebFetch", &i)).await.unwrap();
        let HookDecision::Deny(msg) = d else {
            panic!("expected Deny, got {d:?}");
        };
        assert!(msg.contains("--allow '<Tool>'"), "got: {msg}");
        assert!(
            !msg.contains("acceptEdits"),
            "acceptEdits doesn't cover WebFetch; got: {msg}"
        );
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
    fn rule_deserializes_reason_and_expires_at() {
        let src = r#"
[[rule]]
tool = "Bash"
action = "deny"
reason = "no shell access in CI"
expires_at = "2026-12-31T00:00:00Z"
"#;
        let parsed: RulesFile = toml::from_str(src).unwrap();
        assert_eq!(parsed.rules.len(), 1);
        let r = &parsed.rules[0];
        assert_eq!(r.action, Action::Deny);
        assert_eq!(r.reason.as_deref(), Some("no shell access in CI"));
        assert!(r.expires_at.is_some());
    }

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
    fn snapshot_returns_rules_in_insertion_order() {
        let store = RuntimeRuleStore::new();
        store.add(RuntimeRule {
            pattern: "Bash:ls *".into(),
            action: Action::Allow,
        });
        store.add(RuntimeRule {
            pattern: "Edit(/tmp/*)".into(),
            action: Action::Deny,
        });
        let snap = store.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].pattern, "Bash:ls *");
        assert_eq!(snap[0].action, Action::Allow);
        assert_eq!(snap[1].pattern, "Edit(/tmp/*)");
        assert_eq!(snap[1].action, Action::Deny);
    }

    #[test]
    fn snapshot_on_empty_store_returns_empty() {
        let store = RuntimeRuleStore::new();
        assert!(store.snapshot().is_empty());
    }

    #[test]
    fn remove_drops_rule_at_index_and_shifts_remainder() {
        let store = RuntimeRuleStore::new();
        store.add(RuntimeRule {
            pattern: "a".into(),
            action: Action::Allow,
        });
        store.add(RuntimeRule {
            pattern: "b".into(),
            action: Action::Allow,
        });
        store.add(RuntimeRule {
            pattern: "c".into(),
            action: Action::Allow,
        });
        let removed = store.remove(1);
        assert_eq!(removed.as_ref().map(|r| r.pattern.as_str()), Some("b"));
        let snap = store.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].pattern, "a");
        assert_eq!(snap[1].pattern, "c");
    }

    #[test]
    fn remove_out_of_bounds_returns_none_and_leaves_store_intact() {
        let store = RuntimeRuleStore::new();
        store.add(RuntimeRule {
            pattern: "x".into(),
            action: Action::Allow,
        });
        assert!(store.remove(5).is_none());
        assert!(store.remove(usize::MAX).is_none());
        assert_eq!(store.snapshot().len(), 1);
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

#[cfg(test)]
mod evaluate_rules_tests {
    use super::*;

    fn ctx<'a>(name: &'a str, input: &'a serde_json::Value) -> ToolCtx<'a> {
        ToolCtx {
            turn_index: 0,
            tool_use_id: "t",
            tool_name: name,
            input,
        }
    }

    /// Task 5.3: `evaluate_rules` returns the first matching rule.
    #[test]
    fn test_pane_outcome_reflects_matched_rule() {
        let rules = vec![Rule {
            tool: "Bash:rm *".into(),
            action: Action::Deny,
            comment: None,
            reason: None,
            expires_at: None,
        }];
        let input = serde_json::json!({"command": "rm -rf /"});
        let ctx = ctx("Bash", &input);
        let r = evaluate_rules(&rules, &ctx).unwrap();
        assert_eq!(r.tool, "Bash:rm *");
        assert_eq!(r.action, Action::Deny);
    }

    #[test]
    fn evaluate_rules_returns_none_when_no_match() {
        let rules = vec![Rule {
            tool: "Read".into(),
            action: Action::Allow,
            comment: None,
            reason: None,
            expires_at: None,
        }];
        let input = serde_json::json!({"command": "ls"});
        let ctx = ctx("Bash", &input);
        assert!(evaluate_rules(&rules, &ctx).is_none());
    }

    #[test]
    fn evaluate_rules_first_match_wins() {
        let rules = vec![
            Rule {
                tool: "Bash".into(),
                action: Action::Allow,
                comment: None,
                reason: None,
                expires_at: None,
            },
            Rule {
                tool: "Bash".into(),
                action: Action::Deny,
                comment: None,
                reason: None,
                expires_at: None,
            },
        ];
        let input = serde_json::json!({"command": "ls"});
        let ctx = ctx("Bash", &input);
        let r = evaluate_rules(&rules, &ctx).unwrap();
        assert_eq!(r.action, Action::Allow, "first rule should win");
    }
}
