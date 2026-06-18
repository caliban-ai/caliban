//! Append-only JSONL decision log + a `Hooks` impl that writes to it.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use serde::Serialize;

use crate::error::Result;
use crate::hooks::{HookDecision, Hooks, ToolCtx};

/// Return the canonical path for the permission-decisions log file.
///
/// Falls back from `$XDG_STATE_HOME/caliban/` to `$XDG_DATA_HOME/caliban/`
/// when the state dir is unavailable (macOS).
pub fn decision_log_path() -> Option<PathBuf> {
    let base = dirs::state_dir().or_else(dirs::data_local_dir)?;
    let dir = base.join("caliban");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("permission-decisions.jsonl"))
}

#[derive(Debug, Serialize)]
struct LogLine<'a> {
    ts: String,
    session_id: &'a str,
    turn_index: u32,
    tool_use_id: &'a str,
    tool_name: &'a str,
    input_excerpt: String,
    action: &'a str,
    matched_rule: Option<MatchedRule<'a>>,
}

#[derive(Debug, Serialize)]
struct MatchedRule<'a> {
    pattern: &'a str,
    action: &'a str,
}

/// Append-only writer for the JSONL decision log.
///
/// Writes one JSON line per tool-call decision. When the file exceeds
/// `max_bytes`, the current file is renamed to a date-stamped variant, gzip-
/// compressed, and a fresh file is opened.
pub struct DecisionLogWriter {
    file: Mutex<Option<std::fs::File>>,
    path: PathBuf,
    /// Maximum file size before rotation (default: 100 MiB).
    pub max_bytes: u64,
    session_id: String,
}

impl DecisionLogWriter {
    /// Open (or create) the log file at `path` for append.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be opened for writing.
    pub fn open(path: PathBuf, session_id: String) -> std::io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        Ok(Self {
            file: Mutex::new(Some(file)),
            path,
            max_bytes: 100 * 1024 * 1024,
            session_id,
        })
    }

    /// Append a decision line to the log.
    ///
    /// `matched` carries `(pattern, action)` from the matched rule when
    /// available. Rotation fires when the file exceeds `max_bytes`.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned (a previous panic while
    /// holding the lock). This is expected to be non-recoverable.
    pub fn record(&self, ctx: &ToolCtx<'_>, action: &str, matched: Option<(&str, &str)>) {
        let excerpt = sanitize_excerpt(&ctx.input.to_string(), 256);
        let line = LogLine {
            ts: chrono::Utc::now().to_rfc3339(),
            session_id: &self.session_id,
            turn_index: ctx.turn_index,
            tool_use_id: ctx.tool_use_id,
            tool_name: ctx.tool_name,
            input_excerpt: excerpt,
            action,
            matched_rule: matched.map(|(p, a)| MatchedRule {
                pattern: p,
                action: a,
            }),
        };
        if let Ok(s) = serde_json::to_string(&line) {
            let mut guard = self.file.lock().expect("decision log mutex poisoned");
            if let Some(f) = guard.as_mut() {
                let _ = writeln!(f, "{s}");
                if let Ok(meta) = std::fs::metadata(&self.path)
                    && meta.len() > self.max_bytes
                {
                    // Close current handle, rename + gzip, reopen.
                    *guard = None;
                    let _ = rotate(&self.path);
                    if let Ok(nf) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&self.path)
                    {
                        *guard = Some(nf);
                    }
                }
            }
        }
    }
}

fn sanitize_excerpt(s: &str, n: usize) -> String {
    let head: String = s.chars().take(n).collect();
    head.replace(['\n', '\r'], " ")
}

fn rotate(path: &Path) -> std::io::Result<()> {
    let date = chrono::Utc::now().format("%Y-%m-%d");
    let renamed = path.with_file_name(format!("permission-decisions-{date}.jsonl"));
    std::fs::rename(path, &renamed)?;
    // gzip-in-place
    let gz_path = renamed.with_extension("jsonl.gz");
    let input = std::fs::read(&renamed)?;
    let gz = std::fs::File::create(&gz_path)?;
    let mut enc = flate2::write::GzEncoder::new(gz, flate2::Compression::default());
    enc.write_all(&input)?;
    enc.finish()?;
    std::fs::remove_file(&renamed)?;
    Ok(())
}

