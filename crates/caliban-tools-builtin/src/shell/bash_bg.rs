//! Background-Bash registry + `BashOutput` + `KillShell` tools.
//!
//! When the Bash tool is invoked with `background: true` the command is
//! enrolled in a shared registry and the call returns immediately with the
//! shell id. The registry tracks the child process, an output ring-buffer
//! (capped, defaults to 5 GiB), and the exit status. Two companion tools tail
//! the output and terminate the job.
//!
//! The registry is a `OnceLock<Arc<BashBgRegistry>>` singleton; tests build
//! their own registry via [`BashBgRegistry::new_for_test`] and drop it.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use caliban_agent_core::{Tool, ToolContext, ToolError};
use caliban_provider::{ContentBlock, TextBlock};
use caliban_sandbox::SandboxedShim;
use serde::Deserialize;
use serde_json::{Value, json};

/// 5 GiB default ring-buffer cap, matching Claude Code's documented limit.
pub const DEFAULT_RING_CAP_BYTES: usize = 5 * 1024 * 1024 * 1024;

/// Grace period between SIGTERM and SIGKILL when terminating a background
/// shell. Mirrors the spec; small for test friendliness.
pub const KILL_GRACE: Duration = Duration::from_secs(5);

/// Current status of a background shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BashStatus {
    /// Process is still running.
    Running,
    /// Process exited normally with code.
    Exited(i32),
    /// Process was killed (by SIGTERM/SIGKILL via `KillShell` or signal).
    Killed,
}

impl BashStatus {
    /// Lowercase id string for serialization.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Exited(_) => "exited",
            Self::Killed => "killed",
        }
    }
}

/// Append-only ring buffer with byte cap. On overflow drops the oldest bytes.
#[derive(Debug)]
pub struct RingBuffer {
    cap: usize,
    /// Total bytes ever written (monotonic). Used as the absolute "offset"
    /// in the byte stream so callers can do incremental polling.
    written: u64,
    /// Absolute offset of the first byte still in `buf`. `written - dropped`
    /// equals the byte count currently in `buf`.
    dropped: u64,
    buf: VecDeque<u8>,
}

impl RingBuffer {
    /// Construct an empty buffer with the given cap (in bytes).
    #[must_use]
    pub fn with_cap(cap: usize) -> Self {
        Self {
            cap,
            written: 0,
            dropped: 0,
            buf: VecDeque::with_capacity(std::cmp::min(cap, 64 * 1024)),
        }
    }

    /// Append `bytes`, dropping the oldest data if the buffer would exceed
    /// its cap. Returns the new total `written` offset.
    pub fn push(&mut self, bytes: &[u8]) -> u64 {
        // If the chunk alone is bigger than the cap, only keep the tail.
        let to_take = if bytes.len() > self.cap {
            let start = bytes.len() - self.cap;
            self.buf.clear();
            self.dropped = self.written + (start as u64);
            &bytes[start..]
        } else {
            bytes
        };
        // Drop from the front to make room.
        let new_total = self.buf.len() + to_take.len();
        if new_total > self.cap {
            let drop_n = new_total - self.cap;
            for _ in 0..drop_n {
                self.buf.pop_front();
            }
            self.dropped += drop_n as u64;
        }
        self.buf.extend(to_take);
        self.written += bytes.len() as u64;
        self.written
    }

    /// Total bytes written since the start of the buffer's life.
    #[must_use]
    pub fn written(&self) -> u64 {
        self.written
    }

    /// Total bytes evicted to honor the cap.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Current resident byte count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// True when no bytes are resident (note: bytes may have been dropped).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Read all currently-resident bytes as a UTF-8 lossy string. Returns
    /// `(text, absolute_end_offset)` where `absolute_end_offset = written`.
    #[must_use]
    pub fn snapshot(&self) -> (String, u64) {
        let bytes: Vec<u8> = self.buf.iter().copied().collect();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        (text, self.written)
    }

