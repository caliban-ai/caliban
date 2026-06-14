//! `HeadlessHookSink` — a [`Hooks`] implementation that emits `hook_event`
//! frames into a shared queue when `--include-hook-events` is set.
//!
//! Pairs with [`CompositeHooks`](caliban_agent_core::CompositeHooks): the
//! sink is added as the outermost layer so it observes every event without
//! affecting `before_*` short-circuit semantics (Allow is the only decision
//! it ever returns).

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use caliban_agent_core::{
    CompactCtx, CompactOutcome, ConfigChangeCtx, CwdChangedCtx, FileChangedCtx, HookDecision,
    Hooks, NotificationCtx, PermCtx, PromptCtx, SessionCtx, SessionOutcome, SessionStartOutcome,
    SubagentCtx, SubagentOutcome, TaskCtx, TaskOutcome, ToolCtx, TurnCtx,
};
use caliban_agent_core::{Result as HookResult, TurnDecision, TurnOutcome};
use caliban_provider::ContentBlock;
use serde_json::{Map, Value};

use crate::headless::events::{HookEvent, hook_event};

/// Shared buffer of emitted hook events. Cloned into the [`HeadlessHookSink`]
/// and into the headless driver so the driver can drain frames as the run
/// progresses.
pub(crate) type HookEventBuffer = Arc<Mutex<Vec<HookEvent>>>;

/// Construct an empty event buffer.
#[must_use]
pub(crate) fn new_event_buffer() -> HookEventBuffer {
    Arc::new(Mutex::new(Vec::new()))
}

/// A [`Hooks`] implementation that records every fired event as a
/// `hook_event` frame.
pub(crate) struct HeadlessHookSink {
    buffer: HookEventBuffer,
}

impl HeadlessHookSink {
    /// Construct a new sink that pushes into `buffer`.
    #[must_use]
    pub(crate) fn new(buffer: HookEventBuffer) -> Self {
        Self { buffer }
    }

    fn push(&self, name: &str, payload: Value) {
        let mut guard = self
            .buffer
            .lock()
            .expect("HeadlessHookSink buffer lock poisoned");
        guard.push(hook_event(name, payload));
    }
}

impl std::fmt::Debug for HeadlessHookSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeadlessHookSink").finish()
    }
}

#[async_trait]
impl Hooks for HeadlessHookSink {
    async fn before_turn(&self, ctx: &TurnCtx<'_>) -> HookResult<()> {
        let mut m = Map::new();
        m.insert("turn_index".into(), Value::from(ctx.turn_index));
        self.push("PreTurn", Value::Object(m));
        Ok(())
    }

    async fn after_turn(
        &self,
        ctx: &TurnCtx<'_>,
        _outcome: &TurnOutcome,
    ) -> HookResult<TurnDecision> {
        let mut m = Map::new();
        m.insert("turn_index".into(), Value::from(ctx.turn_index));
        self.push("PostTurn", Value::Object(m));
        Ok(TurnDecision::Continue)
    }

    async fn before_tool(&self, ctx: &ToolCtx<'_>) -> HookResult<HookDecision> {
        let mut m = Map::new();
        m.insert("turn_index".into(), Value::from(ctx.turn_index));
        m.insert("tool_use_id".into(), Value::from(ctx.tool_use_id));
        m.insert("matcher".into(), Value::from(ctx.tool_name));
        m.insert("decision".into(), Value::from("allow"));
        self.push("PreToolUse", Value::Object(m));
        Ok(HookDecision::Allow)
    }

    async fn after_tool(
        &self,
        ctx: &ToolCtx<'_>,
        result: &std::result::Result<Vec<ContentBlock>, caliban_agent_core::ToolError>,
    ) -> HookResult<()> {
        let mut m = Map::new();
        m.insert("turn_index".into(), Value::from(ctx.turn_index));
        m.insert("tool_use_id".into(), Value::from(ctx.tool_use_id));
        m.insert("matcher".into(), Value::from(ctx.tool_name));
        m.insert("is_error".into(), Value::from(result.is_err()));
        self.push("PostToolUse", Value::Object(m));
        Ok(())
    }

