//! Supervisor accept loop + request dispatch.
//!
//! This is the heart of the `caliband` daemon. It binds a Unix domain
//! socket, accepts connections, reads newline-delimited JSON requests,
//! and dispatches them against the [`crate::Registry`].

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use tokio::io::{AsyncBufReadExt as _, AsyncWrite, AsyncWriteExt as _, BufReader};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::proc::{OsSignaller, Signaller, WorkerLauncher};
use crate::proto::{CtlReply, CtlRequest, DaemonStatus, SupervisorError};
use crate::registry::Registry;
use crate::store::AgentStore;
use crate::transport::{BindSpec, Endpoint, Listener, TlsServer};

/// Network (TCP) configuration for a supervisor running in server mode
/// (#280 Task 7). Absent (`None` on the [`Supervisor`]) means Unix mode —
/// the historical default, behaviorally unchanged.
///
/// The supervisor owns the pod's network namespace, so it assigns each agent
/// a distinct TCP port from a monotonic counter starting at `agent_port_base`
/// and advertises `"{advertise_host}:{port}"`. `agent_tls`/`agent_token` are
/// the per-agent listener's own TLS + bearer token (symmetric with the control
/// listener), passed down to each worker so prospero's attach is secured.
pub struct NetworkConfig {
    /// Host clients dial to reach per-agent listeners (DNS name or IP).
    pub advertise_host: String,
    /// First TCP port handed to an agent; subsequent agents get base+1, +2, …
    pub agent_port_base: u16,
    /// TLS material each worker uses to secure its own per-agent listener.
    pub agent_tls: Option<TlsServer>,
    /// Bearer token each worker requires on its per-agent listener.
    pub agent_token: Option<String>,
}

/// Per-daemon-process supervisor. Owns the registry, accept loop, and
/// graceful-shutdown token.
pub struct Supervisor {
    /// How the control listener binds (endpoint + optional TLS + optional
    /// token). Unix by default; TCP in network mode (#280 Task 7).
    bind: BindSpec,
    /// Network config when running in TCP server mode; `None` = Unix mode.
    network: Option<NetworkConfig>,
    /// Monotonic per-agent port offset (added to `agent_port_base`). An
    /// `AtomicU16` gives ~64k distinct assignments before the range is
    /// exhausted; see the ceiling note in [`Supervisor::next_endpoint`].
    next_agent_port: AtomicU16,
    /// The actually-bound TCP address (resolves `:0`), published after
    /// `serve` binds so tests / callers can dial an OS-assigned port.
    bound_addr: OnceLock<String>,
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

    /// Construct a Unix-mode supervisor with an explicit worker launcher
    /// (tests inject a fake here so the daemon lifecycle runs without an LLM).
    ///
    /// Convenience wrapper over [`Supervisor::with_bind`]: builds a Unix
    /// [`BindSpec`] and `network: None`.
    pub fn with_launcher(
        socket_path: impl Into<PathBuf>,
        store: AgentStore,
        agent_runtime_dir: impl Into<PathBuf>,
        launcher: Arc<dyn WorkerLauncher>,
    ) -> Self {
        let bind = BindSpec {
            endpoint: Endpoint::Unix {
                path: socket_path.into(),
            },
            tls: None,
            token: None,
        };
        Self::with_bind(bind, None, store, agent_runtime_dir, launcher)
    }