    /// Read the tail of resident bytes that begin at or after absolute offset
    /// `since`. If `since < dropped`, the returned text begins at the oldest
    /// resident byte; the caller can detect a gap by comparing the returned
    /// `start_offset` against the requested `since`.
    #[must_use]
    pub fn read_since(&self, since: u64) -> (String, u64, u64) {
        let start_offset = std::cmp::max(since, self.dropped);
        // start_offset - self.dropped is at most self.buf.len() (≤ cap, usize),
        // so the cast cannot truncate in practice.
        let skip = usize::try_from(start_offset - self.dropped).unwrap_or(usize::MAX);
        let bytes: Vec<u8> = self.buf.iter().copied().skip(skip).collect();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        (text, start_offset, self.written)
    }
}

/// One enrolled background shell.
pub struct BashJob {
    /// Shell id (UUID v4, 12-char prefix).
    pub id: String,
    /// The original command string.
    pub command: String,
    /// Wall clock at enrollment.
    pub started_at: Instant,
    /// Current status (updated by the watcher task).
    pub status: Mutex<BashStatus>,
    /// stdout ring-buffer.
    pub stdout: Mutex<RingBuffer>,
    /// stderr ring-buffer.
    pub stderr: Mutex<RingBuffer>,
    /// Process group leader PID, used for signaling on Unix. `None` when the
    /// child has already been reaped or on non-Unix.
    pub pid: Mutex<Option<u32>>,
    /// Token signaled when the watcher should terminate (e.g. for `KillShell`).
    pub cancel: tokio_util::sync::CancellationToken,
}

impl std::fmt::Debug for BashJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BashJob")
            .field("id", &self.id)
            .field("command", &self.command)
            .field("started_at", &self.started_at)
            .field("status", &*self.status.lock().unwrap())
            .finish_non_exhaustive()
    }
}

impl BashJob {
    /// Snapshot of the job for listing / status display.
    ///
    /// # Panics
    /// Panics if the status mutex has been poisoned by a prior panic.
    #[must_use]
    pub fn snapshot_status(&self) -> BashStatus {
        *self.status.lock().unwrap()
    }
}

/// Shared registry of background shells.
pub struct BashBgRegistry {
    jobs: Mutex<HashMap<String, Arc<BashJob>>>,
    /// Ring-buffer cap applied to newly enrolled jobs (overridable for tests).
    cap_bytes: usize,
}

impl std::fmt::Debug for BashBgRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BashBgRegistry")
            .field("jobs", &self.jobs.lock().unwrap().len())
            .field("cap_bytes", &self.cap_bytes)
            .finish()
    }
}

impl BashBgRegistry {
    /// Build a registry with the default 5 GiB cap.
    #[must_use]
    pub fn new() -> Self {
        Self::with_cap(DEFAULT_RING_CAP_BYTES)
    }

    /// Build a registry with a custom ring-buffer cap. Useful for tests.
    #[must_use]
    pub fn with_cap(cap_bytes: usize) -> Self {
        Self {
            jobs: Mutex::new(HashMap::new()),
            cap_bytes,
        }
    }

    /// Test-only constructor.
    #[must_use]
    pub fn new_for_test(cap_bytes: usize) -> Arc<Self> {
        Arc::new(Self::with_cap(cap_bytes))
    }

    /// Current ring-buffer cap applied to new jobs.
    #[must_use]
    pub fn cap_bytes(&self) -> usize {
        self.cap_bytes
    }

    /// Number of registered jobs (running or finished).
    ///
    /// # Panics
    /// Panics if the registry mutex has been poisoned.
    #[must_use]
    pub fn job_count(&self) -> usize {
        self.jobs.lock().unwrap().len()
    }

    /// Number of jobs currently running.
    ///
    /// # Panics
    /// Panics if the registry mutex has been poisoned.
    #[must_use]
    pub fn running_count(&self) -> usize {
        self.jobs
            .lock()
            .unwrap()
            .values()
            .filter(|j| j.snapshot_status() == BashStatus::Running)
            .count()
    }

    /// Insert a freshly-enrolled job into the registry.
    ///
    /// # Panics
    /// Panics if the registry mutex has been poisoned.
    pub fn insert(&self, job: Arc<BashJob>) {
        self.jobs.lock().unwrap().insert(job.id.clone(), job);
    }

