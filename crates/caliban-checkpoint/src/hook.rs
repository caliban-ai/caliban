//! [`CheckpointHook`] — `Hooks` impl driving a [`CheckpointRecorder`].
//!
//! Lifecycle:
//!
//! - `before_run` → recorder opens a new prompt directory + empty manifest.
//! - `before_tool` (`Write`/`Edit`/`MultiEdit`/`NotebookEdit`) → parse the
//!   path from input JSON, call `recorder.capture(path)` so the pre-image
//!   is captured *before* the write hits disk.
//! - `after_run` → recorder flushes the manifest.
//!
//! Returns `Allow` from every `before_tool`/`user_prompt_submit` call —
//! the hook is observation-only. Plan-mode prompts emit an empty
//! manifest with `kind: Plan` so `/rewind` can target the prompt for
//! conversation rewind.

use async_trait::async_trait;
use caliban_agent_core::{
    HookDecision, Hooks, Result as HookResult, RunCtx, RunHookOutcome, ToolCtx,
};

use crate::manifest::ManifestKind;
use crate::recorder::CheckpointRecorder;

/// Returns `true` when the tool name is one we snapshot.
#[must_use]
pub fn tracked_tool(name: &str) -> bool {
    matches!(name, "Write" | "Edit" | "MultiEdit" | "NotebookEdit")
}

/// Extract the file paths the tool will touch from its input JSON. Today
/// every tracked tool takes a single `path` string. Returned vec is empty
/// when no path could be extracted (skip with a tracing warn).
#[must_use]
pub fn extract_paths(input: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(p) = input.get("path").and_then(serde_json::Value::as_str) {
        out.push(p.to_string());
    }
    out
}

/// Hook impl that ferries the recorder around.
#[derive(Debug, Clone)]
pub struct CheckpointHook {
    recorder: CheckpointRecorder,
    workspace_root: std::path::PathBuf,
    plan_mode: Option<caliban_agent_core::SharedPlanMode>,
}

impl CheckpointHook {
    /// Construct with a recorder + workspace root.
    #[must_use]
    pub fn new(
        recorder: CheckpointRecorder,
        workspace_root: impl Into<std::path::PathBuf>,
    ) -> Self {
        Self {
            recorder,
            workspace_root: workspace_root.into(),
            plan_mode: None,
        }
    }

    /// Attach a plan-mode flag so plan-mode prompts emit `ManifestKind::Plan`.
    #[must_use]
    pub fn with_plan_mode(mut self, flag: caliban_agent_core::SharedPlanMode) -> Self {
        self.plan_mode = Some(flag);
        self
    }

    /// Resolve a (possibly relative) path string against the workspace root.
    fn resolve(&self, raw: &str) -> std::path::PathBuf {
        let p = std::path::PathBuf::from(raw);
        if p.is_absolute() {
            p
        } else {
            self.workspace_root.join(p)
        }
    }
}

