//! `!cmd` shell escape — synthesizes a `Bash` tool call routed through the
//! same permission stack as a model-issued call.
//!
//! Entry: `is_shell_escape("!ls -l")` returns the trimmed command. The runner
//! dispatches the registered `Bash` tool (gated by `Hooks::before_tool`) and
//! returns the captured stdout/stderr plus exit metadata for inline render.

use std::sync::Arc;

use caliban_agent_core::{
    ContentBlock, HookDecision, Hooks, ToolContext, ToolCtx, ToolError, ToolRegistry,
};
use tokio_util::sync::CancellationToken;

/// Decision after attempting to escape a `!`-prefixed line.
///
/// `Run` carries the command the user typed (without the leading `!`).
/// `NotShellEscape` indicates the input wasn't a shell escape — caller should
/// route normally.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ShellEscapeIntent {
    /// User typed `!cmd` — run `cmd` as a Bash invocation.
    Run(String),
    /// Input didn't start with `!` or had no command after it.
    NotShellEscape,
}

/// Inspect the trimmed input buffer for a shell-escape trigger. The leading
/// `!` must be at column 0 (whole-buffer start) — `!` mid-buffer (e.g.
/// `echo !foo`) does NOT trigger. Both `\n` and `\r\n` line boundaries
/// invalidate the trigger to preserve multi-line behavior.
#[must_use]
pub(crate) fn parse_shell_escape(buffer: &str) -> ShellEscapeIntent {
    let trimmed = buffer.trim_start_matches([' ', '\t']);
    if !trimmed.starts_with('!') {
        return ShellEscapeIntent::NotShellEscape;
    }
    // Only single-line shell escapes; reject if the buffer contains a newline
    // before the `!` was placed at col 0.
    if trimmed.contains('\n') {
        return ShellEscapeIntent::NotShellEscape;
    }
    let cmd = trimmed[1..].trim().to_string();
    if cmd.is_empty() {
        return ShellEscapeIntent::NotShellEscape;
    }
    ShellEscapeIntent::Run(cmd)
}

/// Outcome of running a shell escape — drives the inline render in the
/// transcript.
#[derive(Debug)]
pub(crate) struct ShellEscapeOutcome {
    /// Original command (without the `!`). Retained for display + telemetry.
    #[allow(dead_code, reason = "command is consumed by the TUI renderer")]
    pub(crate) command: String,
    /// Combined stdout+stderr captured from the Bash tool.
    pub(crate) output: String,
    /// `true` if the Bash tool reported an error (non-zero exit, etc.).
    pub(crate) is_error: bool,
    /// `true` if the run was short-circuited by the permission hook.
    pub(crate) denied: bool,
    /// Optional denial / error message.
    pub(crate) message: Option<String>,
}

/// Run a synthesized Bash invocation gated by `hooks` (which is expected to
/// wrap `PermissionsHook`) and return the structured outcome. The synthesized
/// call uses a stable but obviously-synthetic `tool_use_id`.
///
/// # Errors
/// All paths return `Ok(ShellEscapeOutcome)`; permission denials are reported
/// via `denied: true` in the outcome rather than as `Err`. Genuine internal
/// errors (Bash tool not registered, `ToolError`) are reported with
/// `is_error: true`.
pub(crate) async fn run_shell_escape(
    command: String,
    registry: &ToolRegistry,
    hooks: Arc<dyn Hooks + Send + Sync>,
    cancel: CancellationToken,
) -> ShellEscapeOutcome {
    let input = serde_json::json!({ "command": command });
    let tool_use_id = format!("shell-escape-{}", uuid_like_id(&command));

    // Gate via the same hook chain the agent uses for tool dispatch.
    let ctx = ToolCtx {
        turn_index: 0,
        tool_use_id: &tool_use_id,
        tool_name: "Bash",
        input: &input,
    };
    match hooks.before_tool(&ctx).await {
        // Allow + UpdatedInput both proceed. The shell-escape path is a
        // user-driven, single-shot run; rewriting the command via hooks is
        // not supported in v1.
        Ok(HookDecision::Allow | HookDecision::UpdatedInput(_)) => {}
        Ok(HookDecision::Deny(msg)) => {
            return ShellEscapeOutcome {
                command,
                output: String::new(),
                is_error: true,
                denied: true,
                message: Some(msg),
            };
        }
        Err(e) => {
            return ShellEscapeOutcome {
                command,
                output: String::new(),
                is_error: true,
                denied: false,
                message: Some(format!("hook error: {e}")),
            };
        }
    }

    let Some(tool) = registry.get("Bash") else {
        return ShellEscapeOutcome {
            command,
            output: String::new(),
            is_error: true,
            denied: false,
            message: Some("Bash tool not registered".into()),
        };
    };

    let cx = ToolContext {
        tool_use_id: tool_use_id.clone(),
        cancel,
        hooks: Some(Arc::clone(&hooks)),
        turn_index: 0,
    };
    let result = tool.invoke(input.clone(), cx).await;
    match result {
        Ok(blocks) => {
            let text = collect_text(&blocks);
            ShellEscapeOutcome {
                command,
                output: text,
                is_error: false,
                denied: false,
                message: None,
            }
        }
        Err(ToolError::Cancelled) => ShellEscapeOutcome {
            command,
            output: String::new(),
            is_error: true,
            denied: false,
            message: Some("cancelled".into()),
        },
        Err(e) => ShellEscapeOutcome {
            command,
            output: String::new(),
            is_error: true,
            denied: false,
            message: Some(format!("error: {e}")),
        },
    }
}

