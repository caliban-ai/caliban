//! Supervisor accept loop + request dispatch.
//!
//! This is the heart of the `caliband` daemon. It binds a Unix domain
//! socket, accepts connections, reads newline-delimited JSON requests,
//! and dispatches them against the [`crate::Registry`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::proc::WorkerLauncher;
use crate::proto::{CtlReply, CtlRequest, DaemonStatus, SupervisorError};
use crate::registry::Registry;
use crate::store::AgentStore;

/// Per-daemon-process supervisor. Owns the registry, accept loop, and
/// graceful-shutdown token.
pub struct Supervisor {
    socket_path: PathBuf,
    started: Instant,
    registry: Arc<Mutex<Registry>>,
    cancel: CancellationToken,
    /// Per-agent runtime-dir (where per-agent sockets are created).
    agent_runtime_dir: PathBuf,
    /// Strategy for launching worker processes.
    #[allow(dead_code)] // read in Task 3 (Spawn wiring)
    launcher: Arc<dyn WorkerLauncher>,
    /// Live worker pids, keyed by agent id. Inserted on launch, read by
    /// `Kill`, removed by the per-child monitor task on exit.
    #[allow(dead_code)] // read in Task 3 (Spawn wiring)
    procs: Arc<Mutex<HashMap<crate::proto::AgentId, u32>>>,
}

impl Supervisor {
    /// Construct a supervisor that launches real `caliban __agent-worker`
    /// children.
    pub fn new(
        socket_path: impl Into<PathBuf>,
        store: AgentStore,
        agent_runtime_dir: impl Into<PathBuf>,
    ) -> Self {
        Self::with_launcher(
            socket_path,
            store,
            agent_runtime_dir,
            Arc::new(crate::proc::ExecWorkerLauncher::sibling_of_current_exe()),
        )
    }