/// A [`Hooks`] wrapper that intercepts `before_tool` to record each
/// allow/deny/ask decision into the JSONL log, then delegates to `inner`.
pub struct DecisionRecorder {
    /// The shared log writer; may be shared across multiple recorders.
    pub writer: std::sync::Arc<DecisionLogWriter>,
    /// The inner hook chain that makes the actual allow/deny decision.
    pub inner: std::sync::Arc<dyn Hooks>,
    /// When `false` the recorder is a transparent pass-through (no writes).
    pub enabled: bool,
}

#[async_trait]
impl Hooks for DecisionRecorder {
    async fn before_tool(&self, ctx: &ToolCtx<'_>) -> Result<HookDecision> {
        let d = self.inner.before_tool(ctx).await?;
        if self.enabled {
            let action_str = match &d {
                HookDecision::Allow | HookDecision::UpdatedInput(_) => "allow",
                HookDecision::Deny(_) => "deny",
            };
            self.writer.record(ctx, action_str, None);
        }
        Ok(d)
    }

    // Delegate all other Hooks methods to inner.
    async fn after_tool(
        &self,
        ctx: &ToolCtx<'_>,
        result: &std::result::Result<Vec<caliban_provider::ContentBlock>, crate::tool::ToolError>,
    ) -> Result<()> {
        self.inner.after_tool(ctx, result).await
    }

    async fn before_run(&self, ctx: &crate::hooks::RunCtx<'_>) -> Result<()> {
        self.inner.before_run(ctx).await
    }

    async fn after_run(
        &self,
        ctx: &crate::hooks::RunCtx<'_>,
        outcome: &crate::hooks::RunHookOutcome,
    ) -> Result<()> {
        self.inner.after_run(ctx, outcome).await
    }

    async fn after_run_failure(
        &self,
        ctx: &crate::hooks::RunCtx<'_>,
        outcome: &crate::hooks::RunHookOutcome,
    ) -> Result<()> {
        self.inner.after_run_failure(ctx, outcome).await
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

    async fn after_turn_failure(
        &self,
        ctx: &crate::hooks::TurnCtx<'_>,
        outcome: &crate::TurnOutcome,
    ) -> Result<()> {
        self.inner.after_turn_failure(ctx, outcome).await
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

    async fn permission_request(&self, ctx: &crate::hooks::PermCtx<'_>) -> Result<()> {
        self.inner.permission_request(ctx).await
    }

    async fn permission_denied(&self, ctx: &crate::hooks::PermCtx<'_>) -> Result<()> {
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

    #[test]
    fn writer_appends_and_rotates_at_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        let mut w = DecisionLogWriter::open(path.clone(), "S".into()).unwrap();
        w.max_bytes = 200; // tiny cap
        let input = serde_json::json!({"command": "echo hi"});
        let ctx = ToolCtx {
            turn_index: 0,
            tool_use_id: "t",
            tool_name: "Bash",
            input: &input,
            is_read_only: false,
        };
        for _ in 0..30 {
            w.record(&ctx, "allow", None);
        }
        // After many writes, rotation should have happened.
        let entries: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert!(
            entries.iter().any(|e| {
                let n = e
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .to_string();
                n.contains("permission-decisions-") && n.to_lowercase().ends_with(".gz")
            }),
            "expected at least one rotated .gz; got: {entries:?}"
        );
    }

    #[tokio::test]
    async fn decision_recorder_writes_allow_line() {
        use crate::NoopHooks;
        use crate::permissions::{Action, NonInteractiveAskHandler, PermissionsHook, Rule};
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        let writer = Arc::new(DecisionLogWriter::open(path.clone(), "SID".into()).unwrap());

        let mut rules = vec![Rule {
            tool: "Read".into(),
            action: Action::Allow,
            comment: None,
            reason: None,
            expires_at: None,
        }];
        rules.extend(crate::permissions::default_rules());
        let inner: Arc<dyn Hooks> = Arc::new(PermissionsHook::new(
            rules,
            Arc::new(NonInteractiveAskHandler { auto_allow: false }),
            Arc::new(NoopHooks),
        ));

        let recorder = DecisionRecorder {
            writer,
            inner,
            enabled: true,
        };

        let input = serde_json::json!({"file_path": "/etc/hosts"});
        let ctx = ToolCtx {
            turn_index: 0,
            tool_use_id: "t1",
            tool_name: "Read",
            input: &input,
            is_read_only: true,
        };
        let d = recorder.before_tool(&ctx).await.unwrap();
        assert!(matches!(d, HookDecision::Allow));

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(
            body.contains(r#""action":"allow""#),
            "expected JSONL line; got: {body}"
        );
        assert!(body.contains(r#""tool_name":"Read""#));
        assert!(body.contains(r#""session_id":"SID""#));
    }
}
