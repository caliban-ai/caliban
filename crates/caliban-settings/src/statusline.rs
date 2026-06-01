//! Custom statusline runner — spawns a user-configured shell command
//! after each turn end and caches its first stdout line for the
//! renderer to prefix to the existing statusline.
//!
//! Schema-compatible with claude-code's documented contract: a JSON
//! blob on stdin describing the active model / cost / permission mode /
//! effort / workspace / session / turn count, one stdout line out.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt as _;
use tokio::process::Command;
use tokio::sync::Mutex;

/// User-configured statusline command.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StatuslineConfig {
    /// The shell command to invoke. Tokenized on whitespace; first
    /// token is the program, the rest are positional args.
    pub command: String,
    /// Hard timeout per invocation, milliseconds. Default 200.
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u32,
    /// Spaces of leading padding the renderer should add between the
    /// custom segment and the next built-in segment.
    #[serde(default = "default_padding")]
    pub padding: u8,
}

fn default_timeout_ms() -> u32 {
    200
}

fn default_padding() -> u8 {
    1
}

/// JSON context the runner writes to the command's stdin before
/// waiting for output. Matches claude-code's published shape.
#[derive(Debug, Clone, Default, Serialize)]
pub struct StatuslineContext {
    /// Active model id.
    pub model: String,
    /// Cumulative cost in USD, formatted as a string with 4 decimal
    /// places.
    pub cost_usd: String,
    /// Current permission mode (`default`, `accept_edits`, etc.).
    pub permission_mode: String,
    /// Current reasoning-effort level (`low`/`medium`/`high`/`max`/`auto`).
    pub effort: String,
    /// Workspace root path as a display string.
    pub workspace_root: String,
    /// Session id (empty when ephemeral).
    pub session_id: String,
    /// Number of agentic turns completed so far.
    pub turn_count: u32,
}

/// Maximum number of consecutive timeouts before the runner gives up
/// for the rest of the session.
const MAX_CONSECUTIVE_TIMEOUTS: u8 = 3;
/// Cap the rendered line to ~one terminal row.
const MAX_LINE_LEN: usize = 120;

/// Statusline runner — wraps the configured command, caches its last
/// non-timeout output, and disables itself after a streak of timeouts.
pub struct StatuslineRunner {
    config: StatuslineConfig,
    cache: Mutex<Option<(Instant, String)>>,
    consecutive_timeouts: Mutex<u8>,
    /// One-shot latch: warn at most once per misconfiguration. Reset
    /// the next time a spawn succeeds, so a user who fixes the script
    /// path and then breaks it again sees a fresh warning.
    spawn_warned: Mutex<bool>,
}

