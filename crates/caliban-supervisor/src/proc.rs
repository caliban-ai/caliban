//! Worker-process launch + tracking for the supervisor.
//!
//! The supervisor crate is deliberately free of `caliban-agent-core`
//! (ADR 0037: the daemon stays thin). It therefore runs sub-agents by
//! launching the `caliban` binary in its hidden `__agent-worker`
//! subcommand. [`WorkerLauncher`] abstracts that launch so the daemon
//! lifecycle can be tested with a fake worker (a trivial child process)
//! that never touches an LLM.

use std::path::PathBuf;

use crate::proto::AgentRecord;

/// A launched worker: the async child handle the supervisor waits on,
/// plus its OS pid (cached so `Kill` can signal it after the `Child`
/// has been moved into the monitor task).
#[derive(Debug)]
pub struct WorkerHandle {
    /// OS process id of the worker.
    pub pid: u32,
    /// The async child handle (owned by the monitor task).
    pub child: tokio::process::Child,
}

/// Strategy for turning an [`AgentRecord`] into a running worker process.
pub trait WorkerLauncher: Send + Sync {
    /// Launch a worker for `record`. The worker is expected to bind
    /// `record.endpoint` (a Unix socket path today; TCP from #280 Task 7)
    /// and run the agent described by `record.spec`.
    fn launch(&self, record: &AgentRecord) -> std::io::Result<WorkerHandle>;
}

/// Production launcher: spawns `caliban __agent-worker …`.
#[derive(Debug, Clone, Default)]
pub struct ExecWorkerLauncher {
    /// Absolute path to the `caliban` binary to exec.
    caliban_exe: PathBuf,
    /// Optional daemon control-socket path passed to the worker so it can
    /// report Idle/Running transitions (#81).
    control_socket: Option<PathBuf>,
    /// Network mode (#280 Task 7): per-agent TLS cert PEM path passed to the
    /// worker via env so it can secure its own TCP per-agent listener.
    agent_tls_cert_path: Option<PathBuf>,
    /// Per-agent TLS private-key PEM path (env-passed alongside the cert).
    agent_tls_key_path: Option<PathBuf>,
    /// Per-agent bearer token the worker requires on its TCP listener.
    agent_token: Option<String>,
    /// Daemon control endpoint (`host:port`) the worker reports status to over
    /// the network in TCP mode (the network counterpart to `control_socket`).
    control_endpoint: Option<String>,
}

impl ExecWorkerLauncher {
    /// Build a launcher that execs `caliban_exe`.
    pub fn new(caliban_exe: impl Into<PathBuf>) -> Self {
        Self {
            caliban_exe: caliban_exe.into(),
            ..Self::default()
        }
    }

    /// Resolve the `caliban` binary sitting next to the current
    /// executable (same cargo target dir as `caliband`), falling back to
    /// a bare `caliban` for `PATH` lookup.
    #[must_use]
    pub fn sibling_of_current_exe() -> Self {
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("caliban"));
        let mut caliban = exe;
        caliban.set_file_name("caliban");
        if !caliban.exists() {
            caliban = PathBuf::from("caliban");
        }
        Self::new(caliban)
    }

    /// Set the daemon control-socket path the worker reports status to (#81).
    #[must_use]
    pub fn with_control_socket(mut self, path: impl Into<PathBuf>) -> Self {
        self.control_socket = Some(path.into());
        self
    }

    /// Configure the per-agent network material (#280 Task 7) the worker needs
    /// to secure its own TCP listener and to report status back over the
    /// network: TLS cert/key PEM file paths, a bearer token, and the daemon's
    /// control endpoint (`host:port`). All optional; passed to the child via
    /// env only when the agent's endpoint is TCP.
    #[must_use]
    pub fn with_agent_network(
        mut self,
        tls_cert_path: Option<PathBuf>,
        tls_key_path: Option<PathBuf>,
        token: Option<String>,
        control_endpoint: Option<String>,
    ) -> Self {
        self.agent_tls_cert_path = tls_cert_path;
        self.agent_tls_key_path = tls_key_path;
        self.agent_token = token;
        self.control_endpoint = control_endpoint;
        self
    }
}

