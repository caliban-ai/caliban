//! `ModeFilter` — `Hooks` impl that composes a [`PermissionMode`] with the
//! existing [`crate::PermissionsHook`] (ADR 0029).
//!
//! See `docs/superpowers/specs/2026-05-24-permission-modes-design.md`.
//!
//! Composition order:
//!
//! - `bypassPermissions` short-circuits the entire stack to `Allow`
//!   (requires `--allow-dangerously-skip-permissions`).
//! - `acceptEdits` auto-allows `Write`/`Edit`/`MultiEdit`/`NotebookEdit`
//!   without consulting rules.
//! - `plan` denies tools outside the plan-mode allowlist.
//! - `auto` calls the [`AutoModeClassifier`]; `Allow`/`HardDeny` win
//!   immediately, `SoftDeny` falls through to the inner Ask handler.
//! - `dontAsk` rewrites the inner verdict from `Ask` → `Allow`.
//! - `default` is a pass-through.

use std::sync::Arc;

use async_trait::async_trait;

use crate::auto_mode::{AutoModeClassifier, AutoVerdict};
use crate::error::Result;
use crate::hooks::{HookDecision, Hooks, PermCtx, ToolCtx};
use crate::permission_mode::{PermissionMode, SharedPermissionMode, is_file_edit_tool};
use crate::plan_mode::is_allowed_in_plan_mode;

/// `Hooks` impl that wraps an inner `Hooks` (typically [`crate::PermissionsHook`])
/// and applies [`PermissionMode`] semantics on every `before_tool`.
///
/// Delegates every other hook event straight through to the inner.
pub struct ModeFilter {
    mode: SharedPermissionMode,
    classifier: Option<Arc<AutoModeClassifier>>,
    inner: Arc<dyn Hooks>,
    bypass_latch: bool,
}

impl std::fmt::Debug for ModeFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModeFilter")
            .field("mode", &self.mode.load())
            .field("bypass_latch", &self.bypass_latch)
            .field("has_classifier", &self.classifier.is_some())
            .finish_non_exhaustive()
    }
}

impl ModeFilter {
    /// Build a filter.
    ///
    /// - `mode` — shared handle to the active mode (Shift+Tab writes to it).
    /// - `inner` — the wrapped hook (rules + `AskHandler`).
    /// - `classifier` — required for `auto`; `None` makes auto soft-deny
    ///   everything via [`crate::auto_mode::DecisionSource::DisabledFallback`]
    ///   semantics.
    /// - `bypass_latch` — gate for the `bypassPermissions` mode. Without it,
    ///   that mode degrades to `default` and emits a warning.
    #[must_use]
    pub fn new(
        mode: SharedPermissionMode,
        inner: Arc<dyn Hooks>,
        classifier: Option<Arc<AutoModeClassifier>>,
        bypass_latch: bool,
    ) -> Self {
        Self {
            mode,
            classifier,
            inner,
            bypass_latch,
        }
    }

    /// Read the active mode.
    #[must_use]
    pub fn mode(&self) -> PermissionMode {
        self.mode.load()
    }

    /// Whether the bypass-permissions latch is set.
    #[must_use]
    pub fn bypass_latch(&self) -> bool {
        self.bypass_latch
    }
}