/// Concatenate text content blocks into one string for inline render.
fn collect_text(blocks: &[ContentBlock]) -> String {
    let mut out = String::new();
    for b in blocks {
        if let ContentBlock::Text(t) = b {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&t.text);
        }
    }
    out
}

/// Cheap pseudo-id for the synthesized `tool_use_id` — first 8 chars of the
/// command's lowercase hash-equivalent. Doesn't need to be cryptographic.
fn uuid_like_id(seed: &str) -> String {
    let n: u64 = seed
        .bytes()
        .fold(0u64, |a, b| a.wrapping_mul(31).wrapping_add(u64::from(b)));
    format!("{n:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use caliban_agent_core::{
        AskHandler, NoopHooks, PermissionsHook, Tool, ToolCtx,
        permissions::{Action, Rule, default_rules},
    };
    use serde_json::Value;
    use std::sync::Mutex;

    #[test]
    fn empty_input_is_not_shell_escape() {
        assert_eq!(parse_shell_escape(""), ShellEscapeIntent::NotShellEscape);
        assert_eq!(parse_shell_escape("!"), ShellEscapeIntent::NotShellEscape);
        assert_eq!(parse_shell_escape("   "), ShellEscapeIntent::NotShellEscape);
    }

    #[test]
    fn bare_bang_with_command_triggers() {
        assert_eq!(
            parse_shell_escape("!ls"),
            ShellEscapeIntent::Run("ls".into())
        );
        assert_eq!(
            parse_shell_escape("!git status"),
            ShellEscapeIntent::Run("git status".into())
        );
    }

    #[test]
    fn whitespace_before_bang_is_tolerated() {
        // Leading whitespace at column 0 is forgiven (mirrors how shells handle
        // accidental indents).
        assert_eq!(
            parse_shell_escape("  !ls"),
            ShellEscapeIntent::Run("ls".into())
        );
    }

    #[test]
    fn bang_not_at_start_is_not_shell_escape() {
        assert_eq!(
            parse_shell_escape("echo !foo"),
            ShellEscapeIntent::NotShellEscape
        );
    }

    #[test]
    fn multiline_buffer_disables_shell_escape() {
        // Multi-line prompts must not be hijacked by a `!` that happens to be
        // at the start.
        assert_eq!(
            parse_shell_escape("!ls\nnope"),
            ShellEscapeIntent::NotShellEscape
        );
    }

    // ---------- runner tests via stub tools / hooks ----------

    /// Echo tool stand-in for `Bash` (no actual subprocess). Records input
    /// for assertions.
    #[derive(Debug)]
    struct EchoBash {
        seen: Mutex<Vec<String>>,
        out: String,
    }

    #[async_trait]
    impl Tool for EchoBash {
        fn name(&self) -> &'static str {
            "Bash"
        }
        fn description(&self) -> &'static str {
            "echo bash"
        }
        fn input_schema(&self) -> &Value {
            // Static schema, leaked for 'static lifetime in tests.
            static SCHEMA: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
            SCHEMA.get_or_init(|| serde_json::json!({"type":"object"}))
        }
        async fn invoke(
            &self,
            input: Value,
            _cx: ToolContext,
        ) -> Result<Vec<ContentBlock>, ToolError> {
            let cmd = input
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            self.seen.lock().unwrap().push(cmd);
            Ok(vec![ContentBlock::Text(caliban_provider::TextBlock {
                text: self.out.clone(),
                cache_control: None,
            })])
        }
    }

    /// Auto-allow `AskHandler` so tests can drive Bash without an interactive
    /// loop.
    #[derive(Debug)]
    struct AlwaysAllow;
    #[async_trait]
    impl AskHandler for AlwaysAllow {
        async fn prompt(&self, _ctx: &ToolCtx<'_>) -> HookDecision {
            HookDecision::Allow
        }
    }

    fn registry_with_bash(out: &str) -> (ToolRegistry, Arc<EchoBash>) {
        let mut r = ToolRegistry::new();
        let tool = Arc::new(EchoBash {
            seen: Mutex::new(Vec::new()),
            out: out.into(),
        });
        r.register(Arc::clone(&tool) as Arc<dyn Tool>);
        (r, tool)
    }

    #[tokio::test]
    async fn allow_runs_command_and_captures_output() {
        let (registry, _tool) = registry_with_bash("hello world");
        let hook: Arc<dyn Hooks + Send + Sync> = Arc::new(PermissionsHook::new(
            // CLI rule: allow Bash unconditionally.
            {
                let mut r = vec![Rule {
                    tool: "Bash".into(),
                    action: Action::Allow,
                    comment: None,
                }];
                r.extend(default_rules());
                r
            },
            Arc::new(AlwaysAllow),
            Arc::new(NoopHooks),
        ));
        let cancel = CancellationToken::new();
        let outcome = run_shell_escape("ls".into(), &registry, hook, cancel).await;
        assert!(!outcome.is_error);
        assert!(!outcome.denied);
        assert_eq!(outcome.output, "hello world");
    }

    #[tokio::test]
    async fn deny_rule_short_circuits() {
        let (registry, tool) = registry_with_bash("never runs");
        let hook: Arc<dyn Hooks + Send + Sync> = Arc::new(PermissionsHook::new(
            // Deny Bash:rm * specifically.
            {
                let mut r = vec![Rule {
                    tool: "Bash:rm *".into(),
                    action: Action::Deny,
                    comment: None,
                }];
                r.extend(default_rules());
                r
            },
            Arc::new(AlwaysAllow),
            Arc::new(NoopHooks),
        ));
        let cancel = CancellationToken::new();
        let outcome = run_shell_escape("rm -rf /".into(), &registry, hook, cancel).await;
        assert!(outcome.denied);
        assert!(outcome.is_error);
        // The tool must not have been invoked.
        assert!(tool.seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn ask_with_auto_allow_runs_command() {
        // Ensures the default Bash → Ask rule routes through the AskHandler
        // (here, AlwaysAllow), letting the synthesized command run.
        let (registry, _tool) = registry_with_bash("ok");
        let hook: Arc<dyn Hooks + Send + Sync> = Arc::new(PermissionsHook::new(
            default_rules(),
            Arc::new(AlwaysAllow),
            Arc::new(NoopHooks),
        ));
        let cancel = CancellationToken::new();
        let outcome = run_shell_escape("ls".into(), &registry, hook, cancel).await;
        assert!(!outcome.denied);
        assert!(!outcome.is_error);
    }

    #[tokio::test]
    async fn missing_bash_tool_is_reported_as_error() {
        let registry = ToolRegistry::new();
        let hook: Arc<dyn Hooks + Send + Sync> = Arc::new(PermissionsHook::new(
            {
                let mut r = vec![Rule {
                    tool: "Bash".into(),
                    action: Action::Allow,
                    comment: None,
                }];
                r.extend(default_rules());
                r
            },
            Arc::new(AlwaysAllow),
            Arc::new(NoopHooks),
        ));
        let cancel = CancellationToken::new();
        let outcome = run_shell_escape("ls".into(), &registry, hook, cancel).await;
        assert!(outcome.is_error);
        assert!(outcome.message.as_deref().unwrap_or("").contains("Bash"));
    }
}