#[async_trait]
impl Hooks for CheckpointHook {
    async fn before_run(&self, ctx: &RunCtx<'_>) -> HookResult<()> {
        if std::env::var(crate::store::DISABLED_ENV).is_ok_and(|v| !v.is_empty()) {
            return Ok(());
        }
        let kind = if self
            .plan_mode
            .as_ref()
            .is_some_and(|f| f.load(std::sync::atomic::Ordering::Relaxed))
        {
            ManifestKind::Plan
        } else {
            ManifestKind::Files
        };
        let title = ctx
            .user_message
            .map(|m| {
                m.content
                    .iter()
                    .filter_map(|cb| match cb {
                        caliban_provider::ContentBlock::Text(t) => Some(t.text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        let trimmed: String = title.chars().take(80).collect();
        // Resolve a *monotonic* prompt index from the store, not a blindly-
        // trusted `ctx.prompt_index`. If the caller passes a stale/constant
        // index (e.g. always 1), every prompt would otherwise overwrite
        // `prompt-001` and collapse rewind history to one slot (#220 issue 2).
        // Take the larger of the caller's index and the next free slot so a
        // correctly-incrementing caller is still honored. (A read-then-act race
        // under concurrent sub-agents remains — tracked alongside issue 3.)
        let next_free = self.recorder.store().next_prompt_index().unwrap_or(1);
        let prompt_index = ctx.prompt_index.max(next_free).max(1);
        if let Err(e) = self.recorder.open_prompt(prompt_index, kind, trimmed).await {
            tracing::warn!(error = %e, "CheckpointHook::before_run: open_prompt failed");
        }
        Ok(())
    }

    async fn after_run(&self, _ctx: &RunCtx<'_>, _outcome: &RunHookOutcome) -> HookResult<()> {
        if std::env::var(crate::store::DISABLED_ENV).is_ok_and(|v| !v.is_empty()) {
            return Ok(());
        }
        if let Err(e) = self.recorder.close_prompt().await {
            tracing::warn!(error = %e, "CheckpointHook::after_run: close_prompt failed");
        }
        Ok(())
    }

    async fn before_tool(&self, ctx: &ToolCtx<'_>) -> HookResult<HookDecision> {
        if std::env::var(crate::store::DISABLED_ENV).is_ok_and(|v| !v.is_empty()) {
            return Ok(HookDecision::Allow);
        }
        if !tracked_tool(ctx.tool_name) {
            return Ok(HookDecision::Allow);
        }
        for raw in extract_paths(ctx.input) {
            let path = self.resolve(&raw);
            match self
                .recorder
                .capture(&path, ctx.tool_name, ctx.tool_use_id)
                .await
            {
                Ok(()) => {}
                Err(crate::CheckpointError::Skipped { reason }) => {
                    tracing::debug!(reason = %reason, tool = ctx.tool_name, "checkpoint capture skipped");
                }
                Err(e) => {
                    tracing::warn!(error = %e, tool = ctx.tool_name, "checkpoint capture failed");
                    self.recorder.mark_partial().await;
                }
            }
        }
        Ok(HookDecision::Allow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::CheckpointStore;
    use caliban_agent_core::NoopHooks;
    use std::path::Path;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    fn fixture() -> (TempDir, CheckpointHook, CheckpointRecorder) {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let canonical_ws = std::fs::canonicalize(&workspace).unwrap();
        let store_root = tmp.path().join("store");
        std::fs::create_dir_all(&store_root).unwrap();
        let store = CheckpointStore::open_in(&store_root, &canonical_ws, "sess-1").unwrap();
        let rec = CheckpointRecorder::new(store, canonical_ws.clone());
        let hook = CheckpointHook::new(rec.clone(), canonical_ws);
        (tmp, hook, rec)
    }

    #[tokio::test]
    async fn before_run_after_run_open_and_close_prompt() {
        let (_tmp, hook, rec) = fixture();
        let ctx = RunCtx {
            session_id: "sess-1",
            workspace_root: Path::new("/"),
            user_message: None,
            prompt_index: 1,
            cancel: CancellationToken::new(),
        };
        hook.before_run(&ctx).await.unwrap();
        assert!(rec.snapshot_manifest().await.is_some());
        let outcome = RunHookOutcome {
            turn_count: 0,
            input_tokens: 0,
            output_tokens: 0,
            success: true,
        };
        hook.after_run(&ctx, &outcome).await.unwrap();
        assert!(rec.snapshot_manifest().await.is_none());
        // Manifest file exists.
        assert!(rec.store().load_manifest(1).is_ok());
    }

    #[tokio::test]
    async fn monotonic_index_does_not_overwrite_prompt_001() {
        // #220 issue 2: even when the caller passes a constant prompt_index,
        // successive runs must land in distinct prompt dirs (monotonic), not
        // collapse onto prompt-001.
        let (_tmp, hook, rec) = fixture();
        let outcome = RunHookOutcome {
            turn_count: 0,
            input_tokens: 0,
            output_tokens: 0,
            success: true,
        };
        for _ in 0..3 {
            let ctx = RunCtx {
                session_id: "sess-1",
                workspace_root: Path::new("/"),
                user_message: None,
                prompt_index: 1, // stale/constant caller index
                cancel: CancellationToken::new(),
            };
            hook.before_run(&ctx).await.unwrap();
            hook.after_run(&ctx, &outcome).await.unwrap();
        }
        // Three distinct manifests must exist (prompt-001..003), not one.
        assert!(rec.store().load_manifest(1).is_ok());
        assert!(
            rec.store().load_manifest(2).is_ok(),
            "second run must not overwrite prompt-001"
        );
        assert!(
            rec.store().load_manifest(3).is_ok(),
            "third run must not overwrite either"
        );
    }

    #[tokio::test]
    async fn before_tool_captures_path_for_tracked_tools() {
        let (tmp, hook, rec) = fixture();
        let workspace = std::fs::canonicalize(tmp.path().join("ws")).unwrap();
        let file = workspace.join("a.txt");
        std::fs::write(&file, "hello").unwrap();
        let ctx = RunCtx {
            session_id: "sess-1",
            workspace_root: &workspace,
            user_message: None,
            prompt_index: 1,
            cancel: CancellationToken::new(),
        };
        hook.before_run(&ctx).await.unwrap();
        let input = serde_json::json!({"path": "a.txt", "content": "x"});
        let tctx = ToolCtx {
            session_id: "test-session",
            turn_index: 0,
            tool_use_id: "tu_1",
            tool_name: "Write",
            input: &input,
            is_read_only: false,
        };
        let decision = hook.before_tool(&tctx).await.unwrap();
        assert!(matches!(decision, HookDecision::Allow));
        let m = rec.snapshot_manifest().await.unwrap();
        assert_eq!(m.entries.len(), 1);
    }

    #[tokio::test]
    async fn untracked_tools_skip_capture() {
        let (_tmp, hook, rec) = fixture();
        hook.before_run(&RunCtx {
            session_id: "s",
            workspace_root: Path::new("/"),
            user_message: None,
            prompt_index: 1,
            cancel: CancellationToken::new(),
        })
        .await
        .unwrap();
        let input = serde_json::json!({"command": "rm a.txt"});
        let tctx = ToolCtx {
            session_id: "test-session",
            turn_index: 0,
            tool_use_id: "tu_1",
            tool_name: "Bash",
            input: &input,
            is_read_only: false,
        };
        hook.before_tool(&tctx).await.unwrap();
        let m = rec.snapshot_manifest().await.unwrap();
        assert!(m.entries.is_empty(), "Bash should not be tracked");
    }

    // Env-based opt-out is tested by its own integration test (see
    // tests/disabled_env.rs) — racy with other concurrent tests when
    // exercised via the in-process env, so we use a child process /
    // separate binary for that scenario.

    #[tokio::test]
    async fn noop_hooks_default_no_op_for_before_after_run() {
        // Ensures the trait additions remain backward-compatible: NoopHooks
        // (which implements no methods) still compiles + returns Ok from
        // before_run / after_run.
        let h: Arc<dyn Hooks + Send + Sync> = Arc::new(NoopHooks);
        let ctx = RunCtx {
            session_id: "s",
            workspace_root: Path::new("/"),
            user_message: None,
            prompt_index: 0,
            cancel: CancellationToken::new(),
        };
        h.before_run(&ctx).await.unwrap();
        h.after_run(
            &ctx,
            &RunHookOutcome {
                turn_count: 0,
                input_tokens: 0,
                output_tokens: 0,
                success: true,
            },
        )
        .await
        .unwrap();
    }
}
