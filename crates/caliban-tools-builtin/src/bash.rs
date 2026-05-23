//! Bash tool — spawn a shell command and capture stdout + stderr.

use std::process::Stdio;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use async_trait::async_trait;
use caliban_agent_core::{Tool, ToolContext, ToolError};
use caliban_provider::{ContentBlock, TextBlock};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

use crate::workspace::WorkspaceRoot;

const STDOUT_CAP: usize = 30 * 1024;
const STDERR_CAP: usize = 30 * 1024;

/// Shell command execution tool.
#[derive(Debug)]
pub struct BashTool {
    root: Arc<WorkspaceRoot>,
    schema: OnceLock<Value>,
}

impl BashTool {
    /// Construct a Bash tool using the given workspace root.
    #[must_use]
    pub fn new(root: WorkspaceRoot) -> Self {
        Self {
            root: Arc::new(root),
            schema: OnceLock::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct BashInput {
    command: String,
    #[serde(default)]
    timeout_seconds: Option<u64>,
    #[serde(default)]
    cwd: Option<String>,
}

/// Read up to `cap` bytes from an async reader, returning lossy UTF-8.
async fn read_capped<R: AsyncReadExt + Unpin>(mut reader: R, cap: usize) -> String {
    let mut buf = Vec::with_capacity(cap);
    let mut chunk = [0u8; 4096];
    while buf.len() < cap {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let take = n.min(cap - buf.len());
                buf.extend_from_slice(&chunk[..take]);
            }
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// Outcome of the `tokio::select!` in [`BashTool::invoke`].
enum RawOutcome {
    Done(std::process::ExitStatus),
    Timeout,
    Cancelled,
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &'static str {
        "Bash"
    }

    fn description(&self) -> &'static str {
        "Run a shell command via /bin/sh -c. Captures stdout and stderr. Enforces a timeout. Returns exit code, stdout, and stderr."
    }

    fn input_schema(&self) -> &Value {
        self.schema.get_or_init(|| json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Shell command to run"
                },
                "timeout_seconds": {
                    "type": "integer",
                    "description": "Maximum seconds to wait before killing the process (default 60, min 1, max 600)",
                    "minimum": 1,
                    "maximum": 600
                },
                "cwd": {
                    "type": "string",
                    "description": "Working directory for the command, relative to workspace root (default: workspace root)"
                }
            },
            "required": ["command"]
        }))
    }

    async fn invoke(&self, input: Value, cx: ToolContext) -> Result<Vec<ContentBlock>, ToolError> {
        let parsed: BashInput =
            serde_json::from_value(input).map_err(|e| ToolError::invalid_input(format!("{e}")))?;

        let timeout_secs = parsed.timeout_seconds.unwrap_or(60).clamp(1, 600);
        let timeout = Duration::from_secs(timeout_secs);

        let cwd = match parsed.cwd {
            Some(ref c) => self.root.resolve(c)?,
            None => self.root.root().to_path_buf(),
        };

        let mut child = tokio::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&parsed.command)
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(ToolError::execution)?;

        let stdout_pipe = child.stdout.take().expect("piped");
        let stderr_pipe = child.stderr.take().expect("piped");

        // Spawn I/O readers so they drain concurrently while we wait.
        let read_out = tokio::spawn(read_capped(stdout_pipe, STDOUT_CAP));
        let read_err = tokio::spawn(read_capped(stderr_pipe, STDERR_CAP));

        // Spawn the wait as a task that owns `child`.
        // kill_on_drop(true) means dropping `child` (when the task is aborted)
        // sends SIGKILL to the process automatically.
        let wait_task: tokio::task::JoinHandle<std::io::Result<std::process::ExitStatus>> =
            tokio::spawn(async move { child.wait().await });

        let raw = tokio::select! {
            result = wait_task => {
                let status = result
                    .map_err(|e| ToolError::execution(std::io::Error::other(e.to_string())))?
                    .map_err(ToolError::execution)?;
                RawOutcome::Done(status)
            }
            () = tokio::time::sleep(timeout) => RawOutcome::Timeout,
            () = cx.cancel.cancelled() => RawOutcome::Cancelled,
        };

        match raw {
            RawOutcome::Done(status) => {
                let stdout_str = read_out.await.unwrap_or_default();
                let stderr_str = read_err.await.unwrap_or_default();
                let exit_code_num = status.code();
                let exit_code_display = exit_code_num
                    .map_or_else(|| "(killed by signal)".to_string(), |c| c.to_string());
                let stdout_section = if stdout_str.is_empty() {
                    "(empty)".to_string()
                } else {
                    stdout_str
                };
                let stderr_section = if stderr_str.is_empty() {
                    "(empty)".to_string()
                } else {
                    stderr_str
                };

                let text = format!(
                    "→ Bash command: {}\n→ Exit code: {}\n→ Stdout:\n{}\n→ Stderr:\n{}",
                    parsed.command, exit_code_display, stdout_section, stderr_section,
                );

                // Non-zero exit (or killed by signal) → ToolError so the agent flags
                // is_error=true. The full captured output is in the error message so
                // the model still sees stdout/stderr/exit-code context.
                if exit_code_num != Some(0) {
                    return Err(ToolError::execution(std::io::Error::other(text)));
                }

                Ok(vec![ContentBlock::Text(TextBlock {
                    text,
                    cache_control: None,
                })])
            }

            RawOutcome::Timeout => {
                // Aborting the wait task drops `child`, which triggers kill_on_drop.
                read_out.abort();
                read_err.abort();
                Err(ToolError::execution(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("command timed out after {timeout_secs}s"),
                )))
            }

            RawOutcome::Cancelled => {
                read_out.abort();
                read_err.abort();
                Err(ToolError::Cancelled)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    use super::*;

    fn ctx() -> ToolContext {
        ToolContext {
            tool_use_id: "t1".into(),
            cancel: CancellationToken::new(),
        }
    }

    #[tokio::test]
    async fn echo_succeeds() {
        let tmp = TempDir::new().unwrap();
        let tool = BashTool::new(WorkspaceRoot::new(tmp.path()));
        let out = tool
            .invoke(json!({"command": "echo hi"}), ctx())
            .await
            .unwrap();
        let ContentBlock::Text(t) = &out[0] else {
            panic!("expected Text block")
        };
        assert!(t.text.contains("hi"), "output: {}", t.text);
        assert!(t.text.contains("Exit code: 0"), "output: {}", t.text);
    }

    #[tokio::test]
    async fn nonzero_exit_returns_tool_error() {
        let tmp = TempDir::new().unwrap();
        let tool = BashTool::new(WorkspaceRoot::new(tmp.path()));
        let err = tool
            .invoke(json!({"command": "exit 7"}), ctx())
            .await
            .unwrap_err();
        // The full output (including "Exit code: 7") is preserved in the error
        // message so the model can see what failed.
        let msg = format!("{err}");
        assert!(
            matches!(err, ToolError::Execution(_)),
            "wrong variant: {err:?}"
        );
        assert!(msg.contains("Exit code: 7"), "msg: {msg}");
    }

    #[tokio::test]
    async fn command_not_found_returns_tool_error() {
        let tmp = TempDir::new().unwrap();
        let tool = BashTool::new(WorkspaceRoot::new(tmp.path()));
        let err = tool
            .invoke(
                json!({"command": "this-command-definitely-does-not-exist-zzz"}),
                ctx(),
            )
            .await
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            matches!(err, ToolError::Execution(_)),
            "wrong variant: {err:?}"
        );
        // /bin/sh returns 127 for command not found.
        assert!(msg.contains("Exit code: 127"), "msg: {msg}");
    }

    #[tokio::test]
    async fn timeout_fires() {
        let tmp = TempDir::new().unwrap();
        let tool = BashTool::new(WorkspaceRoot::new(tmp.path()));
        let start = std::time::Instant::now();
        let err = tool
            .invoke(json!({"command": "sleep 5", "timeout_seconds": 1}), ctx())
            .await
            .unwrap_err();
        assert!(
            start.elapsed().as_secs() < 3,
            "timeout took too long: {:?}",
            start.elapsed()
        );
        let s = format!("{err}");
        assert!(
            s.to_lowercase().contains("timed out") || s.to_lowercase().contains("timeout"),
            "error message: {s}"
        );
    }

    #[tokio::test]
    async fn cancellation_kills_process() {
        let tmp = TempDir::new().unwrap();
        let tool = BashTool::new(WorkspaceRoot::new(tmp.path()));
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel_clone.cancel();
        });
        let cx = ToolContext {
            tool_use_id: "t1".into(),
            cancel,
        };
        let start = std::time::Instant::now();
        let err = tool
            .invoke(json!({"command": "sleep 30"}), cx)
            .await
            .unwrap_err();
        assert!(
            start.elapsed().as_millis() < 1000,
            "cancellation took too long: {:?}",
            start.elapsed()
        );
        assert!(
            matches!(err, ToolError::Cancelled),
            "expected Cancelled, got: {err}"
        );
    }
}