    /// Look up a job by id.
    ///
    /// # Panics
    /// Panics if the registry mutex has been poisoned.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<Arc<BashJob>> {
        self.jobs.lock().unwrap().get(id).cloned()
    }

    /// Remove a job from the registry.
    ///
    /// # Panics
    /// Panics if the registry mutex has been poisoned.
    pub fn remove(&self, id: &str) -> Option<Arc<BashJob>> {
        self.jobs.lock().unwrap().remove(id)
    }

    /// List all job ids and statuses.
    ///
    /// # Panics
    /// Panics if the registry mutex has been poisoned.
    #[must_use]
    pub fn list(&self) -> Vec<(String, BashStatus, String)> {
        self.jobs
            .lock()
            .unwrap()
            .values()
            .map(|j| (j.id.clone(), j.snapshot_status(), j.command.clone()))
            .collect()
    }

    /// Send SIGTERM/SIGKILL to all running jobs. Called on session exit.
    ///
    /// # Panics
    /// Panics if the registry mutex has been poisoned.
    pub fn kill_all(&self) {
        let ids: Vec<String> = self.jobs.lock().unwrap().keys().cloned().collect();
        for id in ids {
            if let Some(job) = self.get(&id)
                && job.snapshot_status() == BashStatus::Running
            {
                kill_job_now(&job, true);
            }
        }
    }
}

impl Default for BashBgRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Process-wide singleton registry. Lives for the duration of the process;
/// `kill_all_on_exit` should be wired into the session shutdown path.
static GLOBAL_REGISTRY: OnceLock<Arc<BashBgRegistry>> = OnceLock::new();

/// Global registry handle (singleton). First call constructs it.
#[must_use]
pub fn global_registry() -> Arc<BashBgRegistry> {
    GLOBAL_REGISTRY
        .get_or_init(|| Arc::new(BashBgRegistry::new()))
        .clone()
}

/// Send TERM (and KILL on Unix if `force_kill` is set) to a job's process group.
#[allow(unsafe_code)] // libc::kill on negative PID signals process group
fn kill_job_now(job: &BashJob, force_kill: bool) {
    let pid = *job.pid.lock().unwrap();
    #[cfg(unix)]
    if let Some(p) = pid {
        // Process-group SIGTERM/SIGKILL — see `shell::signal_process_group`.
        super::signal_process_group(
            p,
            if force_kill {
                libc::SIGKILL
            } else {
                libc::SIGTERM
            },
        );
    }
    #[cfg(not(unix))]
    let _ = pid;
    job.cancel.cancel();
}

/// Generate a 12-char prefix shell id from a UUID v4.
#[must_use]
pub fn new_shell_id() -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    id.chars().take(12).collect()
}

// ---------------------------------------------------------------------------
// Spawn helper used by Bash when `background: true`.
// ---------------------------------------------------------------------------

/// Build the `/bin/sh -c <command>` child shared by the foreground and
/// background Bash paths: piped stdio, kill-on-drop, its own process group
/// (so the whole process tree can be killed), and the OS-sandbox wrap (ADR 0032).
///
/// Centralizing construction here is what guarantees the background path is
/// sandboxed identically to the foreground path. The wrap is a no-op when no
/// shim is attached, the policy is disabled, the backend is unavailable, or
/// the command is on the bypass list (`SandboxedShim::wrap_command`).
///
/// # Errors
///
/// Returns [`ToolError::Execution`] if the sandbox wrap fails (rare — only on
/// an invalid sandbox config).
pub(super) fn build_shell(
    command: &str,
    cwd: &std::path::Path,
    sandbox: Option<&Arc<SandboxedShim>>,
) -> Result<tokio::process::Command, ToolError> {
    use std::process::Stdio;

    let mut shell = tokio::process::Command::new("/bin/sh");
    shell
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    shell.process_group(0);

    if let Some(shim) = sandbox {
        shim.wrap_command(&mut shell, command)
            .map_err(|e| ToolError::execution(std::io::Error::other(format!("sandbox: {e}"))))?;
    }

    Ok(shell)
}