    async fn session_start(&self, ctx: &SessionCtx<'_>) -> HookResult<SessionStartOutcome> {
        let mut m = Map::new();
        m.insert("session_id".into(), Value::from(ctx.session_id));
        m.insert("cwd".into(), Value::from(ctx.cwd.display().to_string()));
        m.insert("provider".into(), Value::from(ctx.provider));
        m.insert("model".into(), Value::from(ctx.model));
        self.push("SessionStart", Value::Object(m));
        Ok(SessionStartOutcome::default())
    }

    async fn session_end(&self, ctx: &SessionCtx<'_>, outcome: &SessionOutcome) -> HookResult<()> {
        let mut m = Map::new();
        m.insert("session_id".into(), Value::from(ctx.session_id));
        m.insert("turn_count".into(), Value::from(outcome.turn_count));
        m.insert("input_tokens".into(), Value::from(outcome.input_tokens));
        m.insert("output_tokens".into(), Value::from(outcome.output_tokens));
        self.push("SessionEnd", Value::Object(m));
        Ok(())
    }

    async fn user_prompt_submit(&self, ctx: &PromptCtx<'_>) -> HookResult<HookDecision> {
        let mut m = Map::new();
        m.insert("session_id".into(), Value::from(ctx.session_id));
        m.insert("turn_index".into(), Value::from(ctx.turn_index));
        m.insert("prompt".into(), Value::from(ctx.prompt));
        self.push("UserPromptSubmit", Value::Object(m));
        Ok(HookDecision::Allow)
    }

    async fn pre_compact(&self, ctx: &CompactCtx<'_>) -> HookResult<()> {
        let mut m = Map::new();
        m.insert("strategy".into(), Value::from(ctx.strategy));
        m.insert(
            "token_count_before".into(),
            Value::from(ctx.token_count_before),
        );
        self.push("PreCompact", Value::Object(m));
        Ok(())
    }

    async fn post_compact(
        &self,
        _ctx: &CompactCtx<'_>,
        outcome: &CompactOutcome,
    ) -> HookResult<()> {
        let mut m = Map::new();
        m.insert("compacted".into(), Value::from(outcome.compacted));
        m.insert(
            "token_count_after".into(),
            Value::from(outcome.token_count_after),
        );
        self.push("PostCompact", Value::Object(m));
        Ok(())
    }

    async fn config_change(&self, ctx: &ConfigChangeCtx<'_>) -> HookResult<()> {
        let mut m = Map::new();
        m.insert(
            "changed_keys".into(),
            Value::Array(
                ctx.changed_keys
                    .iter()
                    .map(|k| Value::String(k.clone()))
                    .collect(),
            ),
        );
        self.push("ConfigChange", Value::Object(m));
        Ok(())
    }

    async fn cwd_changed(&self, ctx: &CwdChangedCtx<'_>) -> HookResult<()> {
        let mut m = Map::new();
        m.insert(
            "old_cwd".into(),
            Value::from(ctx.old_cwd.display().to_string()),
        );
        m.insert(
            "new_cwd".into(),
            Value::from(ctx.new_cwd.display().to_string()),
        );
        self.push("CwdChanged", Value::Object(m));
        Ok(())
    }

    async fn file_changed(&self, ctx: &FileChangedCtx<'_>) -> HookResult<()> {
        let mut m = Map::new();
        m.insert("path".into(), Value::from(ctx.path.display().to_string()));
        m.insert("kind".into(), Value::from(ctx.kind.as_str()));
        m.insert("tool".into(), Value::from(ctx.tool));
        self.push("FileChanged", Value::Object(m));
        Ok(())
    }

    async fn permission_request(&self, ctx: &PermCtx<'_>) -> HookResult<()> {
        let mut m = Map::new();
        m.insert("tool_use_id".into(), Value::from(ctx.tool_use_id));
        m.insert("tool_name".into(), Value::from(ctx.tool_name));
        m.insert("rule_action".into(), Value::from(ctx.rule_action));
        self.push("PermissionRequest", Value::Object(m));
        Ok(())
    }

    async fn permission_denied(&self, ctx: &PermCtx<'_>) -> HookResult<()> {
        let mut m = Map::new();
        m.insert("tool_use_id".into(), Value::from(ctx.tool_use_id));
        m.insert("tool_name".into(), Value::from(ctx.tool_name));
        m.insert("rule_action".into(), Value::from(ctx.rule_action));
        self.push("PermissionDenied", Value::Object(m));
        Ok(())
    }