    /// Construct a supervisor with an explicit worker launcher (tests
    /// inject a fake here so the daemon lifecycle runs without an LLM).
    pub fn with_launcher(
        socket_path: impl Into<PathBuf>,
        store: AgentStore,
        agent_runtime_dir: impl Into<PathBuf>,
        launcher: Arc<dyn WorkerLauncher>,
    ) -> Self {
        let socket_path = socket_path.into();
        let agent_runtime_dir = agent_runtime_dir.into();
        let registry = Arc::new(Mutex::new(Registry::new(store)));
        Self {
            socket_path,
            started: Instant::now(),
            registry,
            cancel: CancellationToken::new(),
            agent_runtime_dir,
            launcher,
            procs: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Cancellation handle the daemon binary can fire on `SIGTERM` / `Shutdown`.
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Control socket path.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Bind the socket and accept clients until `cancel_token()` fires.
    /// Returns when the cancellation fires.
    pub async fn serve(self: Arc<Self>) -> std::io::Result<()> {
        if let Some(parent) = self.socket_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        if let Some(parent) = self.agent_runtime_dir.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::create_dir_all(&self.agent_runtime_dir).await?;

        // Best-effort: unlink a stale socket from a previous run.
        let _ = tokio::fs::remove_file(&self.socket_path).await;
        let listener = UnixListener::bind(&self.socket_path)?;
        tracing::info!(socket = %self.socket_path.display(), "supervisor listening");

        // Sweep crashed agents on startup so a daemon-restart shows the
        // right thing in `list`.
        {
            let mut r = self.registry.lock().await;
            let swept = r.sweep_crashed();
            if !swept.is_empty() {
                tracing::warn!(count = swept.len(), "swept Running agents → Crashed");
            }
        }

        loop {
            tokio::select! {
                () = self.cancel.cancelled() => {
                    tracing::info!("supervisor shutdown requested");
                    break;
                }
                accepted = listener.accept() => {
                    match accepted {
                        Ok((stream, _addr)) => {
                            let me = Arc::clone(&self);
                            tokio::spawn(async move {
                                if let Err(e) = me.handle_client(stream).await {
                                    tracing::warn!(error = %e, "client handler error");
                                }
                            });
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "accept failed");
                        }
                    }
                }
            }
        }

        // Best-effort cleanup.
        let _ = tokio::fs::remove_file(&self.socket_path).await;
        Ok(())
    }

    async fn handle_client(self: Arc<Self>, stream: UnixStream) -> std::io::Result<()> {
        let (read_half, mut write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half);
        let mut line = String::new();
        loop {
            line.clear();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                return Ok(());
            }
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                continue;
            }
            let req: CtlRequest = match serde_json::from_str(trimmed) {
                Ok(r) => r,
                Err(e) => {
                    let reply = CtlReply::Error {
                        error: SupervisorError::Internal {
                            message: format!("bad request: {e}"),
                        },
                    };
                    write_reply(&mut write_half, &reply).await?;
                    continue;
                }
            };
            let reply = self.dispatch(req).await;
            let is_shutdown = matches!(reply, CtlReply::ShutdownAck);
            write_reply(&mut write_half, &reply).await?;
            if is_shutdown {
                self.cancel.cancel();
                return Ok(());
            }
        }
    }

    async fn dispatch(&self, req: CtlRequest) -> CtlReply {
        match req {
            CtlRequest::List => {
                let r = self.registry.lock().await;
                CtlReply::Listed { agents: r.list() }
            }
            CtlRequest::Spawn { spec } => {
                let mut r = self.registry.lock().await;
                let id_prefix = uuid::Uuid::new_v4().simple().to_string();
                let socket_name = format!("{}-agent.sock", &id_prefix[..8]);
                let socket_path = self.agent_runtime_dir.join(socket_name);
                let rec = r.register(spec, socket_path.clone());
                CtlReply::Spawned {
                    id: rec.id,
                    socket_path,
                }
            }
            CtlRequest::Attach { id } => {
                let r = self.registry.lock().await;
                match r.get(&id) {
                    Some(rec) => CtlReply::AttachAck {
                        socket_path: rec.socket_path.clone(),
                    },
                    None => CtlReply::Error {
                        error: SupervisorError::NotFound { id },
                    },
                }
            }
            CtlRequest::Kill { id } => {
                let mut r = self.registry.lock().await;
                // We don't actually own a child process today (sub-agent
                // worker spawning is wired separately), so `kill` just
                // updates the registry state. Real signal delivery
                // hangs off the worker task and is covered by the
                // foreground-handoff integration tests.
                match r.set_status(&id, crate::proto::AgentStatus::Killed) {
                    Ok(_) => CtlReply::Killed,
                    Err(e) => CtlReply::Error { error: e },
                }
            }
            CtlRequest::Respawn { id } => {
                let mut r = self.registry.lock().await;
                let Some(old) = r.get(&id).cloned() else {
                    return CtlReply::Error {
                        error: SupervisorError::NotFound { id },
                    };
                };
                // Drop old (force=true so it can be running) and
                // re-register with the same spec.
                if let Err(e) = r.remove(&id, true) {
                    return CtlReply::Error { error: e };
                }
                let id_prefix = uuid::Uuid::new_v4().simple().to_string();
                let socket_name = format!("{}-agent.sock", &id_prefix[..8]);
                let socket_path = self.agent_runtime_dir.join(socket_name);
                let rec = r.register(old.spec, socket_path);
                CtlReply::Respawned { id: rec.id }
            }
            CtlRequest::Rm { id, force } => {
                let mut r = self.registry.lock().await;
                match r.remove(&id, force) {
                    Ok(()) => CtlReply::Removed,
                    Err(e) => CtlReply::Error { error: e },
                }
            }
            CtlRequest::Status => {
                let r = self.registry.lock().await;
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let agents = r.len() as u32;
                let uptime_secs = self.started.elapsed().as_secs();
                CtlReply::Status(DaemonStatus {
                    pid: std::process::id(),
                    agents,
                    uptime_secs,
                    socket_path: self.socket_path.clone(),
                })
            }
            CtlRequest::Shutdown => CtlReply::ShutdownAck,
        }
    }
}

async fn write_reply(
    stream: &mut tokio::net::unix::OwnedWriteHalf,
    reply: &CtlReply,
) -> std::io::Result<()> {
    let mut body = serde_json::to_vec(reply).map_err(std::io::Error::other)?;
    body.push(b'\n');
    stream.write_all(&body).await?;
    stream.flush().await?;
    Ok(())
}