    /// Construct a supervisor from an explicit [`BindSpec`] and optional
    /// [`NetworkConfig`] (#280 Task 7). `network: None` with a Unix `bind` is
    /// today's default; a TCP `bind` + `Some(NetworkConfig)` turns on network
    /// server mode. The launcher is passed in fully configured (the binary
    /// wires per-agent TLS/token/control-endpoint onto an `ExecWorkerLauncher`
    /// before handing it here).
    pub fn with_bind(
        bind: BindSpec,
        network: Option<NetworkConfig>,
        store: AgentStore,
        agent_runtime_dir: impl Into<PathBuf>,
        launcher: Arc<dyn WorkerLauncher>,
    ) -> Self {
        let agent_runtime_dir = agent_runtime_dir.into();
        let registry = Arc::new(Mutex::new(Registry::new(store)));
        Self {
            bind,
            network,
            next_agent_port: AtomicU16::new(0),
            bound_addr: OnceLock::new(),
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

    /// Control socket path, when the control listener is a Unix socket.
    /// `None` in network (TCP) mode (#280 Task 7).
    pub fn socket_path(&self) -> Option<&Path> {
        match &self.bind.endpoint {
            Endpoint::Unix { path } => Some(path.as_path()),
            Endpoint::Tcp { .. } => None,
        }
    }

    /// The control listener's endpoint (Unix path or TCP `host:port`).
    pub fn control_endpoint(&self) -> &Endpoint {
        &self.bind.endpoint
    }

    /// The actually-bound TCP address (resolves an OS-assigned `:0` port),
    /// published once `serve` has bound the listener. `None` before bind, or
    /// for a Unix control listener.
    pub fn bound_addr(&self) -> Option<String> {
        self.bound_addr.get().cloned()
    }

    /// Assign the next per-agent [`Endpoint`]: a Unix socket path (Unix mode)
    /// or a monotonically-numbered TCP endpoint (network mode).
    ///
    /// Network mode draws ports from `agent_port_base` upward via an
    /// `AtomicU16` offset. **Ceiling (QA note, ADR 0051 "Revisit if"):** the
    /// counter is 16-bit, so a very long-lived daemon spawning more than
    /// ~64k agents over its lifetime would exhaust the range and this returns
    /// `Internal`. Acceptable for the MVP (agent lifetimes are bounded; warm-
    /// pool reuse is a later refinement). Monotonic (not `base + live-index`)
    /// so a freshly assigned port can't collide with a still-draining worker.
    fn next_endpoint(&self) -> Result<Endpoint, SupervisorError> {
        match &self.network {
            None => {
                let id_prefix = uuid::Uuid::new_v4().simple().to_string();
                let socket_name = format!("{}-agent.sock", &id_prefix[..8]);
                Ok(Endpoint::Unix {
                    path: self.agent_runtime_dir.join(socket_name),
                })
            }
            Some(net) => {
                let offset = self.next_agent_port.fetch_add(1, Ordering::Relaxed);
                let port = u32::from(net.agent_port_base) + u32::from(offset);
                if port > u32::from(u16::MAX) {
                    return Err(SupervisorError::Internal {
                        message: "agent port range exhausted".into(),
                    });
                }
                Ok(Endpoint::Tcp {
                    addr: format!("{}:{}", net.advertise_host, port),
                })
            }
        }
    }

    /// Bind the socket and accept clients until `cancel_token()` fires.
    /// Returns when the cancellation fires.
    pub async fn serve(self: Arc<Self>) -> std::io::Result<()> {
        if let Some(parent) = self.agent_runtime_dir.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::create_dir_all(&self.agent_runtime_dir).await?;

        // `Listener::bind`'s Unix arm creates the parent dir and unlinks a
        // stale socket from a previous run; its TCP arm binds the address in
        // `self.bind` (network mode, #280 Task 7).
        let listener = Listener::bind(&self.bind).await?;
        // Publish the real bound address (resolves an OS-assigned `:0` port)
        // so callers/tests can dial it. Set-once; ignore a redundant re-bind.
        if let Some(addr) = listener.local_addr() {
            let _ = self.bound_addr.set(addr);
        }
        tracing::info!(endpoint = ?self.bind.endpoint, "supervisor listening");

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
                        Ok(conn) => {
                            let me = Arc::clone(&self);
                            tokio::spawn(async move {
                                if let Err(e) = me.handle_client(conn).await {
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

        // Best-effort cleanup: only a Unix control socket leaves a filesystem
        // artifact to unlink. A TCP listener has nothing to clean up here.
        if let Endpoint::Unix { path } = &self.bind.endpoint {
            let _ = tokio::fs::remove_file(path).await;
        }
        Ok(())
    }

    async fn handle_client(
        self: Arc<Self>,
        conn: crate::transport::BoxConn,
    ) -> std::io::Result<()> {
        let (read_half, mut write_half) = tokio::io::split(conn);
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
                let endpoint = match self.next_endpoint() {
                    Ok(e) => e,
                    Err(error) => return CtlReply::Error { error },
                };
                let rec = {
                    let mut r = self.registry.lock().await;
                    r.register(spec, endpoint)
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
                    let endpoint = match self.next_endpoint() {
                        Ok(e) => e,
                        Err(error) => return CtlReply::Error { error },
                    };
                    r.register(old.spec, endpoint)
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
                    endpoint: self.bind.endpoint.clone(),
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

async fn write_reply<W: AsyncWrite + Unpin>(
    stream: &mut W,
    reply: &CtlReply,
) -> std::io::Result<()> {
    let mut body = serde_json::to_vec(reply).map_err(std::io::Error::other)?;
    body.push(b'\n');
    stream.write_all(&body).await?;
    stream.flush().await?;
    Ok(())
}