/// Spawn `command` as a background shell, enrolling it in `registry`. Returns
/// the shell id. The shell runs in `cwd` and inherits no stdin; stdout/stderr
/// are piped into the job's ring buffers, and the OS sandbox (when active) is
/// applied via [`build_shell`].
///
/// # Errors
///
/// Returns [`ToolError::Execution`] if the shell fails to spawn.
///
/// # Panics
/// Panics if internal registry / job mutexes have been poisoned.
pub fn spawn_background(
    registry: &Arc<BashBgRegistry>,
    command: String,
    cwd: &std::path::Path,
    sandbox: Option<&Arc<SandboxedShim>>,
) -> Result<String, ToolError> {
    let id = new_shell_id();
    let cap = registry.cap_bytes();

    // Build the shell through the shared helper so the background path is
    // sandbox-wrapped identically to the foreground path (#160) — previously
    // it spawned `/bin/sh` directly and skipped the OS-sandbox wrap entirely.
    let mut shell = build_shell(&command, cwd, sandbox)?;

    let mut child = shell.spawn().map_err(ToolError::execution)?;
    let pid = child.id();
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");

    let job = Arc::new(BashJob {
        id: id.clone(),
        command,
        started_at: Instant::now(),
        status: Mutex::new(BashStatus::Running),
        stdout: Mutex::new(RingBuffer::with_cap(cap)),
        stderr: Mutex::new(RingBuffer::with_cap(cap)),
        pid: Mutex::new(pid),
        cancel: tokio_util::sync::CancellationToken::new(),
    });
    registry.insert(job.clone());

    // Drain stdout and stderr in two reader tasks.
    let stdout_job = job.clone();
    tokio::spawn(async move {
        drain_to_ring(stdout, &stdout_job, true).await;
    });
    let stderr_job = job.clone();
    tokio::spawn(async move {
        drain_to_ring(stderr, &stderr_job, false).await;
    });

    // Watcher task: waits for child exit, updates status.
    let watch_job = job.clone();
    tokio::spawn(async move {
        let exit = tokio::select! {
            r = child.wait() => Some(r),
            () = watch_job.cancel.cancelled() => None,
        };
        {
            let mut status_lock = watch_job.status.lock().unwrap();
            if let Some(Ok(s)) = exit {
                if let Some(code) = s.code() {
                    *status_lock = BashStatus::Exited(code);
                } else {
                    *status_lock = BashStatus::Killed;
                }
            } else {
                *status_lock = BashStatus::Killed;
            }
        }
        if !matches!(exit, Some(Ok(_))) {
            // Best-effort cleanup if cancellation interrupted the wait.
            let _ = child.start_kill();
            drop(child);
        }
        // Clear pid so we don't try to signal a reaped pid.
        *watch_job.pid.lock().unwrap() = None;
    });

    Ok(id)
}

async fn drain_to_ring<R>(reader: R, job: &BashJob, to_stdout: bool)
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    let mut reader = reader;
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if to_stdout {
                    job.stdout.lock().unwrap().push(&buf[..n]);
                } else {
                    job.stderr.lock().unwrap().push(&buf[..n]);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// BashOutput tool
// ---------------------------------------------------------------------------

/// Tool that returns the latest stdout/stderr for an enrolled background shell.
#[derive(Debug)]
pub struct BashOutputTool {
    registry: Arc<BashBgRegistry>,
    schema: OnceLock<Value>,
}

impl BashOutputTool {
    /// Build with an explicit registry handle.
    #[must_use]
    pub fn new(registry: Arc<BashBgRegistry>) -> Self {
        Self {
            registry,
            schema: OnceLock::new(),
        }
    }

    /// Build using the process singleton.
    #[must_use]
    pub fn with_global_registry() -> Self {
        Self::new(global_registry())
    }
}

#[derive(Debug, Deserialize)]
struct BashOutputInput {
    shell_id: String,
    #[serde(default)]
    since_offset: Option<u64>,
}

#[async_trait]
impl Tool for BashOutputTool {
    fn name(&self) -> &'static str {
        "BashOutput"
    }

    fn description(&self) -> &'static str {
        "Read the latest stdout/stderr from a background shell launched via Bash with background:true. Optional since_offset returns only the slice past that absolute byte offset (for incremental polling)."
    }

    fn input_schema(&self) -> &Value {
        self.schema.get_or_init(|| {
            json!({
                "type": "object",
                "properties": {
                    "shell_id": { "type": "string", "description": "Shell id returned by Bash(background=true)." },
                    "since_offset": { "type": "integer", "minimum": 0, "description": "Return only bytes after this absolute byte offset (for incremental polling)." }
                },
                "required": ["shell_id"]
            })
        })
    }

    async fn invoke(&self, input: Value, _cx: ToolContext) -> Result<Vec<ContentBlock>, ToolError> {
        let parsed: BashOutputInput = crate::parse_input(input)?;
        let job = self.registry.get(&parsed.shell_id).ok_or_else(|| {
            ToolError::execution(std::io::Error::other(format!(
                "no background shell with id {}",
                parsed.shell_id
            )))
        })?;
        let since = parsed.since_offset.unwrap_or(0);
        let (stdout_text, stdout_start, stdout_end) = job.stdout.lock().unwrap().read_since(since);
        let (stderr_text, stderr_start, stderr_end) = job.stderr.lock().unwrap().read_since(since);
        let status = job.snapshot_status();

        let age = job.started_at.elapsed();
        let header = format!(
            "shell_id: {}   status: {}   age: {:.1}s\nstdout (bytes {}..{}):\n",
            job.id,
            status.as_str(),
            age.as_secs_f32(),
            stdout_start,
            stdout_end,
        );
        let mid = format!("\nstderr (bytes {stderr_start}..{stderr_end}):\n");
        let text = format!("{header}{stdout_text}{mid}{stderr_text}");

        Ok(vec![ContentBlock::Text(TextBlock {
            text,
            cache_control: None,
        })])
    }
}

