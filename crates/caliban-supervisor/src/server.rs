//! Supervisor accept loop + request dispatch.
//!
//! This is the heart of the `caliband` daemon. It binds a Unix domain
//! socket, accepts connections, reads newline-delimited JSON requests,
//! and dispatches them against the [`crate::Registry`].

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::proc::{OsSignaller, Signaller, WorkerLauncher};
use crate::proto::{CtlReply, CtlRequest, DaemonStatus, SupervisorError};
use crate::registry::Registry;
use crate::store::AgentStore;
use crate::transport::Endpoint;

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
    launcher: Arc<dyn WorkerLauncher>,
    /// Strategy for delivering termination signals to worker pids.
    signaller: Arc<dyn Signaller>,
}

impl Supervisor {
    /// Construct a supervisor that launches real `caliban __agent-worker`
    /// children.
    pub fn new(
        socket_path: impl Into<PathBuf>,
        store: AgentStore,
        agent_runtime_dir: impl Into<PathBuf>,
    ) -> Self {
        let socket_path: PathBuf = socket_path.into();
        let launcher = Arc::new(
            crate::proc::ExecWorkerLauncher::sibling_of_current_exe()
                .with_control_socket(socket_path.clone()),
        );
        Self::with_launcher(socket_path, store, agent_runtime_dir, launcher)
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
            signaller: Arc::new(OsSignaller),
        }
    }

    /// Override the signaller (tests interpose here to drive the
    /// `Kill`/`Respawn` interleaving deterministically).
    #[must_use]
    pub fn with_signaller(mut self, signaller: Arc<dyn Signaller>) -> Self {
        self.signaller = signaller;
        self
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

    /// Launch a worker for `rec`, record its pid, flip the registry to
    /// `Running`, and spawn a monitor task that writes the terminal
    /// status when the child exits. On launch failure the agent is
    /// marked `Failed`.
    async fn launch_and_monitor(&self, rec: crate::proto::AgentRecord) {
        let id = rec.id.clone();
        match self.launcher.launch(&rec) {
            Ok(handle) => {
                let pid = handle.pid;
                let mut child = handle.child;
                // Track the pid and flip to Running in one registry critical
                // section, so the pid and status never disagree (#140).
                {
                    let mut r = self.registry.lock().await;
                    r.track_pid(&id, pid);
                    r.set_status_if_running(&id, crate::proto::AgentStatus::Running);
                }
                let registry = Arc::clone(&self.registry);
                // Once the worker exits, its per-agent socket is stale —
                // unlink it (the worker can't reliably clean up on exit; #77).
                // Only Unix endpoints have a filesystem path to unlink; a TCP
                // endpoint (#280 Task 7) has nothing to clean up here.
                let socket_path = rec.unix_socket_path().map(std::path::Path::to_path_buf);
                tokio::spawn(async move {
                    // The wait MUST stay outside the registry lock — holding it
                    // across the child's lifetime would serialize the daemon.
                    let terminal = match child.wait().await {
                        Ok(s) if s.success() => crate::proto::AgentStatus::Done,
                        Ok(_) => crate::proto::AgentStatus::Failed,
                        Err(e) => {
                            tracing::warn!(error = %e, agent = %id, "worker wait failed");
                            crate::proto::AgentStatus::Failed
                        }
                    };
                    // Record the terminal status and forget the pid together.
                    {
                        let mut r = registry.lock().await;
                        r.set_status_if_running(&id, terminal);
                        r.forget_pid(&id);
                    }
                    if let Some(socket_path) = socket_path {
                        let _ = tokio::fs::remove_file(&socket_path).await;
                    }
                });
            }
            Err(e) => {
                tracing::warn!(error = %e, agent = %id, "worker launch failed");
                self.registry
                    .lock()
                    .await
                    .set_status(&id, crate::proto::AgentStatus::Failed)
                    .ok();
            }
        }
    }

    /// Deliver `SIGTERM` to the live worker tracked for `id`, if any.
    ///
    /// The pid now lives inside the registry (#140), so the caller passes its
    /// held `&Registry`: looking up the pid and signalling both happen within
    /// the caller's registry critical section. A concurrent `Respawn` therefore
    /// cannot supersede the worker between the signal and the surrounding
    /// bookkeeping (#115, #138), and there is no separate pid lock to mis-order.
    fn signal_worker(&self, registry: &Registry, id: &crate::proto::AgentId) {
        if let Some(pid) = registry.pid_of(id) {
            let delivered = self.signaller.signal_term(pid);
            tracing::info!(agent = %id, pid, delivered, "sent SIGTERM to worker");
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn dispatch(&self, req: CtlRequest) -> CtlReply {
        match req {
            CtlRequest::List => {
                let r = self.registry.lock().await;
                CtlReply::Listed { agents: r.list() }
            }
            CtlRequest::Spawn { spec } => {
                let rec = {
                    let mut r = self.registry.lock().await;
                    let id_prefix = uuid::Uuid::new_v4().simple().to_string();
                    let socket_name = format!("{}-agent.sock", &id_prefix[..8]);
                    let socket_path = self.agent_runtime_dir.join(socket_name);
                    r.register(spec, Endpoint::Unix { path: socket_path })
                };
                let reply = CtlReply::Spawned {
                    id: rec.id.clone(),
                    endpoint: rec.endpoint.clone(),
                };
                self.launch_and_monitor(rec).await;
                reply
            }
            CtlRequest::Attach { id } => {
                let r = self.registry.lock().await;
                match r.get(&id) {
                    Some(rec) => CtlReply::AttachAck {
                        endpoint: rec.endpoint.clone(),
                    },
                    None => CtlReply::Error {
                        error: SupervisorError::NotFound { id },
                    },
                }
            }
            CtlRequest::Kill { id } => {
                // Signal the owned child if we have its pid; then record the
                // state transition. The monitor task observes the real exit but
                // won't clobber `Killed` (guarded by `set_status_if_running`)
                // and forgets the pid on exit.
                //
                // The pid lives in the registry, so the signal and the status
                // update share one critical section — a concurrent `Respawn`
                // cannot supersede the worker between them (#115, #140).
                let mut r = self.registry.lock().await;
                self.signal_worker(&r, &id);
                match r.set_status(&id, crate::proto::AgentStatus::Killed) {
                    Ok(_) => CtlReply::Killed,
                    Err(e) => CtlReply::Error { error: e },
                }
            }
            CtlRequest::Respawn { id } => {
                let new_rec = {
                    let mut r = self.registry.lock().await;
                    let Some(old) = r.get(&id).cloned() else {
                        return CtlReply::Error {
                            error: SupervisorError::NotFound { id },
                        };
                    };
                    // Drop old (force=true so it can be running) and
                    // re-register with the same spec. `remove` also forgets the
                    // old pid, all under this one registry lock — a concurrent
                    // `Kill` cannot observe a torn pid/record state (#115, #140).
                    if let Err(e) = r.remove(&id, true) {
                        return CtlReply::Error { error: e };
                    }
                    let id_prefix = uuid::Uuid::new_v4().simple().to_string();
                    let socket_name = format!("{}-agent.sock", &id_prefix[..8]);
                    let socket_path = self.agent_runtime_dir.join(socket_name);
                    r.register(old.spec, Endpoint::Unix { path: socket_path })
                };
                let reply = CtlReply::Respawned {
                    id: new_rec.id.clone(),
                };
                self.launch_and_monitor(new_rec).await;
                reply
            }
            CtlRequest::Rm { id, force } => {
                // Force-removing a still-running agent must not orphan its
                // worker: signal the owned child first, mirroring `Kill`
                // (#76). Only when `force` is set — a non-force rm of a
                // running agent is refused by `remove` below, so we must
                // not signal in that case.
                //
                // Signal + registry removal (which also forgets the pid) share
                // one critical section, so a concurrent `Respawn` cannot
                // supersede the agent between them (#138, #140).
                let mut r = self.registry.lock().await;
                if force {
                    self.signal_worker(&r, &id);
                }
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
                    endpoint: Endpoint::Unix {
                        path: self.socket_path.clone(),
                    },
                })
            }
            CtlRequest::Shutdown => CtlReply::ShutdownAck,
            CtlRequest::ReportStatus { id, status } => {
                let mut r = self.registry.lock().await;
                r.report_status(&id, status);
                CtlReply::StatusReported
            }
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
