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
    Done(std::io::Result<std::process::ExitStatus>),
    Timeout,
    Cancelled,
}

/// Send `SIGKILL` to the child's entire process group (so subprocesses spawned
/// by the shell don't survive as orphans), then `wait()` to reap the zombie.
///
/// On non-Unix targets only the immediate child is killed (via `start_kill` /
/// `kill_on_drop`); subprocess containment requires Windows Job objects which
/// are not in scope here.
#[allow(unused_variables)]
#[allow(unsafe_code)] // libc::kill is a stable, well-defined FFI call; required to signal a process group (negative PID) which the safe API in std doesn't expose
async fn kill_process_tree(child_pid: Option<u32>, child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child_pid
        && let Ok(pid_i32) = i32::try_from(pid)
    {
        // SAFETY: `libc::kill` takes two integer arguments and returns an
        // integer; no pointer dereferences, no aliasing concerns. A negative
        // first argument signals the entire process group with PGID = our
        // child's PID (because we called `process_group(0)` on Command).
        // ESRCH on an already-dead group is fine; we ignore the return.
        unsafe {
            libc::kill(-pid_i32, libc::SIGKILL);
        }
    }
    // start_kill is a no-op if the process already exited, otherwise it
    // sends SIGKILL to the leader (redundant with the group kill above,
    // but harmless and serves as a fallback on non-Unix).
    let _ = child.start_kill();
    // Explicit wait so we don't leave a zombie. kill_on_drop sends SIGKILL
    // but does NOT wait — without this, we'd accumulate zombies in long-
    // running caliban sessions.
    let _ = child.wait().await;
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

        let mut shell = tokio::process::Command::new("/bin/sh");
        shell
            .arg("-c")
            .arg(&parsed.command)
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        // Put the child into its own process group so on timeout/cancel
        // we can SIGKILL the entire tree — not just the shell, but any
        // subprocesses it spawned (e.g., `sh -c "find / | xargs grep …"`
        // forks find AND grep AND xargs). Without process_group(0), the
        // shell dies and its descendants get reparented to init,
        // leaving orphans.
        #[cfg(unix)]
        shell.process_group(0);

        let mut child = shell.spawn().map_err(ToolError::execution)?;
        // Capture the PID now while we still have access to the Child.
        // On Unix the child's PID equals its PGID (because of process_group(0)).
        let child_pid = child.id();

        let stdout_pipe = child.stdout.take().expect("piped");
        let stderr_pipe = child.stderr.take().expect("piped");

        // Spawn I/O readers so they drain concurrently while we wait.
        let read_out = tokio::spawn(read_capped(stdout_pipe, STDOUT_CAP));
        let read_err = tokio::spawn(read_capped(stderr_pipe, STDERR_CAP));

        // IMPORTANT: do NOT spawn the wait into a separate task — that pattern
        // leaks the child on timeout/cancel because dropping a tokio JoinHandle
        // does not abort the task. Instead, keep `child.wait()` as a pinned
        // local future so we retain access to `child` after select! returns and
        // can clean up the process tree ourselves.
        let outcome = {
            let wait_fut = child.wait();
            tokio::pin!(wait_fut);
            tokio::select! {
                result = &mut wait_fut => RawOutcome::Done(result),
                () = tokio::time::sleep(timeout) => RawOutcome::Timeout,
                () = cx.cancel.cancelled() => RawOutcome::Cancelled,
            }
        };

        match outcome {
            RawOutcome::Done(status_result) => {
                let status = status_result.map_err(ToolError::execution)?;
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
                kill_process_tree(child_pid, &mut child).await;
                read_out.abort();
                read_err.abort();
                Err(ToolError::execution(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("command timed out after {timeout_secs}s"),
                )))
            }

            RawOutcome::Cancelled => {
                kill_process_tree(child_pid, &mut child).await;
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
            hooks: None,
            turn_index: 0,
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
            hooks: None,
            turn_index: 0,
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

    /// Regression test for the orphan-shell bug: when a shell command spawns
    /// subprocesses (a backgrounded `sleep`), cancellation must kill the
    /// entire process group, not just the shell leader.
    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_kills_subprocess_tree() {
        use std::process::Command;

        let tmp = TempDir::new().unwrap();
        let tool = BashTool::new(WorkspaceRoot::new(tmp.path()));
        let cancel = CancellationToken::new();

        // Use an unusual sleep duration as a marker so we can find this specific
        // sleep process via `ps`. PID-derived to avoid clashes with concurrent runs.
        let marker_seconds: u32 = 30000 + (std::process::id() % 1000);
        // Backgrounded sleep + `wait` forces the shell to fork a separate
        // /bin/sleep subprocess (rather than exec-replacing itself). If our
        // process-group kill works, both die together; if not, sleep orphans.
        let cmd = format!("/bin/sleep {marker_seconds} & wait");

        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel_clone.cancel();
        });
        let cx = ToolContext {
            tool_use_id: "t1".into(),
            cancel,
            hooks: None,
            turn_index: 0,
        };
        let err = tool.invoke(json!({"command": cmd}), cx).await.unwrap_err();
        assert!(matches!(err, ToolError::Cancelled));

        // Give the OS a beat to actually reap the now-killed processes.
        tokio::time::sleep(Duration::from_millis(300)).await;

        // ps lists all processes; look for our specific sleep duration. If the
        // process-group SIGKILL worked, no process should be carrying it.
        let needle = format!("sleep {marker_seconds}");
        let output = Command::new("ps")
            .arg("-eo")
            .arg("pid,command")
            .output()
            .expect("ps should run");
        let ps_text = String::from_utf8_lossy(&output.stdout);
        let surviving: Vec<&str> = ps_text
            .lines()
            .filter(|l| l.contains(&needle) && !l.contains("grep") && !l.contains("ps -eo"))
            .collect();
        assert!(
            surviving.is_empty(),
            "subprocess(es) survived cancellation:\n{}",
            surviving.join("\n"),
        );
    }
}