impl std::fmt::Debug for StatuslineRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StatuslineRunner")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl StatuslineRunner {
    /// New runner; nothing happens until `refresh` is called.
    #[must_use]
    pub fn new(config: StatuslineConfig) -> Self {
        Self {
            config,
            cache: Mutex::new(None),
            consecutive_timeouts: Mutex::new(0),
            spawn_warned: Mutex::new(false),
        }
    }

    /// Run the command, pipe `ctx` as JSON to stdin, wait up to
    /// `timeout_ms` for the first stdout line. Returns the cached
    /// previous value on timeout / failure. After
    /// [`MAX_CONSECUTIVE_TIMEOUTS`] failures the runner disables
    /// itself for the rest of the process and `refresh` short-circuits
    /// to the cached value.
    pub async fn refresh(&self, ctx: StatuslineContext) -> String {
        if *self.consecutive_timeouts.lock().await >= MAX_CONSECUTIVE_TIMEOUTS {
            return self.cached().await;
        }
        let payload = serde_json::to_string(&ctx).unwrap_or_default();
        let mut parts = self.config.command.split_whitespace();
        let Some(prog) = parts.next() else {
            return self.cached().await;
        };
        let args: Vec<&str> = parts.collect();
        let mut cmd = Command::new(prog);
        cmd.args(&args)
            .kill_on_drop(true)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        let mut child = match cmd.spawn() {
            Ok(c) => {
                *self.spawn_warned.lock().await = false;
                c
            }
            Err(e) => {
                let mut warned = self.spawn_warned.lock().await;
                if !*warned {
                    tracing::warn!(
                        target: "caliban::statusline",
                        error = %e,
                        program = %prog,
                        "failed to spawn statusline command; check `statusLine.command` in settings.toml",
                    );
                    *warned = true;
                }
                return self.cached().await;
            }
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(payload.as_bytes()).await;
            let _ = stdin.shutdown().await;
        }
        let timeout = Duration::from_millis(u64::from(self.config.timeout_ms));
        let result = tokio::time::timeout(timeout, child.wait_with_output()).await;
        if let Ok(Ok(o)) = result {
            *self.consecutive_timeouts.lock().await = 0;
            let line: String = String::from_utf8_lossy(&o.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(MAX_LINE_LEN)
                .collect();
            *self.cache.lock().await = Some((Instant::now(), line.clone()));
            line
        } else {
            let mut t = self.consecutive_timeouts.lock().await;
            *t += 1;
            if *t == MAX_CONSECUTIVE_TIMEOUTS {
                tracing::warn!(
                    target: "caliban::statusline",
                    "statusline script timed out {MAX_CONSECUTIVE_TIMEOUTS} times; disabling for session",
                );
            }
            self.cached().await
        }
    }

    async fn cached(&self) -> String {
        self.cache
            .lock()
            .await
            .as_ref()
            .map(|(_, s)| s.clone())
            .unwrap_or_default()
    }

    /// Test-only: seed the cache directly so refresh-fallback paths
    /// can be exercised without spawning a real process.
    #[cfg(test)]
    pub async fn set_cached_for_test(&self, s: String) {
        *self.cache.lock().await = Some((Instant::now(), s));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    #[tokio::test]
    async fn runner_returns_script_stdout() {
        let dir = tempdir().unwrap();
        let script_path = dir.path().join("hello.sh");
        std::fs::write(&script_path, "#!/bin/sh\necho hello world").unwrap();
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let runner = StatuslineRunner::new(StatuslineConfig {
            command: script_path.to_string_lossy().to_string(),
            timeout_ms: 1_000,
            padding: 1,
        });
        let out = runner.refresh(StatuslineContext::default()).await;
        assert_eq!(out.trim(), "hello world");
    }

    #[tokio::test]
    async fn runner_returns_cached_when_spawn_fails() {
        // Point at a path that definitely doesn't exist so spawn returns
        // Err. The runner must fall back to the cached value (here:
        // empty seed cleared) rather than crashing, and on a real
        // session the WARN side-effect tells the operator their
        // `statusLine.command` is misconfigured.
        let runner = StatuslineRunner::new(StatuslineConfig {
            command: "/this/path/does/not/exist/statusline-bin".into(),
            timeout_ms: 200,
            padding: 1,
        });
        runner.set_cached_for_test("prior".into()).await;
        let out = runner.refresh(StatuslineContext::default()).await;
        assert_eq!(out, "prior");
        // Refreshing again must not panic and must still return cached;
        // the WARN latch suppresses additional log noise after the
        // first spawn failure.
        let out2 = runner.refresh(StatuslineContext::default()).await;
        assert_eq!(out2, "prior");
    }

    #[tokio::test]
    async fn runner_returns_cached_on_timeout() {
        // Use a script that actually sleeps so the timeout fires. The
        // `command` field is whitespace-tokenized so we can't pass a
        // shell-escaped one-liner; write a script to a tempfile.
        let dir = tempdir().unwrap();
        let script = dir.path().join("slow.sh");
        std::fs::write(&script, "#!/bin/sh\nsleep 2\necho too-slow\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let runner = StatuslineRunner::new(StatuslineConfig {
            command: script.to_string_lossy().to_string(),
            timeout_ms: 50,
            padding: 1,
        });
        runner.set_cached_for_test("cached".into()).await;
        let out = runner.refresh(StatuslineContext::default()).await;
        assert_eq!(out.trim(), "cached");
    }
}