impl WorkerLauncher for ExecWorkerLauncher {
    fn launch(&self, record: &AgentRecord) -> std::io::Result<WorkerHandle> {
        let manifest_path = record.session_dir.join("manifest.json");
        let mut cmd = tokio::process::Command::new(&self.caliban_exe);
        cmd.arg("__agent-worker")
            .arg("--manifest")
            .arg(&manifest_path);
        match &record.endpoint {
            // Unix mode: bind the per-agent socket path.
            crate::transport::Endpoint::Unix { path } => {
                cmd.arg("--socket").arg(path);
            }
            // Network mode (#280 Task 7): the advertised endpoint is
            // `{advertise_host}:{port}`, but the worker binds locally on the
            // SAME port across all interfaces — `0.0.0.0:{port}`.
            crate::transport::Endpoint::Tcp { addr } => {
                let port = addr.rsplit(':').next().unwrap_or_default();
                cmd.arg("--listen").arg(format!("0.0.0.0:{port}"));
                // The worker secures its own listener with the same TLS/token
                // as the control plane; pass them via env (symmetric wiring).
                if let Some(cert) = &self.agent_tls_cert_path {
                    cmd.env("CALIBAN_AGENT_TLS_CERT", cert);
                }
                if let Some(key) = &self.agent_tls_key_path {
                    cmd.env("CALIBAN_AGENT_TLS_KEY", key);
                }
                if let Some(token) = &self.agent_token {
                    cmd.env("CALIBAN_AGENT_TOKEN", token);
                }
                // How the worker reaches the daemon's control listener over the
                // network to report Idle/Running (best-effort; QA-validated).
                if let Some(ep) = &self.control_endpoint {
                    cmd.env("CALIBAN_CONTROL_ENDPOINT", ep);
                }
            }
        }
        if let Some(ref ctl) = self.control_socket {
            cmd.arg("--control-socket").arg(ctl);
        }
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let child = cmd.spawn()?;
        let pid = child
            .id()
            .ok_or_else(|| std::io::Error::other("worker child has no pid (already exited?)"))?;
        Ok(WorkerHandle { pid, child })
    }
}

/// Best-effort `SIGTERM` to `pid`. No-op on non-unix. Returns whether
/// the signal was delivered (false if the process was already gone).
#[cfg(unix)]
#[must_use]
pub fn signal_term(pid: u32) -> bool {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    #[allow(clippy::cast_possible_wrap)] // pids fit in i32 on all supported unix platforms
    let raw = pid as i32;
    kill(Pid::from_raw(raw), Signal::SIGTERM).is_ok()
}

/// Non-unix stub: signal delivery is unsupported.
#[cfg(not(unix))]
#[must_use]
pub fn signal_term(_pid: u32) -> bool {
    false
}

/// Strategy for delivering a termination signal to a worker pid. Abstracted
/// (like [`WorkerLauncher`]) so tests can interpose on the exact moment a
/// signal is sent — e.g. to deterministically drive the `Kill`/`Respawn`
/// interleaving without relying on a probabilistic race.
pub trait Signaller: Send + Sync {
    /// Best-effort terminate `pid`. Returns whether the signal was delivered.
    fn signal_term(&self, pid: u32) -> bool;
}

/// Production signaller: delivers a real `SIGTERM` via [`signal_term`].
#[derive(Debug, Default, Clone, Copy)]
pub struct OsSignaller;

impl Signaller for OsSignaller {
    fn signal_term(&self, pid: u32) -> bool {
        signal_term(pid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{AgentRecord, AgentStatus, SpawnSpec};
    use std::path::PathBuf;

    fn record(socket: PathBuf, session_dir: PathBuf) -> AgentRecord {
        AgentRecord {
            id: "test0001".into(),
            name: "test".into(),
            status: AgentStatus::Spawning,
            started_at: "2026-06-09T00:00:00Z".into(),
            session_dir,
            endpoint: crate::transport::Endpoint::Unix { path: socket },
            spec: SpawnSpec {
                label: None,
                frontmatter_path: None,
                initial_prompt: "hi".into(),
                model: None,
                provider: None,
                tool_allowlist: None,
                isolation_worktree: false,
                inherit_hooks: true,
                interactive: false,
                inherited_hooks_config: None,
            },
        }
    }

    /// A fake launcher for this module's tests: runs `/bin/sh -c <script>`
    /// so a test can script the worker's behavior (bind the socket, sleep,
    /// exit N) with no LLM. The daemon's integration tests (tests/ipc.rs)
    /// define their own copy once worker spawning is wired.
    struct ShLauncher {
        script: String,
    }

    impl WorkerLauncher for ShLauncher {
        fn launch(&self, _record: &AgentRecord) -> std::io::Result<WorkerHandle> {
            let mut cmd = tokio::process::Command::new("/bin/sh");
            cmd.arg("-c").arg(&self.script);
            let child = cmd.spawn()?;
            let pid = child.id().expect("sh child pid");
            Ok(WorkerHandle { pid, child })
        }
    }

    #[tokio::test]
    async fn launch_runs_a_real_child_that_exits_zero() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");
        let launcher = ShLauncher {
            script: format!("touch {}; exit 0", socket.display()),
        };
        let rec = record(socket.clone(), dir.path().to_path_buf());
        let mut handle = launcher.launch(&rec).unwrap();
        let status = handle.child.wait().await.unwrap();
        assert!(status.success());
        assert!(
            socket.exists(),
            "worker should have created the socket file"
        );
    }
}
