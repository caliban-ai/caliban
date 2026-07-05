//! Supervisor accept loop + request dispatch.
//!
//! This is the heart of the `caliband` daemon. It binds a Unix domain
//! socket, accepts connections, reads newline-delimited JSON requests,
//! and dispatches them against the [`crate::Registry`].

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
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
    /// Monotonic per-agent port offset (added to `agent_port_base`). Widened
    /// to `AtomicU32` (#280 fix-before-merge) so the offset keeps growing
    /// forever instead of wrapping back to 0 at 65536 — a 16-bit counter
    /// would silently re-hand a low port to a new agent while an earlier
    /// worker holding that port is still alive. With `u32` the `port >
    /// u16::MAX` ceiling in [`Supervisor::next_endpoint`] latches
    /// permanently once tripped: exhaustion becomes a clean, terminal error
    /// (ADR 0051 "clean error, not wrap"), never a collision.
    next_agent_port: AtomicU32,
    /// The actually-bound TCP address (resolves `:0`), published after
    /// `serve` binds so tests / callers can dial an OS-assigned port.
    bound_addr: OnceLock<String>,
    started: Instant,
    registry: Arc<Mutex<Registry>>,
    cancel: CancellationToken,
    /// Per-agent runtime-dir (where per-agent sockets are created).
    agent_runtime_dir: PathBuf,
    /// Workspace root the daemon resolves `SpawnSpec.source` against (#281).
    /// Defaults to `.` when not explicitly set via [`Supervisor::with_workspace_root`].
    workspace_root: PathBuf,
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
            next_agent_port: AtomicU32::new(0),
            bound_addr: OnceLock::new(),
            started: Instant::now(),
            registry,
            cancel: CancellationToken::new(),
            agent_runtime_dir,
            workspace_root: PathBuf::from("."),
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

    /// Set the workspace root the daemon resolves sources against (#281).
    #[must_use]
    pub fn with_workspace_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.workspace_root = root.into();
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
    /// `AtomicU32` offset. **Ceiling (ADR 0051 "clean error, not wrap"):**
    /// once `agent_port_base + offset` exceeds `u16::MAX` this returns
    /// `Internal` — and because the offset is a `u32` that only ever grows,
    /// once tripped the ceiling latches permanently: every subsequent call
    /// keeps returning the same clean error rather than eventually wrapping
    /// back through 0 and re-handing a port a still-live worker is advertising.
    /// (A 16-bit counter would wrap at 65536 and do exactly that.) Monotonic
    /// (not `base + live-index`) so a freshly assigned port can't collide
    /// with a still-draining worker either.
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
                let port = u32::from(net.agent_port_base) + offset;
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
    /// marked `Failed`. `worktree_cleanup` (source dir, worktree name),
    /// when set, is removed alongside the stale per-agent socket once the
    /// worker exits (#281 Task 4).
    async fn launch_and_monitor(
        &self,
        rec: crate::proto::AgentRecord,
        worktree_cleanup: Option<(PathBuf, String)>,
    ) {
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
                    cleanup_worktree(worktree_cleanup); // best-effort, like the socket unlink
                });
            }
            Err(e) => {
                tracing::warn!(error = %e, agent = %id, "worker launch failed");
                self.registry
                    .lock()
                    .await
                    .set_status(&id, crate::proto::AgentStatus::Failed)
                    .ok();
                // The worktree (if any) was already materialized before the
                // launch attempt; a failed launch must not leak it.
                cleanup_worktree(worktree_cleanup);
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
                let id_prefix = uuid::Uuid::new_v4().simple().to_string();
                // Resolve the source directory (None -> workspace root).
                let source_dir = match crate::sources::resolve_source(
                    &self.workspace_root,
                    spec.source.as_deref(),
                ) {
                    Ok(d) => d,
                    Err(e) => {
                        return CtlReply::Error {
                            error: SupervisorError::Internal {
                                message: format!("source resolve: {e}"),
                            },
                        };
                    }
                };
                // Optionally materialize a per-source worktree.
                let (working_dir, worktree_cleanup) = if spec.isolation_worktree {
                    match worktree_for_agent(&source_dir, &id_prefix) {
                        Ok(handle) => {
                            (handle.path.clone(), Some((source_dir.clone(), handle.name)))
                        }
                        Err(e) => {
                            return CtlReply::Error {
                                error: SupervisorError::Internal {
                                    message: format!("worktree create: {e}"),
                                },
                            };
                        }
                    }
                } else {
                    (source_dir.clone(), None)
                };
                // Resolve the endpoint last: source/worktree resolution can
                // fail (bad source, git error, disk full), and a failure
                // there must not have already burned a network port (#281
                // task-4 review, cheap ordering fix).
                let endpoint = match self.next_endpoint() {
                    Ok(e) => e,
                    Err(error) => {
                        cleanup_worktree(worktree_cleanup);
                        return CtlReply::Error { error };
                    }
                };
                let rec = {
                    let mut r = self.registry.lock().await;
                    r.register(spec, endpoint, working_dir)
                };
                let reply = CtlReply::Spawned {
                    id: rec.id.clone(),
                    endpoint: rec.endpoint.clone(),
                };
                self.launch_and_monitor(rec, worktree_cleanup).await;
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
                // Lock 1: read-only lookup of the current spec. Deliberately
                // does NOT remove the old record here (that used to happen
                // in this same critical section, but every fallible step
                // below — source resolution, worktree creation, endpoint
                // allocation — ran AFTER the removal, so any of them failing
                // returned an error having already destroyed the agent with
                // no way back; a failed Respawn is a recovery operation and
                // must never be more destructive than a no-op). The old
                // record stays intact and visible in `list()` until the
                // second critical section below actually supersedes it.
                let old_spec = {
                    let r = self.registry.lock().await;
                    match r.get(&id) {
                        Some(rec) => rec.spec.clone(),
                        None => {
                            return CtlReply::Error {
                                error: SupervisorError::NotFound { id },
                            };
                        }
                    }
                };
                // Resolve the source directory and (optionally) materialize
                // a fresh worktree OUTSIDE any lock — real filesystem/git
                // I/O. Nothing has been removed yet, so any failure here
                // returns cleanly with the original agent still registered.
                let id_prefix = uuid::Uuid::new_v4().simple().to_string();
                let source_dir = match crate::sources::resolve_source(
                    &self.workspace_root,
                    old_spec.source.as_deref(),
                ) {
                    Ok(d) => d,
                    Err(e) => {
                        return CtlReply::Error {
                            error: SupervisorError::Internal {
                                message: format!("source resolve: {e}"),
                            },
                        };
                    }
                };
                let (working_dir, worktree_cleanup) = if old_spec.isolation_worktree {
                    match worktree_for_agent(&source_dir, &id_prefix) {
                        Ok(handle) => {
                            (handle.path.clone(), Some((source_dir.clone(), handle.name)))
                        }
                        Err(e) => {
                            return CtlReply::Error {
                                error: SupervisorError::Internal {
                                    message: format!("worktree create: {e}"),
                                },
                            };
                        }
                    }
                } else {
                    (source_dir.clone(), None)
                };
                // Resolve the endpoint last (same cheap ordering fix as
                // Spawn): a failed source/worktree resolution must not have
                // already burned a network port. If a worktree WAS just
                // materialized and this still fails, it must not leak.
                let endpoint = match self.next_endpoint() {
                    Ok(e) => e,
                    Err(error) => {
                        cleanup_worktree(worktree_cleanup);
                        return CtlReply::Error { error };
                    }
                };
                // Lock 2: remove the old record and register the new one
                // atomically, in the SAME critical section. This is the
                // fix — a concurrent `Kill`/`Rm --force` holding the lock
                // mid-signal still can't observe a torn pid/record state
                // (#115/#140), and the old agent is only ever destroyed once
                // its replacement is guaranteed to take its place.
                let new_rec = {
                    let mut r = self.registry.lock().await;
                    if let Err(e) = r.remove(&id, true) {
                        drop(r);
                        // Lost a race (e.g. a concurrent Respawn/Rm already
                        // removed `id` between Lock 1 and here): don't leak
                        // the worktree we just created for a respawn that
                        // isn't going to happen.
                        cleanup_worktree(worktree_cleanup);
                        return CtlReply::Error { error: e };
                    }
                    r.register(old_spec, endpoint, working_dir)
                };
                let reply = CtlReply::Respawned {
                    id: new_rec.id.clone(),
                };
                self.launch_and_monitor(new_rec, worktree_cleanup).await;
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

/// Best-effort remove a just-created worktree, so a later failure (a launch
/// failure, an exhausted port counter, a lock-ordering race that loses) never
/// leaks it. Mirrors the terminal-worker cleanup in `launch_and_monitor`;
/// shared so `Spawn` and `Respawn` can call it from their own late-failure
/// paths too (#281 task-4 review).
fn cleanup_worktree(cleanup: Option<(PathBuf, String)>) {
    if let Some((source_dir, wt_name)) = cleanup
        && let Ok(mgr) = caliban_worktrees::WorktreeManager::new(&source_dir)
    {
        let _ = mgr.remove(&wt_name, true);
    }
}

/// Create a per-agent git worktree rooted at `source_dir`. Returns the handle
/// (its `.path` is the worktree root). Errors if the source is not a checkout.
fn worktree_for_agent(
    source_dir: &Path,
    agent_name: &str,
) -> Result<caliban_worktrees::WorktreeHandle, caliban_worktrees::WorktreeError> {
    let mgr = caliban_worktrees::WorktreeManager::new(source_dir)?;
    mgr.create(&caliban_worktrees::WorktreeSpec::new(agent_name))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proc::WorkerHandle;
    use crate::proto::AgentRecord;
    use crate::store::AgentStore;

    /// Never actually launches anything — these tests only exercise
    /// `next_endpoint`'s port bookkeeping, not a real worker lifecycle.
    struct NopLauncher;
    impl WorkerLauncher for NopLauncher {
        fn launch(&self, _record: &AgentRecord) -> std::io::Result<WorkerHandle> {
            unreachable!("next_endpoint tests never launch a worker")
        }
    }

    fn test_supervisor(network: NetworkConfig) -> Supervisor {
        let dir = tempfile::tempdir().unwrap();
        let store = AgentStore::new(dir.path().join("data"));
        let agent_dir = dir.path().join("agents-rt");
        let bind = BindSpec {
            endpoint: Endpoint::Tcp {
                addr: "127.0.0.1:0".into(),
            },
            tls: None,
            token: None,
        };
        // Leak the tempdir handle so its directory outlives this short unit
        // test without needing to thread a guard through the return type.
        std::mem::forget(dir);
        Supervisor::with_bind(bind, Some(network), store, agent_dir, Arc::new(NopLauncher))
    }

    /// Regression for the fix-before-merge finding: `next_agent_port` used to
    /// be an `AtomicU16`, so `fetch_add` wrapped at 65536 back to 0 — the
    /// `port > u16::MAX` ceiling caught the exhaustion once, but the very
    /// next call would silently re-hand `agent_port_base` (a port a still-
    /// live worker may already be advertising) to a brand new agent.
    /// Widening the counter to `AtomicU32` must make that ceiling latch
    /// permanently: every call after exhaustion keeps returning the same
    /// clean `Internal` error, never a fresh low port (ADR 0051 "clean
    /// error, not wrap").
    #[test]
    fn agent_port_ceiling_latches_permanently_instead_of_wrapping() {
        let network = NetworkConfig {
            advertise_host: "localhost".into(),
            agent_port_base: u16::MAX, // one call in-range, then exhausted
            agent_tls: None,
            agent_token: None,
        };
        let sup = test_supervisor(network);

        // First call: base itself (65535) is still <= u16::MAX -> Ok.
        let first = sup
            .next_endpoint()
            .expect("first port assignment (the base itself) should succeed");
        match first {
            Endpoint::Tcp { addr } => assert!(addr.ends_with(":65535"), "got {addr}"),
            Endpoint::Unix { .. } => panic!("expected a TCP endpoint in network mode"),
        }

        // Every call after this must keep returning the same clean error —
        // latched, not wrapping back through 0 to re-hand a live port.
        for i in 0..5 {
            match sup.next_endpoint() {
                Err(SupervisorError::Internal { message }) => {
                    assert!(
                        message.contains("exhausted"),
                        "call {i}: got message {message:?}"
                    );
                }
                other => panic!("call {i}: expected a latched Internal error, got {other:?}"),
            }
        }
    }

    /// `worktree_for_agent` (#281 Task 4) materializes a real git worktree
    /// under `<source>/.caliban/worktrees/<name>` given a real git checkout.
    #[test]
    fn worktree_for_agent_materializes_under_source() {
        let repo = tempfile::tempdir().unwrap();
        // init a real git repo with one commit so HEAD exists
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(repo.path())
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args([
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "--allow-empty",
                "-qm",
                "init",
            ])
            .current_dir(repo.path())
            .status()
            .unwrap();
        let handle = worktree_for_agent(repo.path(), "agent0001").unwrap();
        assert!(handle.path.exists());
        assert!(
            handle
                .path
                .starts_with(repo.path().join(".caliban").join("worktrees"))
        );
    }
}