#[async_trait]
impl Hooks for ModeFilter {
    async fn before_tool(&self, ctx: &ToolCtx<'_>) -> Result<HookDecision> {
        match self.mode.load() {
            PermissionMode::BypassPermissions => {
                if self.bypass_latch {
                    tracing::warn!(
                        target: caliban_common::tracing_targets::TARGET_PERMISSIONS,
                        tool = ctx.tool_name,
                        "bypassPermissions: allowing without consulting rules",
                    );
                    return Ok(HookDecision::Allow);
                }
                // No latch — degrade to default behavior, log loudly.
                tracing::warn!(
                    target: caliban_common::tracing_targets::TARGET_PERMISSIONS,
                    "bypassPermissions active without --allow-dangerously-skip-permissions; \
                     degrading to default-mode semantics",
                );
                self.inner.before_tool(ctx).await
            }
            PermissionMode::AcceptEdits => {
                if is_file_edit_tool(ctx.tool_name) {
                    return Ok(HookDecision::Allow);
                }
                self.inner.before_tool(ctx).await
            }
            PermissionMode::Plan => {
                if is_allowed_in_plan_mode(ctx.tool_name) {
                    self.inner.before_tool(ctx).await
                } else {
                    let perm_ctx = PermCtx {
                        turn_index: ctx.turn_index,
                        tool_use_id: ctx.tool_use_id,
                        tool_name: ctx.tool_name,
                        input: ctx.input,
                        rule_action: "deny",
                        rule_comment: Some("plan mode: read-only"),
                    };
                    if let Err(e) = self.inner.permission_denied(&perm_ctx).await {
                        tracing::warn!(
                            target: caliban_common::tracing_targets::TARGET_PERMISSIONS,
                            error = %e,
                            "permission_denied hook error (non-fatal)",
                        );
                    }
                    Ok(HookDecision::Deny(format!(
                        "plan mode: '{}' is not in the read-only allowlist",
                        ctx.tool_name
                    )))
                }
            }
            PermissionMode::Auto => self.handle_auto(ctx).await,
            PermissionMode::DontAsk => {
                let inner = self.inner.before_tool(ctx).await?;
                Ok(rewrite_ask_to_allow(inner))
            }
            PermissionMode::Default => self.inner.before_tool(ctx).await,
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

impl ModeFilter {
    async fn handle_auto(&self, ctx: &ToolCtx<'_>) -> Result<HookDecision> {
        let Some(classifier) = self.classifier.as_ref() else {
            // No classifier wired: degrade to a soft_deny that falls
            // through to the inner Ask handler, matching the spec's
            // disabled-fallback semantics.
            return self.inner.before_tool(ctx).await;
        };
        let decision = classifier.classify(ctx).await;
        match decision.verdict {
            AutoVerdict::Allow => Ok(HookDecision::Allow),
            AutoVerdict::HardDeny => {
                let comment = format!("auto-mode hard_deny: {}", decision.reason);
                let perm_ctx = PermCtx {
                    turn_index: ctx.turn_index,
                    tool_use_id: ctx.tool_use_id,
                    tool_name: ctx.tool_name,
                    input: ctx.input,
                    rule_action: "deny",
                    rule_comment: Some(&comment),
                };
                if let Err(e) = self.inner.permission_denied(&perm_ctx).await {
                    tracing::warn!(
                        target: caliban_common::tracing_targets::TARGET_PERMISSIONS,
                        error = %e,
                        "permission_denied hook error (non-fatal)",
                    );
                }
                Ok(HookDecision::Deny(comment))
            }
            AutoVerdict::SoftDeny => {
                // Fall through to the inner: PermissionsHook's Ask path
                // dispatches to whichever AskHandler the binary installed
                // (TUI modal, non-interactive, etc.). The classifier's
                // reason is surfaced via tracing for now; ADR 0027 wires
                // it into the modal body.
                tracing::info!(
                    target: caliban_common::tracing_targets::TARGET_PERMISSIONS,
                    tool = ctx.tool_name,
                    reason = decision.reason.as_str(),
                    "auto-mode soft_deny → Ask",
                );
                self.inner.before_tool(ctx).await
            }
        }
    }
}

fn rewrite_ask_to_allow(decision: HookDecision) -> HookDecision {
    // Today, PermissionsHook resolves Ask synchronously into Allow/Deny via
    // its installed AskHandler before this code sees it. There is no
    // `HookDecision::Ask` enum variant exposed to outer layers — the inner
    // hook converts Ask into a concrete Allow/Deny. `dontAsk` mode flips
    // any non-interactive Deny that was synthesized by the
    // NonInteractiveAskHandler back into Allow (the spec's "every Ask
    // becomes Allow" semantics).
    //
    // We detect that synthesized-Ask case by the textual marker
    // `"requires interactive approval"` in the Deny reason — this is the
    // exact message NonInteractiveAskHandler emits. Real, rule-based
    // Denies have a different reason ("permission denied for tool …") and
    // pass through.
    match decision {
        HookDecision::Deny(reason) if reason.contains("requires interactive approval") => {
            HookDecision::Allow
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::json;

    use super::*;
    use crate::auto_mode::AutoModeConfig;
    use crate::hooks::NoopHooks;
    use crate::permissions::{Action, NonInteractiveAskHandler, PermissionsHook, Rule};

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

    fn permissions_hook(
        rules: Vec<Rule>,
        ask_handler: Arc<dyn crate::permissions::AskHandler>,
    ) -> Arc<dyn Hooks> {
        Arc::new(PermissionsHook::new(
            rules,
            ask_handler,
            Arc::new(NoopHooks),
        ))
    }

    fn default_inner_ask_deny() -> Arc<dyn Hooks> {
        let mut rules = vec![rule("Bash", Action::Ask), rule("Read", Action::Allow)];
        rules.extend(crate::permissions::default_rules());
        permissions_hook(
            rules,
            Arc::new(NonInteractiveAskHandler { auto_allow: false }),
        )
    }

    // --- PermissionMode parsing / cycling ---

    #[test]
    fn permission_mode_parses_all_variants() {
        use PermissionMode::*;
        for (s, mode) in [
            ("default", Default),
            ("acceptEdits", AcceptEdits),
            ("plan", Plan),
            ("auto", Auto),
            ("dontAsk", DontAsk),
            ("bypassPermissions", BypassPermissions),
        ] {
            assert_eq!(PermissionMode::parse(s).unwrap(), mode);
            assert_eq!(mode.as_str(), s);
        }
        assert!(PermissionMode::parse("bogus").is_err());
    }

    #[test]
    fn permission_mode_cycle_forward_completes_loop() {
        let mut m = PermissionMode::Default;
        let mut seen = vec![m];
        for _ in 0..6 {
            m = m.next();
            seen.push(m);
        }
        assert_eq!(seen.first(), seen.last());
        // All six variants appear in the cycle exactly once before wrap.
        let unique: std::collections::HashSet<_> = seen[..6].iter().copied().collect();
        assert_eq!(unique.len(), 6);
    }

    #[test]
    fn permission_mode_cycle_prev_is_inverse_of_next() {
        let modes = [
            PermissionMode::Default,
            PermissionMode::AcceptEdits,
            PermissionMode::Plan,
            PermissionMode::Auto,
            PermissionMode::DontAsk,
            PermissionMode::BypassPermissions,
        ];
        for m in modes {
            assert_eq!(m.next().prev(), m);
        }
    }

    // --- ModeFilter::default is pass-through ---

    #[tokio::test]
    async fn default_mode_is_pass_through() {
        // Inner rule: Bash → Allow.
        let mut rules = vec![rule("Bash", Action::Allow)];
        rules.extend(crate::permissions::default_rules());
        let inner = permissions_hook(
            rules,
            Arc::new(NonInteractiveAskHandler { auto_allow: false }),
        );
        let mode = SharedPermissionMode::new(PermissionMode::Default);
        let filter = ModeFilter::new(mode, inner, None, false);
        let d = filter
            .before_tool(&ctx("Bash", &json!({"command": "ls"})))
            .await
            .unwrap();
        assert!(matches!(d, HookDecision::Allow));
    }

    // --- acceptEdits ---

    #[tokio::test]
    async fn accept_edits_auto_allows_file_edit_tools() {
        // Inner returns Deny for Bash and Write to prove the filter
        // short-circuits *before* consulting the rules for Write/Edit.
        let mut rules = vec![rule("Write", Action::Deny), rule("Bash", Action::Deny)];
        rules.extend(crate::permissions::default_rules());
        let inner = permissions_hook(
            rules,
            Arc::new(NonInteractiveAskHandler { auto_allow: false }),
        );
        let mode = SharedPermissionMode::new(PermissionMode::AcceptEdits);
        let filter = ModeFilter::new(mode, inner, None, false);

        // Write/Edit/MultiEdit/NotebookEdit all short-circuit to Allow,
        // bypassing the explicit Deny in the inner rules.
        for tool in ["Write", "Edit", "MultiEdit", "NotebookEdit"] {
            let d = filter
                .before_tool(&ctx(tool, &json!({"path": "/tmp/x"})))
                .await
                .unwrap();
            assert!(
                matches!(d, HookDecision::Allow),
                "tool {tool} should auto-allow under acceptEdits",
            );
        }

        // Bash still delegates → Deny.
        let d = filter
            .before_tool(&ctx("Bash", &json!({"command": "ls"})))
            .await
            .unwrap();
        assert!(matches!(d, HookDecision::Deny(_)));
    }

    // --- plan ---

    #[tokio::test]
    async fn plan_mode_denies_outside_allowlist() {
        let inner = default_inner_ask_deny();
        let mode = SharedPermissionMode::new(PermissionMode::Plan);
        let filter = ModeFilter::new(mode, inner, None, false);

        let d = filter
            .before_tool(&ctx("Bash", &json!({"command": "ls"})))
            .await
            .unwrap();
        assert!(matches!(d, HookDecision::Deny(_)));

        // Read is on the plan-mode allowlist → delegates to inner (Allow).
        let d = filter
            .before_tool(&ctx("Read", &json!({"path": "/tmp/x"})))
            .await
            .unwrap();
        assert!(matches!(d, HookDecision::Allow));
    }

    // --- dontAsk ---

    #[tokio::test]
    async fn dont_ask_rewrites_ask_to_allow() {
        // Inner is a PermissionsHook whose Bash rule is Ask + non-interactive
        // handler (auto_allow=false). Without the filter, that yields Deny.
        let inner = default_inner_ask_deny();
        let mode = SharedPermissionMode::new(PermissionMode::DontAsk);
        let filter = ModeFilter::new(mode, inner, None, false);

        let d = filter
            .before_tool(&ctx("Bash", &json!({"command": "ls"})))
            .await
            .unwrap();
        assert!(
            matches!(d, HookDecision::Allow),
            "dontAsk must flip the synthesized Ask→Deny into Allow"
        );
    }

    #[tokio::test]
    async fn dont_ask_preserves_explicit_rule_deny() {
        // Inner rule: Bash → Deny (rule-based, not Ask-synthesized).
        let mut rules = vec![rule("Bash", Action::Deny)];
        rules.extend(crate::permissions::default_rules());
        let inner = permissions_hook(
            rules,
            Arc::new(NonInteractiveAskHandler { auto_allow: false }),
        );
        let mode = SharedPermissionMode::new(PermissionMode::DontAsk);
        let filter = ModeFilter::new(mode, inner, None, false);

        let d = filter
            .before_tool(&ctx("Bash", &json!({"command": "ls"})))
            .await
            .unwrap();
        assert!(
            matches!(d, HookDecision::Deny(_)),
            "dontAsk must NOT override an explicit rule-based Deny"
        );
    }

    // --- bypassPermissions ---

    #[tokio::test]
    async fn bypass_with_latch_allows_everything() {
        // Even an explicit rule-based Deny is overridden when the latch is set.
        let mut rules = vec![rule("Bash", Action::Deny)];
        rules.extend(crate::permissions::default_rules());
        let inner = permissions_hook(
            rules,
            Arc::new(NonInteractiveAskHandler { auto_allow: false }),
        );
        let mode = SharedPermissionMode::new(PermissionMode::BypassPermissions);
        let filter = ModeFilter::new(mode, inner, None, true);

        let d = filter
            .before_tool(&ctx("Bash", &json!({"command": "rm -rf /"})))
            .await
            .unwrap();
        assert!(matches!(d, HookDecision::Allow));
    }

    #[tokio::test]
    async fn bypass_without_latch_degrades_to_default() {
        // Without the latch, the filter must NOT pass Allow for an explicit Deny.
        let mut rules = vec![rule("Bash", Action::Deny)];
        rules.extend(crate::permissions::default_rules());
        let inner = permissions_hook(
            rules,
            Arc::new(NonInteractiveAskHandler { auto_allow: false }),
        );
        let mode = SharedPermissionMode::new(PermissionMode::BypassPermissions);
        let filter = ModeFilter::new(mode, inner, None, false);

        let d = filter
            .before_tool(&ctx("Bash", &json!({"command": "rm -rf /"})))
            .await
            .unwrap();
        assert!(matches!(d, HookDecision::Deny(_)));
    }

    // --- bypass startup-gate ---

    #[test]
    fn resolve_startup_mode_blocks_bypass_without_latch() {
        let err =
            crate::permission_mode::resolve_startup_mode(Some("bypassPermissions"), None, false)
                .unwrap_err();
        assert!(err.contains("--allow-dangerously-skip-permissions"));
    }

    #[test]
    fn resolve_startup_mode_cli_overrides_env() {
        let m =
            crate::permission_mode::resolve_startup_mode(Some("acceptEdits"), Some("plan"), false)
                .unwrap();
        assert_eq!(m, PermissionMode::AcceptEdits);
    }

    #[test]
    fn resolve_startup_mode_env_when_cli_absent() {
        let m = crate::permission_mode::resolve_startup_mode(None, Some("plan"), false).unwrap();
        assert_eq!(m, PermissionMode::Plan);
    }

    #[test]
    fn resolve_startup_mode_default_when_neither() {
        let m = crate::permission_mode::resolve_startup_mode(None, None, false).unwrap();
        assert_eq!(m, PermissionMode::Default);
    }

    // --- auto mode with a fake classifier provider ---

    /// Provider that returns a queued JSON body inside a single text block.
    struct ScriptedProvider {
        bodies: Mutex<Vec<String>>,
        call_count: std::sync::atomic::AtomicUsize,
    }

    impl ScriptedProvider {
        fn new(bodies: Vec<String>) -> Self {
            Self {
                bodies: Mutex::new(bodies),
                call_count: std::sync::atomic::AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.call_count.load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    #[async_trait]
    impl caliban_provider::Provider for ScriptedProvider {
        async fn complete(
            &self,
            _req: caliban_provider::CompletionRequest,
        ) -> caliban_provider::error::Result<caliban_provider::CompletionResponse> {
            self.call_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let body = self
                .bodies
                .lock()
                .unwrap()
                .pop()
                .unwrap_or_else(|| r#"{"decision":"soft_deny","reason":"no more"}"#.into());
            Ok(caliban_provider::CompletionResponse {
                id: "scripted".into(),
                model: "scripted".into(),
                message: caliban_provider::Message {
                    role: caliban_provider::Role::Assistant,
                    content: vec![caliban_provider::ContentBlock::Text(
                        caliban_provider::TextBlock {
                            text: body,
                            cache_control: None,
                        },
                    )],
                },
                stop_reason: caliban_provider::StopReason::EndTurn,
                stop_sequence: None,
                usage: caliban_provider::Usage::default(),
            })
        }

        async fn stream(
            &self,
            _req: caliban_provider::CompletionRequest,
        ) -> caliban_provider::error::Result<caliban_provider::MessageStream> {
            unimplemented!("scripted provider does not stream")
        }

        fn capabilities(&self, _model: &str) -> caliban_provider::Capabilities {
            caliban_provider::Capabilities {
                max_input_tokens: 1024,
                max_output_tokens: 1024,
                vision: false,
                tool_use: caliban_provider::ToolUseCapability::None,
                thinking: false,
                prompt_caching: caliban_provider::PromptCachingCapability::None,
                json_mode: false,
                streaming: false,
                stop_sequences: false,
                top_k: false,
                system_prompt: caliban_provider::SystemPromptCapability::SeparateField,
                refusal_field: false,
            }
        }

        fn list_models(&self) -> Vec<caliban_provider::ModelInfo> {
            vec![]
        }

        fn name(&self) -> &'static str {
            "scripted"
        }
    }

    /// Errors immediately for every call (used to test fallback).
    struct ErrorProvider;

    #[async_trait]
    impl caliban_provider::Provider for ErrorProvider {
        async fn complete(
            &self,
            _req: caliban_provider::CompletionRequest,
        ) -> caliban_provider::error::Result<caliban_provider::CompletionResponse> {
            Err(caliban_provider::Error::ServerError {
                status: 500,
                body: "boom".into(),
            })
        }
        async fn stream(
            &self,
            _req: caliban_provider::CompletionRequest,
        ) -> caliban_provider::error::Result<caliban_provider::MessageStream> {
            unimplemented!("error provider does not stream")
        }
        fn capabilities(&self, _model: &str) -> caliban_provider::Capabilities {
            ScriptedProvider::new(vec![]).capabilities("")
        }
        fn list_models(&self) -> Vec<caliban_provider::ModelInfo> {
            vec![]
        }
        fn name(&self) -> &'static str {
            "error"
        }
    }

    fn auto_filter(
        provider: Arc<dyn caliban_provider::Provider + Send + Sync>,
        config: AutoModeConfig,
        inner: Arc<dyn Hooks>,
    ) -> ModeFilter {
        let classifier = Arc::new(AutoModeClassifier::new(provider, "haiku", config));
        ModeFilter::new(
            SharedPermissionMode::new(PermissionMode::Auto),
            inner,
            Some(classifier),
            false,
        )
    }

    #[tokio::test]
    async fn auto_mode_classifier_allow_flows_through() {
        let provider = Arc::new(ScriptedProvider::new(vec![
            r#"{"decision":"allow","reason":"safe read"}"#.into(),
        ]));
        let inner = default_inner_ask_deny();
        let filter = auto_filter(provider.clone(), AutoModeConfig::default(), inner);
        let d = filter
            .before_tool(&ctx("Bash", &json!({"command": "ls"})))
            .await
            .unwrap();
        assert!(matches!(d, HookDecision::Allow));
        assert_eq!(provider.calls(), 1);
    }

    #[tokio::test]
    async fn auto_mode_classifier_hard_deny_flows_through() {
        let provider = Arc::new(ScriptedProvider::new(vec![
            r#"{"decision":"hard_deny","reason":"dangerous"}"#.into(),
        ]));
        let inner = default_inner_ask_deny();
        let filter = auto_filter(provider, AutoModeConfig::default(), inner);
        let d = filter
            .before_tool(&ctx("Bash", &json!({"command": "rm /"})))
            .await
            .unwrap();
        match d {
            HookDecision::Deny(reason) => assert!(reason.contains("hard_deny")),
            other => panic!("expected Deny, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn auto_mode_soft_deny_falls_through_to_ask() {
        // Soft-deny under the spec routes back through the inner. The inner
        // here is a PermissionsHook whose Bash rule is Ask + auto_allow=true,
        // so the soft_deny fall-through becomes Allow.
        let provider = Arc::new(ScriptedProvider::new(vec![
            r#"{"decision":"soft_deny","reason":"unclear"}"#.into(),
        ]));
        let mut rules = vec![rule("Bash", Action::Ask)];
        rules.extend(crate::permissions::default_rules());
        let inner = permissions_hook(
            rules,
            Arc::new(NonInteractiveAskHandler { auto_allow: true }),
        );
        let filter = auto_filter(provider, AutoModeConfig::default(), inner);
        let d = filter
            .before_tool(&ctx("Bash", &json!({"command": "ls"})))
            .await
            .unwrap();
        // The inner Ask resolves to Allow via the auto-allow handler.
        assert!(matches!(d, HookDecision::Allow));
    }

    #[tokio::test]
    async fn auto_mode_cache_hits_second_call() {
        let provider = Arc::new(ScriptedProvider::new(vec![
            r#"{"decision":"allow","reason":"once"}"#.into(),
        ]));
        let inner = default_inner_ask_deny();
        let filter = auto_filter(provider.clone(), AutoModeConfig::default(), inner);
        let input = json!({"command": "ls"});

        let d1 = filter.before_tool(&ctx("Bash", &input)).await.unwrap();
        let d2 = filter.before_tool(&ctx("Bash", &input)).await.unwrap();
        assert!(matches!(d1, HookDecision::Allow));
        assert!(matches!(d2, HookDecision::Allow));
        // Provider called once; second call served from cache.
        assert_eq!(provider.calls(), 1);
    }

    #[tokio::test]
    async fn auto_mode_disabled_short_circuits_classifier() {
        let provider = Arc::new(ScriptedProvider::new(vec![]));
        let inner = default_inner_ask_deny();
        let cfg = AutoModeConfig {
            disabled: true,
            ..AutoModeConfig::default()
        };
        let filter = auto_filter(provider.clone(), cfg, inner);
        let _ = filter
            .before_tool(&ctx("Bash", &json!({"command": "ls"})))
            .await
            .unwrap();
        // The classifier was never called.
        assert_eq!(provider.calls(), 0);
    }

    #[tokio::test]
    async fn auto_mode_static_rule_short_circuits_classifier() {
        let provider = Arc::new(ScriptedProvider::new(vec![]));
        let inner = default_inner_ask_deny();
        let cfg = AutoModeConfig {
            hard_deny: vec!["Bash:rm *".into()],
            ..AutoModeConfig::default()
        };
        let filter = auto_filter(provider.clone(), cfg, inner);
        let d = filter
            .before_tool(&ctx("Bash", &json!({"command": "rm -rf /tmp"})))
            .await
            .unwrap();
        assert!(matches!(d, HookDecision::Deny(_)));
        assert_eq!(provider.calls(), 0);
    }

    #[tokio::test]
    async fn auto_mode_environment_short_circuits_classifier() {
        let provider = Arc::new(ScriptedProvider::new(vec![]));
        let inner = default_inner_ask_deny();
        let cfg = AutoModeConfig {
            environment: vec!["Read".into(), "Glob".into(), "Grep".into()],
            ..AutoModeConfig::default()
        };
        let filter = auto_filter(provider.clone(), cfg, inner);
        let d = filter
            .before_tool(&ctx("Read", &json!({"path": "/tmp/x"})))
            .await
            .unwrap();
        assert!(matches!(d, HookDecision::Allow));
        assert_eq!(provider.calls(), 0);
    }

    #[tokio::test]
    async fn auto_mode_malformed_output_falls_back_to_soft_deny() {
        let provider = Arc::new(ScriptedProvider::new(vec!["not valid json".into()]));
        // Inner: Bash → Ask with auto_allow=true so the soft-deny fall-through
        // resolves to Allow if and only if the filter correctly treats the
        // malformed output as a SoftDeny.
        let mut rules = vec![rule("Bash", Action::Ask)];
        rules.extend(crate::permissions::default_rules());
        let inner = permissions_hook(
            rules,
            Arc::new(NonInteractiveAskHandler { auto_allow: true }),
        );
        let filter = auto_filter(provider, AutoModeConfig::default(), inner);
        let d = filter
            .before_tool(&ctx("Bash", &json!({"command": "ls"})))
            .await
            .unwrap();
        assert!(matches!(d, HookDecision::Allow));
    }

    #[tokio::test]
    async fn auto_mode_provider_error_falls_back_to_soft_deny() {
        let provider = Arc::new(ErrorProvider);
        let mut rules = vec![rule("Bash", Action::Ask)];
        rules.extend(crate::permissions::default_rules());
        let inner = permissions_hook(
            rules,
            Arc::new(NonInteractiveAskHandler { auto_allow: true }),
        );
        let filter = auto_filter(provider, AutoModeConfig::default(), inner);
        let d = filter
            .before_tool(&ctx("Bash", &json!({"command": "ls"})))
            .await
            .unwrap();
        // Soft-deny fall-through → inner Ask → auto_allow=true → Allow.
        assert!(matches!(d, HookDecision::Allow));
    }

    #[test]
    fn defaults_expansion_produces_non_empty_lists() {
        let cfg = AutoModeConfig {
            hard_deny: vec![crate::auto_mode::DEFAULTS_TOKEN.into()],
            soft_deny: vec![crate::auto_mode::DEFAULTS_TOKEN.into()],
            allow: vec![crate::auto_mode::DEFAULTS_TOKEN.into()],
            environment: vec![crate::auto_mode::DEFAULTS_TOKEN.into()],
            disabled: false,
        }
        .with_defaults_expanded();
        assert!(!cfg.hard_deny.is_empty());
        assert!(cfg.hard_deny.iter().any(|p| p.starts_with("Bash:sudo")));
        assert!(!cfg.soft_deny.is_empty());
        assert!(!cfg.allow.is_empty());
        assert!(!cfg.environment.is_empty());
    }

    #[test]
    fn shared_permission_mode_is_lock_free_readable_after_store() {
        let m = SharedPermissionMode::default();
        assert_eq!(m.load(), PermissionMode::Default);
        m.store(PermissionMode::Plan);
        assert_eq!(m.load(), PermissionMode::Plan);
        let clone = m.clone();
        // Store via one handle, observe via the other.
        clone.store(PermissionMode::Auto);
        assert_eq!(m.load(), PermissionMode::Auto);
    }
}