// ---------------------------------------------------------------------------
// KillShell tool
// ---------------------------------------------------------------------------

/// Tool that terminates a background shell via SIGTERM and falls through to
/// SIGKILL after a grace window.
#[derive(Debug)]
pub struct KillShellTool {
    registry: Arc<BashBgRegistry>,
    grace: Duration,
    schema: OnceLock<Value>,
}

impl KillShellTool {
    /// Build with an explicit registry handle.
    #[must_use]
    pub fn new(registry: Arc<BashBgRegistry>) -> Self {
        Self::with_grace(registry, KILL_GRACE)
    }

    /// Build with a custom SIGTERM→SIGKILL grace window (for tests).
    #[must_use]
    pub fn with_grace(registry: Arc<BashBgRegistry>, grace: Duration) -> Self {
        Self {
            registry,
            grace,
            schema: OnceLock::new(),
        }
    }

    /// Build using the process singleton.
    #[must_use]
    pub fn with_global_registry() -> Self {
        Self::new(global_registry())
    }
}

#[derive(Debug, Deserialize)]
struct KillShellInput {
    shell_id: String,
}

#[async_trait]
impl Tool for KillShellTool {
    fn name(&self) -> &'static str {
        "KillShell"
    }

    fn description(&self) -> &'static str {
        "Terminate a background shell launched via Bash with background:true. Sends SIGTERM, waits ~5s, then SIGKILL. Reaps the child."
    }

    fn input_schema(&self) -> &Value {
        self.schema.get_or_init(|| {
            json!({
                "type": "object",
                "properties": {
                    "shell_id": { "type": "string", "description": "Shell id returned by Bash(background=true)." }
                },
                "required": ["shell_id"]
            })
        })
    }

    async fn invoke(&self, input: Value, _cx: ToolContext) -> Result<Vec<ContentBlock>, ToolError> {
        let parsed: KillShellInput = crate::parse_input(input)?;
        let job = self.registry.get(&parsed.shell_id).ok_or_else(|| {
            ToolError::execution(std::io::Error::other(format!(
                "no background shell with id {}",
                parsed.shell_id
            )))
        })?;

        if job.snapshot_status() != BashStatus::Running {
            return Ok(vec![ContentBlock::Text(TextBlock {
                text: format!(
                    "Shell {} is already in status {}; no action taken.",
                    job.id,
                    job.snapshot_status().as_str()
                ),
                cache_control: None,
            })]);
        }

        kill_job_now(&job, false);

        // Wait up to grace for the watcher task to mark it killed/exited.
        let deadline = Instant::now() + self.grace;
        while Instant::now() < deadline {
            if job.snapshot_status() != BashStatus::Running {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        if job.snapshot_status() == BashStatus::Running {
            kill_job_now(&job, true);
            // Give the reaper a brief tick.
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        let status = job.snapshot_status();
        let consumed_stdout = job.stdout.lock().unwrap().written();
        let consumed_stderr = job.stderr.lock().unwrap().written();
        Ok(vec![ContentBlock::Text(TextBlock {
            text: format!(
                "Killed shell {}; status={}; consumed_stdout={} bytes, consumed_stderr={} bytes",
                job.id,
                status.as_str(),
                consumed_stdout,
                consumed_stderr,
            ),
            cache_control: None,
        })])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    fn ctx() -> ToolContext {
        ToolContext {
            tool_use_id: "t1".into(),
            cancel: CancellationToken::new(),
            hooks: None,
            turn_index: 0,
        }
    }

    // ----------------------------------------------------------------------
    // RingBuffer
    // ----------------------------------------------------------------------

    #[test]
    fn ring_buffer_drops_oldest_at_cap() {
        // 16 B cap; write 24 bytes; oldest 8 bytes dropped.
        let mut rb = RingBuffer::with_cap(16);
        rb.push(b"0123456789ABCDEF");
        assert_eq!(rb.len(), 16);
        assert_eq!(rb.written(), 16);
        rb.push(b"GHIJKLMN");
        assert_eq!(rb.len(), 16);
        assert_eq!(rb.dropped(), 8);
        let (text, end) = rb.snapshot();
        assert_eq!(text, "89ABCDEFGHIJKLMN");
        assert_eq!(end, 24);
    }

    #[test]
    fn ring_buffer_handles_chunk_bigger_than_cap() {
        let mut rb = RingBuffer::with_cap(4);
        rb.push(b"0123456789");
        let (text, end) = rb.snapshot();
        assert_eq!(text, "6789");
        assert_eq!(end, 10);
        assert_eq!(rb.dropped(), 6);
    }

    #[test]
    fn ring_buffer_read_since_returns_tail() {
        let mut rb = RingBuffer::with_cap(32);
        rb.push(b"hello world");
        let (text, start, end) = rb.read_since(6);
        assert_eq!(text, "world");
        assert_eq!(start, 6);
        assert_eq!(end, 11);
    }

    // ----------------------------------------------------------------------
    // spawn_background / BashOutput / KillShell
    // ----------------------------------------------------------------------

    #[test]
    fn build_shell_without_sandbox_invokes_bin_sh_directly() {
        let cmd = build_shell("echo hi", &std::env::current_dir().unwrap(), None).unwrap();
        let std_cmd = cmd.as_std();
        assert_eq!(std_cmd.get_program(), "/bin/sh");
        let args: Vec<String> = std_cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["-c", "echo hi"]);
    }

    #[test]
    fn build_shell_routes_through_the_sandbox_wrap() {
        // Regression for #160: the background path must apply the OS-sandbox
        // wrap exactly like the foreground path. An *active* shim rewrites the
        // program to the sandbox wrapper; an inactive one (disabled policy, or
        // no backend binary on this runner) is a documented no-op that leaves
        // `/bin/sh` in place. Either way the wrap is now invoked.
        let policy = caliban_sandbox::Policy {
            enabled: true,
            ..Default::default()
        };
        let shim = Arc::new(caliban_sandbox::SandboxedShim::new(policy).unwrap());
        let cmd = build_shell("echo hi", &std::env::current_dir().unwrap(), Some(&shim)).unwrap();
        let program = cmd.as_std().get_program().to_string_lossy().into_owned();
        if shim.is_active() {
            assert_ne!(
                program, "/bin/sh",
                "an active sandbox must wrap the shell program",
            );
        } else {
            assert_eq!(program, "/bin/sh");
        }
    }

    #[tokio::test]
    async fn spawn_background_returns_shell_id_immediately() {
        let reg = BashBgRegistry::new_for_test(1024 * 1024);
        let start = Instant::now();
        let id = spawn_background(
            &reg,
            "sleep 5".into(),
            &std::env::current_dir().unwrap(),
            None,
        )
        .unwrap();
        // The call must not block — well under 1s.
        assert!(start.elapsed() < Duration::from_millis(500));
        assert_eq!(id.len(), 12);
        // Registry knows about it.
        assert!(reg.get(&id).is_some());
        assert_eq!(reg.running_count(), 1);
        // Tidy up.
        if let Some(job) = reg.get(&id) {
            kill_job_now(&job, true);
        }
    }

    #[tokio::test]
    async fn bash_output_returns_streaming_stdout() {
        let reg = BashBgRegistry::new_for_test(1024 * 1024);
        let id = spawn_background(
            &reg,
            "printf 'hello'; sleep 30".into(),
            &std::env::current_dir().unwrap(),
            None,
        )
        .unwrap();
        // Give the drainer a moment.
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let job = reg.get(&id).unwrap();
            let (text, _e) = job.stdout.lock().unwrap().snapshot();
            if text.contains("hello") {
                break;
            }
        }
        let tool = BashOutputTool::new(reg.clone());
        let out = tool.invoke(json!({"shell_id": id}), ctx()).await.unwrap();
        let ContentBlock::Text(t) = &out[0] else {
            panic!("expected Text")
        };
        assert!(t.text.contains("hello"), "out: {}", t.text);
        assert!(t.text.contains("status: running"), "out: {}", t.text);
        // Tidy up.
        if let Some(job) = reg.get(&id) {
            kill_job_now(&job, true);
        }
    }

    #[tokio::test]
    async fn bash_output_supports_since_offset() {
        let reg = BashBgRegistry::new_for_test(1024 * 1024);
        let id = spawn_background(
            &reg,
            "printf 'aaaaa'; sleep 30".into(),
            &std::env::current_dir().unwrap(),
            None,
        )
        .unwrap();
        // Wait until we have 5 bytes.
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let job = reg.get(&id).unwrap();
            if job.stdout.lock().unwrap().written() >= 5 {
                break;
            }
        }
        let tool = BashOutputTool::new(reg.clone());
        let out = tool
            .invoke(json!({"shell_id": id, "since_offset": 3}), ctx())
            .await
            .unwrap();
        let ContentBlock::Text(t) = &out[0] else {
            panic!("expected Text")
        };
        assert!(t.text.contains("bytes 3..5"), "out: {}", t.text);
        if let Some(job) = reg.get(&id) {
            kill_job_now(&job, true);
        }
    }

    #[tokio::test]
    async fn kill_shell_terminates_running_job() {
        let reg = BashBgRegistry::new_for_test(1024 * 1024);
        let id = spawn_background(
            &reg,
            "sleep 60".into(),
            &std::env::current_dir().unwrap(),
            None,
        )
        .unwrap();
        assert_eq!(reg.running_count(), 1);
        let tool = KillShellTool::with_grace(reg.clone(), Duration::from_millis(500));
        let out = tool
            .invoke(json!({"shell_id": id.clone()}), ctx())
            .await
            .unwrap();
        let ContentBlock::Text(t) = &out[0] else {
            panic!("expected Text")
        };
        assert!(t.text.contains("Killed shell"), "out: {}", t.text);
        // Give the OS a tick to reap.
        for _ in 0..20 {
            if reg.running_count() == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(reg.running_count(), 0);
    }

    #[tokio::test]
    async fn kill_all_terminates_every_running_job() {
        let reg = BashBgRegistry::new_for_test(1024 * 1024);
        let ids: Vec<String> = (0..3)
            .map(|_| {
                spawn_background(
                    &reg,
                    "sleep 60".into(),
                    &std::env::current_dir().unwrap(),
                    None,
                )
                .unwrap()
            })
            .collect();
        assert_eq!(reg.running_count(), 3);
        reg.kill_all();
        for _ in 0..40 {
            if reg.running_count() == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(reg.running_count(), 0);
        for id in ids {
            let job = reg.get(&id).unwrap();
            assert_ne!(job.snapshot_status(), BashStatus::Running);
        }
    }

    #[tokio::test]
    async fn bash_output_unknown_id_returns_error() {
        let reg = BashBgRegistry::new_for_test(1024);
        let tool = BashOutputTool::new(reg);
        let err = tool
            .invoke(json!({"shell_id": "doesnotexist"}), ctx())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Execution(_)));
        let msg = format!("{err}");
        assert!(msg.contains("no background shell"), "msg: {msg}");
    }

    #[test]
    fn new_shell_id_is_12_chars() {
        let id = new_shell_id();
        assert_eq!(id.len(), 12);
        assert!(id.chars().all(|c| c.is_ascii_alphanumeric()));
    }
}