    async fn notification(&self, ctx: &NotificationCtx<'_>) -> HookResult<()> {
        let mut m = Map::new();
        m.insert("level".into(), Value::from(ctx.level.as_str()));
        m.insert("message".into(), Value::from(ctx.message));
        self.push("Notification", Value::Object(m));
        Ok(())
    }

    async fn subagent_start(&self, ctx: &SubagentCtx<'_>) -> HookResult<()> {
        let mut m = Map::new();
        m.insert("agent_name".into(), Value::from(ctx.agent_name));
        m.insert("task_id".into(), Value::from(ctx.task_id));
        self.push("SubagentStart", Value::Object(m));
        Ok(())
    }

    async fn subagent_stop(
        &self,
        ctx: &SubagentCtx<'_>,
        outcome: &SubagentOutcome,
    ) -> HookResult<()> {
        let mut m = Map::new();
        m.insert("agent_name".into(), Value::from(ctx.agent_name));
        m.insert("task_id".into(), Value::from(ctx.task_id));
        m.insert("success".into(), Value::from(outcome.success));
        self.push("SubagentStop", Value::Object(m));
        Ok(())
    }

    async fn task_created(&self, ctx: &TaskCtx<'_>) -> HookResult<()> {
        let mut m = Map::new();
        m.insert("task_id".into(), Value::from(ctx.task_id));
        m.insert("status".into(), Value::from(ctx.status));
        self.push("TaskCreated", Value::Object(m));
        Ok(())
    }

    async fn task_completed(&self, ctx: &TaskCtx<'_>, outcome: &TaskOutcome) -> HookResult<()> {
        let mut m = Map::new();
        m.insert("task_id".into(), Value::from(ctx.task_id));
        m.insert(
            "terminal_status".into(),
            Value::from(outcome.terminal_status.clone()),
        );
        self.push("TaskCompleted", Value::Object(m));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn session_start_emits_camelcase_envelope() {
        let buf = new_event_buffer();
        let sink = HeadlessHookSink::new(Arc::clone(&buf));
        let cwd = PathBuf::from("/tmp");
        let ctx = SessionCtx {
            session_id: "abc",
            cwd: &cwd,
            provider: "mock",
            model: "m",
        };
        sink.session_start(&ctx).await.unwrap();
        let frames = buf.lock().unwrap();
        assert_eq!(frames.len(), 1);
        let json = serde_json::to_value(&frames[0]).unwrap();
        assert_eq!(json["type"], "hook_event");
        assert_eq!(json["hookEventName"], "SessionStart");
        assert_eq!(json["hookSpecificOutput"]["session_id"], "abc");
        assert_eq!(json["hookSpecificOutput"]["provider"], "mock");
    }

    #[tokio::test]
    async fn session_end_records_outcome() {
        let buf = new_event_buffer();
        let sink = HeadlessHookSink::new(Arc::clone(&buf));
        let cwd = PathBuf::from("/tmp");
        let ctx = SessionCtx {
            session_id: "abc",
            cwd: &cwd,
            provider: "mock",
            model: "m",
        };
        let outcome = SessionOutcome {
            turn_count: 3,
            input_tokens: 100,
            output_tokens: 50,
        };
        sink.session_end(&ctx, &outcome).await.unwrap();
        let frames = buf.lock().unwrap();
        let json = serde_json::to_value(&frames[0]).unwrap();
        assert_eq!(json["hookEventName"], "SessionEnd");
        assert_eq!(json["hookSpecificOutput"]["turn_count"], 3);
        assert_eq!(json["hookSpecificOutput"]["input_tokens"], 100);
    }

    #[tokio::test]
    async fn pre_tool_use_emits_decision_and_matcher() {
        let buf = new_event_buffer();
        let sink = HeadlessHookSink::new(Arc::clone(&buf));
        let input = serde_json::json!({"foo": 1});
        let ctx = ToolCtx {
            turn_index: 0,
            tool_use_id: "tu_1",
            tool_name: "Bash",
            input: &input,
        };
        let dec = sink.before_tool(&ctx).await.unwrap();
        assert!(matches!(dec, HookDecision::Allow));
        let frames = buf.lock().unwrap();
        let json = serde_json::to_value(&frames[0]).unwrap();
        assert_eq!(json["hookEventName"], "PreToolUse");
        assert_eq!(json["hookSpecificOutput"]["matcher"], "Bash");
        assert_eq!(json["hookSpecificOutput"]["decision"], "allow");
    }
}
