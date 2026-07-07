//! The `caliban __agent-worker` entry point (ADR 0037, #71).
//!
//! Launched by the `caliband` supervisor as a child process. Reads the
//! agent's manifest, binds the per-agent socket, runs the agent loop to
//! completion, and exits with a code the supervisor maps to a terminal
//! status (0 = Done, non-zero = Failed).

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use caliban_agent_core::{InputProvider, RunSettings};
use caliban_supervisor::proto::AgentRecord;
use clap::Parser as _;
use futures::StreamExt as _;
use tokio::io::AsyncWriteExt as _;
use tokio::sync::{Mutex as AsyncMutex, broadcast, mpsc};

use crate::attach::AttachInbound;

/// Fan-out hub for a worker's `TurnEvent` NDJSON stream. Holds the full
/// history of serialized event lines plus a broadcast channel for live
/// delivery. The lock makes "append + broadcast" and "snapshot + subscribe"
/// atomic with respect to each other, so a client that attaches mid-run
/// receives every event exactly once (no gap between the historical
/// snapshot and the live tail).
/// Maximum number of event lines the hub retains for replay. Matches the
/// broadcast channel capacity so an attaching client replays at most the same
/// window the live channel can buffer. Bounds per-worker memory on a
/// long-running session — without this the history grew unbounded. (#116)
const HISTORY_CAP: usize = 1024;

struct EventHub {
    history: Mutex<std::collections::VecDeque<Arc<str>>>,
    tx: broadcast::Sender<Arc<str>>,
}

impl EventHub {
    fn new() -> Arc<Self> {
        let (tx, _rx) = broadcast::channel(1024);
        Arc::new(Self {
            history: Mutex::new(std::collections::VecDeque::with_capacity(HISTORY_CAP)),
            tx,
        })
    }

    /// Append a serialized event line (NO trailing newline) to history and
    /// broadcast it to live subscribers, atomically.
    fn publish(&self, line: Arc<str>) {
        // Recover a poisoned guard instead of panicking: a panic elsewhere
        // while holding this lock must not cascade into worker death, just
        // because the history Vec was left in a consistent-enough state. (#113)
        let mut hist = self
            .history
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Ring buffer: drop the oldest line once at capacity so a long-running
        // worker's history stays bounded. (#116)
        if hist.len() == HISTORY_CAP {
            hist.pop_front();
        }
        hist.push_back(Arc::clone(&line));
        // Err just means there are no live subscribers right now — fine.
        let _ = self.tx.send(line);
    }

    /// Atomically snapshot the history and subscribe for live events. The
    /// returned receiver yields only events published AFTER this call;
    /// combined with the snapshot, the caller sees each event exactly once.
    fn subscribe(&self) -> (Vec<Arc<str>>, broadcast::Receiver<Arc<str>>) {
        // Recover a poisoned guard instead of panicking (see `publish`). (#113)
        let hist = self
            .history
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let rx = self.tx.subscribe();
        (hist.iter().cloned().collect(), rx)
    }
}

/// Abstraction for reporting Idle/Running transitions to the daemon.
/// Best-effort: errors are swallowed so a control-socket hiccup never
/// fails the run. (#81)
#[async_trait::async_trait]
trait StatusSink: Send + Sync {
    async fn set(&self, status: caliban_supervisor::proto::AgentStatus);
}

/// Reports Idle/Running to the daemon control socket (best-effort).
struct ControlSocketStatus {
    client: caliban_supervisor::SupervisorClient,
    id: String,
}

#[async_trait::async_trait]
impl StatusSink for ControlSocketStatus {
    async fn set(&self, status: caliban_supervisor::proto::AgentStatus) {
        let _ = self.client.report_status(&self.id, status).await; // best-effort
    }
}

/// Increments the attached-client count for its lifetime (#81 ticket 5).
struct ClientCountGuard(Arc<AtomicUsize>);
impl ClientCountGuard {
    fn new(counter: Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self(counter)
    }
}
impl Drop for ClientCountGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// `InputProvider` backed by the per-agent socket's inbound frames.
///
/// All attach connections feed one shared mpsc (`inbox`); the run awaits here
/// at each end-of-run boundary. `UserMessage` resumes; `EndInput` or a closed
/// channel ends the run. (#81 ticket 2; idle-timeout with client tracking is
/// ticket 5.)
struct SocketInputProvider {
    inbox: AsyncMutex<mpsc::Receiver<AttachInbound>>,
    /// Optional status reporter — reports Idle before awaiting, Running on resume.
    status: Option<Arc<dyn StatusSink>>,
    /// Idle timeout when no client is attached. `None` = idle indefinitely.
    /// (#81 ticket 5)
    idle_timeout: Option<std::time::Duration>,
    /// Shared count of currently attached clients. (#81 ticket 5)
    has_clients: Arc<AtomicUsize>,
}

/// Idle poll cadence: how often `next_input` wakes to observe a client
/// attaching mid-idle (and reset the deadline). The timeout itself fires on a
/// `sleep_until(deadline)`, so this interval bounds attach-detection latency,
/// not timeout precision. (#119)
const IDLE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

#[async_trait::async_trait]
impl InputProvider for SocketInputProvider {
    async fn next_input(
        &self,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Option<Vec<caliban_provider::Message>> {
        if let Some(s) = &self.status {
            s.set(caliban_supervisor::proto::AgentStatus::Idle).await;
        }
        let mut rx = self.inbox.lock().await;
        // Track an absolute deadline rather than accumulating elapsed time in
        // discrete ticks: chunked accumulation overshot a custom timeout that
        // isn't a multiple of the poll interval by up to one poll (e.g. a 7s
        // timeout fired at ~10s). We still poll at `POLL` so a client attaching
        // mid-idle is observed and the deadline reset, but `sleep_until` wakes
        // exactly at the deadline when it is nearer than the next poll. (#119)
        let mut deadline = self
            .idle_timeout
            .map(|limit| tokio::time::Instant::now() + limit);
        let out = loop {
            let wake = match deadline {
                Some(d) => d.min(tokio::time::Instant::now() + IDLE_POLL_INTERVAL),
                None => tokio::time::Instant::now() + IDLE_POLL_INTERVAL,
            };
            tokio::select! {
                () = cancel.cancelled() => break None,
                frame = rx.recv() => break match frame {
                    Some(AttachInbound::UserMessage { text }) =>
                        Some(vec![caliban_provider::Message::user_text(text)]),
                    Some(AttachInbound::EndInput) | None => None,
                },
                () = tokio::time::sleep_until(wake) => {
                    if self.has_clients.load(Ordering::Relaxed) > 0 {
                        // operator present — reset the countdown
                        deadline = self
                            .idle_timeout
                            .map(|limit| tokio::time::Instant::now() + limit);
                        continue;
                    }
                    if let Some(d) = deadline
                        && tokio::time::Instant::now() >= d
                    {
                        tracing::info!(
                            "interactive agent idle timeout with no clients — ending"
                        );
                        break None;
                    }
                    // No timeout configured (or not yet reached) — keep polling.
                }
            }
        };
        if out.is_some()
            && let Some(s) = &self.status
        {
            s.set(caliban_supervisor::proto::AgentStatus::Running).await;
        }
        out
    }
}

/// Read inbound `AttachInbound` NDJSON frames from `reader` and forward them
/// to `inbox`. Malformed lines are skipped. Returns on EOF or inbox closed.
async fn read_inbound_frames<R>(reader: R, inbox: mpsc::Sender<AttachInbound>)
where
    R: tokio::io::AsyncRead + Unpin + Send,
{
    use tokio::io::AsyncBufReadExt as _;
    use tokio::sync::mpsc::error::TrySendError;
    let mut lines = tokio::io::BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(frame) = serde_json::from_str::<AttachInbound>(&line) {
            // Non-blocking forward: the receiver only drains between runs, so a
            // blocking `send().await` would let an operator flooding frames
            // while the agent is busy wedge this reader task indefinitely.
            // Policy: drop the frame (and warn) when the inbox is full; stop on
            // a closed channel. (#118)
            match inbox.try_send(frame) {
                Ok(()) => {}
                Err(TrySendError::Full(dropped)) => {
                    tracing::warn!(?dropped, "inbound frame channel full — dropping frame");
                }
                Err(TrySendError::Closed(_)) => break,
            }
        }
        // Malformed lines are silently skipped.
    }
}

/// Parse an idle-timeout from an optional env-var string value (pure, testable).
///
/// `None` (var absent) → 5 min (300 s) default.
/// `"0"` → `None` (disabled, idle forever).
/// `"<N>"` (positive integer) → `Some(N seconds)`.
/// Anything else (malformed) → 5 min default.
fn parse_idle_timeout(val: Option<&str>) -> Option<std::time::Duration> {
    match val {
        None => Some(std::time::Duration::from_mins(5)),
        Some(s) => match s.trim().parse::<u64>() {
            Ok(0) => None,
            Ok(secs) => Some(std::time::Duration::from_secs(secs)),
            Err(_) => Some(std::time::Duration::from_mins(5)),
        },
    }
}

/// Idle timeout for an interactive worker awaiting operator input with no
/// client attached (#81 ticket 5). Default 300s; `CALIBAN_AGENT_IDLE_TIMEOUT_SECS`
/// overrides; `0` disables the timeout (idle forever — bounded only by Kill).
fn worker_idle_timeout() -> Option<std::time::Duration> {
    let val = std::env::var("CALIBAN_AGENT_IDLE_TIMEOUT_SECS").ok();
    parse_idle_timeout(val.as_deref())
}

/// Parse a `SpawnSpec.provider` string into a `ProviderKind` (case-
/// insensitive). Returns `None` for an unknown/empty provider. (#93)
fn parse_provider(s: &str) -> Option<crate::args::ProviderKind> {
    use clap::ValueEnum as _;
    crate::args::ProviderKind::from_str(s, true).ok()
}

/// Load the `AgentRecord` the supervisor wrote for this worker.
pub(crate) fn load_record(manifest: &Path) -> std::io::Result<AgentRecord> {
    let body = std::fs::read(manifest)?;
    serde_json::from_slice(&body).map_err(std::io::Error::other)
}

/// Load the worker's per-agent listener TLS from the environment (#280 Task
/// 7). The supervisor sets `CALIBAN_AGENT_TLS_CERT`/`_KEY` to PEM file paths;
/// returns `None` (plaintext) when either is unset. A malformed/unreadable
/// PEM is a hard error so a misconfigured TLS deployment fails loudly rather
/// than silently downgrading to plaintext.
fn load_agent_tls() -> std::io::Result<Option<caliban_supervisor::transport::TlsServer>> {
    let (Ok(cert_path), Ok(key_path)) = (
        std::env::var("CALIBAN_AGENT_TLS_CERT"),
        std::env::var("CALIBAN_AGENT_TLS_KEY"),
    ) else {
        return Ok(None);
    };
    let cert_pem = std::fs::read(&cert_path)?;
    let key_pem = std::fs::read(&key_path)?;
    caliban_supervisor::transport::tls_server_from_pem(&cert_pem, &key_pem).map(Some)
}

/// Fail-closed credential policy for the TCP session plane (#288).
///
/// A `--listen` (network) agent must never bind unauthenticated, and must never
/// carry a bearer token over plaintext (an on-path observer could steal it —
/// the same risk #280 guards on the client dial). Returns `Err(reason)` unless
/// **both** a non-empty token and TLS are configured. Empty/whitespace-only
/// tokens are treated as absent. Unix (`--socket`) mode does not use this — it
/// is local and filesystem-permission-scoped.
fn require_network_credentials(token: Option<&str>, tls_present: bool) -> Result<(), String> {
    let token = token.map(str::trim).filter(|t| !t.is_empty());
    if token.is_none() {
        return Err(
            "CALIBAN_AGENT_TOKEN is required for --listen (network) mode; \
                    refusing to bind an unauthenticated listener"
                .to_owned(),
        );
    }
    if !tls_present {
        return Err(
            "agent TLS (CALIBAN_AGENT_TLS_CERT/KEY) is required for --listen mode; \
                    refusing to send the bearer token over plaintext"
                .to_owned(),
        );
    }
    Ok(())
}

/// Build the client the worker uses to report Idle/Running back to the daemon.
///
/// Network mode (#280 Task 7): when `CALIBAN_CONTROL_ENDPOINT` (`host:port`)
/// is set, dial the daemon over TCP, optionally with TLS
/// (`CALIBAN_CONTROL_TLS_CA` PEM path + `CALIBAN_CONTROL_TLS_SERVER_NAME`,
/// default `localhost`) and a bearer token (`CALIBAN_CONTROL_TOKEN`, falling
/// back to `CALIBAN_AGENT_TOKEN`). Otherwise fall back to the Unix
/// `--control-socket`.
///
/// Security posture (#280 fix-before-merge): `CALIBAN_CONTROL_TLS_CA` unset
/// is an intentional plaintext choice and stays best-effort. But a CA that
/// IS set and fails to load (unreadable file or unparseable PEM) must not
/// silently downgrade to a plaintext dial while still carrying the bearer
/// token — that would leak it to an on-path observer. So that case is
/// `Err`; the caller logs it and disables the (non-critical) status sink
/// entirely rather than ever building a plaintext+token client.
///
/// NOTE (QA): the TCP status path is wired but not exercised by the Task 7
/// deliverable test (which uses a fake launcher). It needs end-to-end QA.
fn build_status_client(
    control_socket: Option<&Path>,
) -> Result<Option<caliban_supervisor::SupervisorClient>, String> {
    if let Ok(endpoint) = std::env::var("CALIBAN_CONTROL_ENDPOINT") {
        let token = std::env::var("CALIBAN_CONTROL_TOKEN")
            .or_else(|_| std::env::var("CALIBAN_AGENT_TOKEN"))
            .ok();
        let tls = match std::env::var("CALIBAN_CONTROL_TLS_CA").ok() {
            None => None,
            Some(ca) => {
                let server_name = std::env::var("CALIBAN_CONTROL_TLS_SERVER_NAME")
                    .unwrap_or_else(|_| "localhost".to_string());
                let ca_pem = std::fs::read(&ca).map_err(|e| {
                    format!("CALIBAN_CONTROL_TLS_CA is set but could not be read ({ca}): {e}")
                })?;
                let client =
                    caliban_supervisor::transport::tls_client_from_pem(&ca_pem, &server_name)
                        .map_err(|e| {
                            format!("CALIBAN_CONTROL_TLS_CA is set but could not be loaded: {e}")
                        })?;
                Some(client)
            }
        };
        return Ok(Some(caliban_supervisor::SupervisorClient::new_tcp(
            endpoint, tls, token,
        )));
    }
    Ok(control_socket.map(caliban_supervisor::SupervisorClient::new))
}

/// Entry point body. Returns the process exit code.
///
/// Exactly one of `socket` (Unix mode) or `listen` (TCP network mode, #280
/// Task 7) must be `Some`.
#[allow(clippy::too_many_lines)]
pub(crate) async fn run(
    manifest: &Path,
    socket: Option<&Path>,
    listen: Option<&str>,
    control_socket: Option<&Path>,
) -> i32 {
    let record = match load_record(manifest) {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "[caliban __agent-worker] cannot read manifest {}: {e}",
                manifest.display()
            );
            return 70; // EX_SOFTWARE
        }
    };

    // --- Create the event hub before binding the socket so the accept task
    // can clone it immediately.
    let hub = EventHub::new();

    // --- Build the per-agent listener BindSpec. Unix mode (`--socket`) binds
    // a filesystem socket whose existence signals liveness; TCP mode
    // (`--listen`, #280 Task 7) binds a network socket secured with the
    // per-agent TLS + token the supervisor passed via env.
    let bind = match (listen, socket) {
        (Some(addr), _) => {
            let tls = match load_agent_tls() {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("[caliban __agent-worker] agent TLS load failed: {e}");
                    return 74; // EX_IOERR
                }
            };
            let token = std::env::var("CALIBAN_AGENT_TOKEN").ok();
            // #288: fail closed — never bind a network listener that is
            // unauthenticated or would carry the bearer token over plaintext.
            if let Err(reason) = require_network_credentials(token.as_deref(), tls.is_some()) {
                eprintln!("[caliban __agent-worker] refusing to bind {addr}: {reason}");
                return 78; // EX_CONFIG
            }
            caliban_supervisor::transport::BindSpec {
                endpoint: caliban_supervisor::transport::Endpoint::Tcp {
                    addr: addr.to_string(),
                },
                tls,
                token,
            }
        }
        (None, Some(path)) => caliban_supervisor::transport::BindSpec {
            endpoint: caliban_supervisor::transport::Endpoint::Unix {
                path: path.to_path_buf(),
            },
            tls: None,
            token: None,
        },
        (None, None) => {
            eprintln!("[caliban __agent-worker] one of --socket or --listen is required");
            return 64; // EX_USAGE
        }
    };
    let bind_desc = format!("{:?}", bind.endpoint);
    let listener = match caliban_supervisor::transport::Listener::bind(&bind).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[caliban __agent-worker] bind {bind_desc} failed: {e}");
            return 74; // EX_IOERR
        }
    };
    // --- Shared attached-client counter for idle-timeout tracking (#81 ticket 5).
    // Every accept connection increments this via ClientCountGuard and
    // decrements on drop, so SocketInputProvider can reset its idle timer
    // while at least one operator is watching.
    let has_clients = Arc::new(AtomicUsize::new(0));

    // --- Set up the optional inbound inbox for interactive mode. ---
    // When interactive, every attach connection feeds inbound AttachInbound
    // frames into a shared mpsc; the SocketInputProvider awaits it at the
    // end-of-run boundary instead of finishing. `inbox_keepalive` is kept
    // alive in run() so the channel stays open while no clients are attached.
    // An idle timeout (#81 ticket 5) ends the run after no client is attached
    // for `worker_idle_timeout()` seconds.
    let (inbox_keepalive, input_source): (
        Option<mpsc::Sender<AttachInbound>>,
        Option<Arc<dyn InputProvider>>,
    ) = if record.spec.interactive {
        let (tx, rx) = mpsc::channel::<AttachInbound>(64);
        // Build a status sink from the daemon control plane, if reachable:
        // the network endpoint (`CALIBAN_CONTROL_ENDPOINT`, #280 Task 7) takes
        // precedence over the Unix `--control-socket`. A misconfigured
        // control-TLS CA (set but unreadable/unparseable) is a hard `Err`
        // from `build_status_client` — never a plaintext dial carrying the
        // bearer token. Status reporting is non-critical, so we just log and
        // disable the sink rather than failing the whole run.
        let status_sink: Option<Arc<dyn StatusSink>> = match build_status_client(control_socket) {
            Ok(client_opt) => client_opt.map(|client| -> Arc<dyn StatusSink> {
                Arc::new(ControlSocketStatus {
                    client,
                    id: record.id.clone(),
                })
            }),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "control-plane status client misconfigured; disabling status reporting"
                );
                None
            }
        };
        let provider: Arc<dyn InputProvider> = Arc::new(SocketInputProvider {
            inbox: AsyncMutex::new(rx),
            status: status_sink,
            idle_timeout: worker_idle_timeout(),
            has_clients: Arc::clone(&has_clients),
        });
        (Some(tx), Some(provider))
    } else {
        (None, None)
    };

    // Accept loop: replay history then forward live events to each client.
    // When interactive, also spawn a read-half task per connection that
    // forwards inbound AttachInbound frames to the shared inbox.
    let accept_hub = Arc::clone(&hub);
    let accept_inbox_tx = inbox_keepalive.clone();
    let accept_has_clients = Arc::clone(&has_clients);
    tokio::spawn(run_agent_accept_loop(
        listener,
        accept_hub,
        accept_inbox_tx,
        accept_has_clients,
    ));
    // Keep the sender alive so the channel doesn't close between connections.
    let _ = &inbox_keepalive;

    // --- Build the agent. ---
    let _ = tokio::fs::create_dir_all(&record.session_dir).await;
    let ndjson_path = record
        .session_dir
        .join(caliban_supervisor::store::TRANSCRIPT_FILE);

    // Construct minimal Args with the spec's model override.
    // Args does not impl Default, so we parse a minimal invocation.
    //
    // Permission posture (#75 / #84): the worker runs with `--bare` (no TUI/MCP)
    // and a real permission gate. When `spec.inherit_hooks` is true AND
    // `spec.inherited_hooks_config` carries a valid `InheritableHookConfig`,
    // the gate is rebuilt from the parent's rules+mode+audit (#84). Otherwise
    // the binary-default gate (#75) is used: `spec.tool_allowlist` seeds Allow
    // rules that precede the `default_rules()` tail (read-only Allow;
    // Bash/Write/Edit Ask → denied non-interactively because `auto_allow: false`).
    // `tool_allowlist` registry filtering applies in both paths.
    let mut args = match crate::args::Args::try_parse_from(["caliban", "--bare"]) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("[caliban __agent-worker] args construction failed: {e}");
            return 70;
        }
    };
    // Select the provider from the spawn spec (#93). Must precede provider
    // construction and model defaulting (default_model_for keys off it).
    if let Some(p) = record.spec.provider.as_deref() {
        if let Some(pk) = parse_provider(p) {
            args.provider = Some(pk);
        } else {
            eprintln!("[caliban __agent-worker] unknown provider {p:?}");
            return 64; // EX_USAGE
        }
    }
    if let Some(model) = record.spec.model.clone() {
        args.model = Some(model);
    }

    let pool = Arc::new(caliban_settings::ApiKeyHelperPool::from_raw(None));
    let provider = match crate::startup::build_provider(&args, &pool) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[caliban __agent-worker] provider: {e}");
            return 1;
        }
    };

    // Resolve model string exactly as main.rs does.
    let model = args
        .model
        .clone()
        .unwrap_or_else(|| crate::default_model_for(crate::resolved_provider(&args)).to_string());

    let workspace = caliban_tools_builtin::WorkspaceRoot::current_dir()
        .unwrap_or_else(|_| caliban_tools_builtin::WorkspaceRoot::new(record.session_dir.clone()));
    let todos = caliban_agent_core::new_shared_todos();
    let plan_mode = caliban_agent_core::new_shared_plan_mode();
    let registry = crate::startup::build_registry(
        &args,
        workspace,
        Arc::clone(&todos),
        Arc::clone(&plan_mode),
        &[],
    );
    let registry = filter_registry(registry, record.spec.tool_allowlist.as_deref());

    // Permission gate: inherit the parent's policy when asked + available
    // (#84), else the binary-default gate (#75).
    let inherited = if record.spec.inherit_hooks {
        record
            .spec
            .inherited_hooks_config
            .as_deref()
            .and_then(crate::hook_inherit::InheritableHookConfig::from_json)
    } else {
        None
    };
    let permissions: Arc<dyn caliban_agent_core::Hooks + Send + Sync> = if let Some(cfg) = inherited
    {
        build_inherited_hooks(cfg, &provider, &model, record.id.clone())
    } else {
        // #75 default gate (unchanged).
        let rules = build_worker_rules(record.spec.tool_allowlist.as_deref());
        let ask: Arc<dyn caliban_agent_core::AskHandler> =
            Arc::new(caliban_agent_core::NonInteractiveAskHandler { auto_allow: false });
        Arc::new(caliban_agent_core::PermissionsHook::new(
            rules,
            ask,
            Arc::new(caliban_agent_core::NoopHooks),
        ))
    };

    let agent = Arc::new(
        caliban_agent_core::Agent::builder()
            .provider(provider)
            .tools(registry)
            .config(caliban_agent_core::AgentConfig {
                model: model.clone(),
                ..caliban_agent_core::AgentConfig::default()
            })
            .hooks(permissions)
            .build()
            .expect("worker agent builder"),
    );

    // --- Drive the agent loop. ---
    let messages = vec![caliban_provider::Message::user_text(
        record.spec.initial_prompt.clone(),
    )];
    let cancel = tokio_util::sync::CancellationToken::new();
    let mut stream = if let Some(provider) = input_source {
        // Interactive mode: await inbound messages at each end-of-run boundary.
        agent.stream_until_done_with_settings(
            messages,
            cancel,
            RunSettings {
                input_source: Some(provider),
                ..RunSettings::default()
            },
        )
    } else {
        // Non-interactive (default): run to completion exactly as before.
        agent.stream_until_done(messages, cancel)
    };

    let mut ndjson = match tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&ndjson_path)
        .await
    {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[caliban __agent-worker] open ndjson: {e}");
            return 74;
        }
    };

    let mut stop = caliban_agent_core::StopCondition::EndOfTurn;
    while let Some(event) = stream.next().await {
        match event {
            Ok(ev) => {
                // Capture the terminal stop condition from RunEnd.
                if let caliban_agent_core::TurnEvent::RunEnd {
                    ref stopped_for, ..
                } = ev
                {
                    stop = stopped_for.clone();
                }
                // Write the full event as one NDJSON line. TurnEvent
                // derives Serialize with an internal `"type"` tag (#78), so
                // the `agents attach` client can read these back verbatim.
                if let Ok(json) = serde_json::to_string(&ev) {
                    // Persist to stdout.ndjson (newline-delimited).
                    let mut line = json.clone().into_bytes();
                    line.push(b'\n');
                    let _ = ndjson.write_all(&line).await;
                    // Fan out to any attached clients (no trailing newline;
                    // the client writer adds it).
                    hub.publish(Arc::from(json.as_str()));
                }
            }
            Err(e) => {
                eprintln!("[caliban __agent-worker] stream error: {e}");
                return 1;
            }
        }
    }
    let _ = ndjson.flush().await;
    // Best-effort: let attached clients drain the last events before the
    // process exits. (#79; a graceful connection-join is a future refinement.)
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    crate::startup::stop_condition_exit_code(&stop)
}

/// Build the permission hook chain from a parent's inherited config (#84):
/// `PermissionsHook(inherited rules)` → `ModeFilter(inherited mode)` → audit.
///
/// The ask handler stays non-interactive deny-on-Ask — a background worker
/// has no human attending. The auto-mode classifier is only constructed when
/// the inherited mode is `Auto` (it needs the worker's own provider + model).
fn build_inherited_hooks(
    cfg: crate::hook_inherit::InheritableHookConfig,
    provider: &Arc<dyn caliban_provider::Provider + Send + Sync>,
    model: &str,
    session_id: String,
) -> Arc<dyn caliban_agent_core::Hooks + Send + Sync> {
    use caliban_agent_core::{
        AutoModeClassifier, AutoModeConfig, DEFAULTS_TOKEN, ModeFilter, NonInteractiveAskHandler,
        NoopHooks, PermissionMode, PermissionsHook, SharedPermissionMode,
    };
    let ask: Arc<dyn caliban_agent_core::AskHandler> =
        Arc::new(NonInteractiveAskHandler { auto_allow: false });
    // Re-hydrate the parent's snapshotted runtime rules into a fresh store and
    // wire it into the gate, so a live "Always allow/deny" set in the parent is
    // honored by this inherited worker (consulted before the static rules).
    // (#114)
    let runtime_store = Arc::new(caliban_agent_core::RuntimeRuleStore::new());
    for rule in cfg.runtime_rules {
        runtime_store.add(rule);
    }
    let inner: Arc<dyn caliban_agent_core::Hooks> = Arc::new(
        PermissionsHook::new(cfg.rules, ask, Arc::new(NoopHooks)).with_runtime_rules(runtime_store),
    );

    let mode = SharedPermissionMode::new(cfg.mode);
    // Auto mode needs the LLM classifier; other modes pass `None` which makes
    // auto soft-deny everything via `DisabledFallback` semantics — but that
    // branch is never reached when mode != Auto.
    let classifier = if cfg.mode == PermissionMode::Auto {
        let auto_cfg = AutoModeConfig {
            environment: vec![DEFAULTS_TOKEN.into()],
            allow: vec![DEFAULTS_TOKEN.into()],
            soft_deny: vec![DEFAULTS_TOKEN.into()],
            hard_deny: vec![DEFAULTS_TOKEN.into()],
            disabled: false,
        };
        Some(Arc::new(AutoModeClassifier::new(
            Arc::clone(provider),
            model,
            auto_cfg,
        )))
    } else {
        None
    };
    let filter: Arc<dyn caliban_agent_core::Hooks + Send + Sync> =
        Arc::new(ModeFilter::new(mode, inner, classifier, false));

    crate::startup::wrap_with_audit(filter, cfg.audit, session_id)
}

/// Filter `registry` down to the tools named in `allowlist`. `None` keeps
/// the full registry. Mirrors the sub-agent allowlist pattern in
/// `startup::install_sub_agent`.
fn filter_registry(
    registry: caliban_agent_core::ToolRegistry,
    allowlist: Option<&[String]>,
) -> caliban_agent_core::ToolRegistry {
    let Some(names) = allowlist else {
        return registry;
    };
    let mut filtered = caliban_agent_core::ToolRegistry::new();
    for name in names {
        if let Some(t) = registry.get(name) {
            filtered.register(std::sync::Arc::clone(t));
        }
    }
    filtered
}

/// Build the worker's permission rule list (#75): each tool the spawner
/// granted via `tool_allowlist` is allowed; the binary `default_rules()`
/// tail governs everything else (read-only allowed; Bash/Write/Edit/Web
/// fall to Ask → denied non-interactively). First match wins, so the
/// allowlist grants must precede the default tail.
fn build_worker_rules(allowlist: Option<&[String]>) -> Vec<caliban_agent_core::Rule> {
    let mut rules: Vec<caliban_agent_core::Rule> = Vec::new();
    if let Some(names) = allowlist {
        for tool in names {
            rules.push(caliban_agent_core::Rule {
                tool: tool.clone(),
                action: caliban_agent_core::Action::Allow,
                comment: Some("granted via SpawnSpec tool_allowlist (#75)".into()),
                reason: None,
                expires_at: None,
            });
        }
    }
    rules.extend(caliban_agent_core::default_rules());
    rules
}

/// Per-agent accept loop: spawns [`serve_attach_client`] for every accepted
/// connection, forever.
///
/// In network mode (#280), `listener.accept()` performs a TLS handshake and
/// bearer-token preamble check and returns `Err(io::Error)` (kind
/// `PermissionDenied`, or an I/O error from a failed handshake) for a
/// wrong/missing token or a garbage dial — e.g. a mistyped token or a port
/// scan. That must not be fatal to the loop: a single rejected connection
/// used to permanently disable all future attaches to this agent (the old
/// shape was `while let Ok(conn) = listener.accept().await { … }`, which
/// exits forever on the first `Err`). Instead we log and keep accepting.
async fn run_agent_accept_loop(
    listener: caliban_supervisor::transport::Listener,
    hub: Arc<EventHub>,
    inbox: Option<mpsc::Sender<AttachInbound>>,
    has_clients: Arc<AtomicUsize>,
) {
    loop {
        match listener.accept().await {
            Ok(conn) => {
                let conn_hub = Arc::clone(&hub);
                let conn_inbox = inbox.clone();
                let conn_clients = Arc::clone(&has_clients);
                tokio::spawn(serve_attach_client(
                    conn,
                    conn_hub,
                    conn_inbox,
                    conn_clients,
                ));
            }
            Err(e) => {
                tracing::warn!(error = %e, "per-agent accept failed");
            }
        }
    }
}

/// Serve one attached client: replay the event history, then forward live
/// events until the agent finishes (`Closed`) or the client disconnects.
///
/// When `inbox` is `Some`, the socket is full-duplex: inbound
/// `AttachInbound` NDJSON frames from the client are parsed by a background
/// task and forwarded to the shared inbox (ADR 0047 / #81). When `None`
/// (non-interactive agent), the read half is dropped and ignored — the
/// client can't send.
///
/// `clients` is incremented for the duration of this call via `ClientCountGuard`
/// so the `SocketInputProvider` knows at least one operator is watching and
/// resets its idle timer (#81 ticket 5).
async fn serve_attach_client(
    conn: caliban_supervisor::transport::BoxConn,
    hub: Arc<EventHub>,
    inbox: Option<mpsc::Sender<AttachInbound>>,
    clients: Arc<AtomicUsize>,
) {
    // Hold the guard for the entire lifetime of this connection — even early
    // returns (e.g. write errors in history replay) are covered by Drop.
    let _client_guard = ClientCountGuard::new(clients);

    let (read_half, mut write_half) = tokio::io::split(conn);

    // Spawn the inbound read task when in interactive mode.
    if let Some(tx) = inbox {
        tokio::spawn(read_inbound_frames(read_half, tx));
    }
    // (Non-interactive: read_half is dropped here — the client can't send.)

    let (history, mut rx) = hub.subscribe();
    for line in &history {
        if write_line(&mut write_half, line).await.is_err() {
            return;
        }
    }
    loop {
        match rx.recv().await {
            Ok(line) => {
                if write_line(&mut write_half, &line).await.is_err() {
                    break;
                }
            }
            // A slow client fell behind the 1024-event buffer; keep going
            // with subsequent events rather than dropping the connection.
            Err(broadcast::error::RecvError::Lagged(_)) => {}

            // All senders dropped: the agent run finished and the hub was
            // released. (On the normal path the process exits right after
            // the run, so clients usually EOF via socket teardown before
            // this fires — but handle it cleanly if the hub drops first.)
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
    let _ = write_half.shutdown().await;
}

/// Write one NDJSON line (event + trailing newline) to the client.
async fn write_line<W: tokio::io::AsyncWrite + Unpin>(
    stream: &mut W,
    line: &str,
) -> std::io::Result<()> {
    stream.write_all(line.as_bytes()).await?;
    stream.write_all(b"\n").await
}

#[cfg(test)]
mod tests {
    use super::*;
    use caliban_agent_core::{Action, default_rules};

    // --- require_network_credentials: fail-closed TCP session-plane auth (#288) ---

    #[test]
    fn network_creds_rejects_missing_token() {
        let err = require_network_credentials(None, false).unwrap_err();
        assert!(
            err.contains("CALIBAN_AGENT_TOKEN"),
            "missing token must be rejected with a token-requirement reason, got: {err}"
        );
    }

    #[test]
    fn network_creds_treats_empty_token_as_absent() {
        assert!(
            require_network_credentials(Some(""), true).is_err(),
            "empty token must be treated as absent"
        );
        assert!(
            require_network_credentials(Some("   "), true).is_err(),
            "whitespace-only token must be treated as absent"
        );
    }

    #[test]
    fn network_creds_requires_tls_when_token_present() {
        let err = require_network_credentials(Some("secret"), false).unwrap_err();
        assert!(
            err.to_lowercase().contains("tls"),
            "a token without TLS must be rejected with a TLS/plaintext reason, got: {err}"
        );
    }

    #[test]
    fn network_creds_accepts_token_and_tls() {
        assert!(
            require_network_credentials(Some("secret"), true).is_ok(),
            "a non-empty token with TLS present is the only accepted configuration"
        );
    }

    // --- parse_provider (#93) ---

    #[test]
    fn parse_provider_recognizes_lowercase() {
        assert_eq!(
            parse_provider("ollama"),
            Some(crate::args::ProviderKind::Ollama)
        );
        assert_eq!(
            parse_provider("anthropic"),
            Some(crate::args::ProviderKind::Anthropic)
        );
        assert_eq!(
            parse_provider("openai"),
            Some(crate::args::ProviderKind::Openai)
        );
        assert_eq!(
            parse_provider("google"),
            Some(crate::args::ProviderKind::Google)
        );
    }

    #[test]
    fn parse_provider_is_case_insensitive() {
        assert_eq!(
            parse_provider("OLLAMA"),
            Some(crate::args::ProviderKind::Ollama)
        );
        assert_eq!(
            parse_provider("Anthropic"),
            Some(crate::args::ProviderKind::Anthropic)
        );
    }

    #[test]
    fn parse_provider_returns_none_for_unknown() {
        assert_eq!(parse_provider("bogus"), None);
    }

    #[test]
    fn parse_provider_returns_none_for_empty() {
        assert_eq!(parse_provider(""), None);
    }

    // --- build_worker_rules ---

    #[test]
    fn worker_rules_grant_allowlisted_tools_first() {
        let rules = build_worker_rules(Some(&["Bash".to_string(), "Edit".to_string()]));
        let defaults = default_rules();
        // Total length: 2 allowlist grants + all defaults.
        assert_eq!(
            rules.len(),
            2 + defaults.len(),
            "expected 2 allowlist rules + default tail, got {}",
            rules.len()
        );
        // First two rules are the allowlist grants in order.
        assert_eq!(rules[0].tool, "Bash");
        assert!(
            matches!(rules[0].action, Action::Allow),
            "expected Allow for Bash, got {:?}",
            rules[0].action
        );
        assert_eq!(rules[1].tool, "Edit");
        assert!(
            matches!(rules[1].action, Action::Allow),
            "expected Allow for Edit, got {:?}",
            rules[1].action
        );
        // Remainder equals default_rules() verbatim.
        for (i, (got, want)) in rules[2..].iter().zip(defaults.iter()).enumerate() {
            assert_eq!(
                got.tool, want.tool,
                "default rule [{i}] tool mismatch: {} vs {}",
                got.tool, want.tool
            );
            assert_eq!(
                got.action, want.action,
                "default rule [{i}] action mismatch"
            );
        }
    }

    #[test]
    fn worker_rules_without_allowlist_are_just_defaults() {
        let rules = build_worker_rules(None);
        let defaults = default_rules();
        assert_eq!(rules.len(), defaults.len());
        // Spot-check: Read should be Allow.
        let read_rule = rules.iter().find(|r| r.tool == "Read");
        assert!(read_rule.is_some(), "expected a Read rule in defaults");
        assert!(
            matches!(read_rule.unwrap().action, Action::Allow),
            "Read default should be Allow"
        );
    }

    #[test]
    fn load_record_reads_a_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let store = caliban_supervisor::store::AgentStore::new(dir.path().join("agents"));
        let rec = AgentRecord {
            id: "w1".into(),
            name: "w".into(),
            status: caliban_supervisor::proto::AgentStatus::Spawning,
            started_at: "2026-06-09T00:00:00Z".into(),
            session_dir: store.session_dir("w1"),
            endpoint: caliban_supervisor::Endpoint::Unix {
                path: dir.path().join("w1.sock"),
            },
            working_dir: std::path::PathBuf::new(),
            spec: caliban_supervisor::proto::SpawnSpec {
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
                source: None,
            },
        };
        store.write_manifest(&rec).unwrap();
        let loaded = load_record(&store.session_dir("w1").join("manifest.json")).unwrap();
        assert_eq!(loaded.id, "w1");
        assert_eq!(loaded.spec.initial_prompt, "hi");
    }

    #[test]
    fn event_hub_subscribe_after_publish_replays_history() {
        let hub = EventHub::new();
        hub.publish(Arc::from("a"));
        hub.publish(Arc::from("b"));
        hub.publish(Arc::from("c"));
        let (history, _rx) = hub.subscribe();
        assert_eq!(history.len(), 3);
        assert_eq!(&*history[0], "a");
        assert_eq!(&*history[1], "b");
        assert_eq!(&*history[2], "c");
    }

    #[test]
    fn event_hub_history_is_bounded_to_cap() {
        // A long-running worker emits events indefinitely; history must not
        // grow without bound. Past the cap the oldest lines are dropped, and
        // an attaching client still replays the most-recent retained window.
        let hub = EventHub::new();
        let total = HISTORY_CAP + 50;
        for i in 0..total {
            hub.publish(Arc::from(format!("e{i}").as_str()));
        }
        let (history, _rx) = hub.subscribe();
        assert_eq!(history.len(), HISTORY_CAP, "history must be capped");
        // Newest retained...
        assert_eq!(&*history[history.len() - 1], format!("e{}", total - 1));
        // ...oldest dropped: the window starts at total - HISTORY_CAP.
        assert_eq!(&*history[0], format!("e{}", total - HISTORY_CAP));
    }

    /// Poison the hub's `history` mutex by panicking while holding the guard,
    /// with the panic message suppressed so test output stays clean. A static
    /// guard serializes the process-global panic-hook swap so concurrent
    /// poison tests can't clobber each other's saved hook.
    fn poison_history(hub: &EventHub) {
        static HOOK_GUARD: Mutex<()> = Mutex::new(());
        let _serial = HOOK_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = hub.history.lock().unwrap();
            panic!("intentional poison");
        }));
        std::panic::set_hook(prev);
        assert!(result.is_err(), "poisoning closure should have unwound");
        assert!(
            hub.history.is_poisoned(),
            "history mutex should be poisoned after a panic-while-locked"
        );
    }

    #[test]
    fn event_hub_publish_recovers_from_poisoned_lock() {
        // A panic while holding the history lock poisons it. `publish` must
        // recover the guard rather than panic the worker, so events keep
        // flowing after a poisoned-lock recovery.
        let hub = EventHub::new();
        hub.publish(Arc::from("before"));
        poison_history(&hub);

        hub.publish(Arc::from("after"));

        let (history, _rx) = hub.subscribe();
        assert!(
            history.iter().any(|l| &**l == "after"),
            "publish after poison must append, got {history:?}"
        );
    }

    #[test]
    fn event_hub_subscribe_recovers_from_poisoned_lock() {
        // `subscribe` must also recover a poisoned history lock so an attaching
        // client still receives the retained snapshot.
        let hub = EventHub::new();
        hub.publish(Arc::from("x"));
        poison_history(&hub);

        let (history, _rx) = hub.subscribe();
        assert!(
            history.iter().any(|l| &**l == "x"),
            "subscribe after poison must return the snapshot, got {history:?}"
        );
    }

    #[tokio::test]
    async fn event_hub_no_gap_between_history_and_live() {
        let hub = EventHub::new();
        hub.publish(Arc::from("a"));
        hub.publish(Arc::from("b"));

        // Subscribe: snapshot should include "a" and "b".
        let (history, mut rx) = hub.subscribe();
        assert_eq!(history.len(), 2);
        assert_eq!(&*history[0], "a");
        assert_eq!(&*history[1], "b");

        // Publish after subscribe: receiver should get "c" and "d" only.
        hub.publish(Arc::from("c"));
        hub.publish(Arc::from("d"));

        let first = rx.recv().await.unwrap();
        let second = rx.recv().await.unwrap();
        assert_eq!(&*first, "c");
        assert_eq!(&*second, "d");
    }

    #[test]
    fn event_hub_publish_without_subscribers_is_ok() {
        let hub = EventHub::new();
        // No subscribers — send returns Err but must not panic.
        hub.publish(Arc::from("x"));
        hub.publish(Arc::from("y"));
        let (history, _rx) = hub.subscribe();
        assert_eq!(history.len(), 2);
    }

    // --- read_inbound_frames ---

    #[tokio::test]
    async fn read_inbound_frames_forwards_and_skips_malformed() {
        use tokio::io::AsyncWriteExt as _;
        let (mut writer, reader) = tokio::io::duplex(4096);

        let (tx, mut rx) = mpsc::channel::<AttachInbound>(16);
        let task = tokio::spawn(read_inbound_frames(reader, tx));

        writer
            .write_all(b"{\"type\":\"UserMessage\",\"text\":\"hi\"}\n")
            .await
            .unwrap();
        writer.write_all(b"not valid json at all\n").await.unwrap();
        writer
            .write_all(b"{\"type\":\"EndInput\"}\n")
            .await
            .unwrap();
        drop(writer); // EOF → read_inbound_frames returns

        task.await.unwrap();

        let first = rx.recv().await.expect("first frame");
        assert_eq!(first, AttachInbound::UserMessage { text: "hi".into() });
        let second = rx.recv().await.expect("second frame");
        assert_eq!(second, AttachInbound::EndInput);
        // Channel should be closed (no more frames).
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn read_inbound_frames_does_not_stall_when_channel_full() {
        // 100 valid frames into a 64-cap channel that is never drained while
        // the reader runs. With the old blocking send().await the reader wedged
        // at frame 65; try_send must drop the overflow and drain to EOF. (#118)
        let mut input = Vec::new();
        for i in 0..100 {
            input.extend_from_slice(
                format!("{{\"type\":\"UserMessage\",\"text\":\"m{i}\"}}\n").as_bytes(),
            );
        }
        let (tx, mut rx) = mpsc::channel::<AttachInbound>(64);

        // `rx` stays alive (channel open) but is NOT drained during the run, so
        // the channel fills and try_send must reject overflow rather than block.
        let res = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            read_inbound_frames(input.as_slice(), tx),
        )
        .await;
        assert!(res.is_ok(), "reader stalled on a full inbox channel");

        // Exactly the cap was buffered; the overflow was dropped, not blocked.
        let mut delivered = 0;
        while rx.try_recv().is_ok() {
            delivered += 1;
        }
        assert_eq!(delivered, 64, "should buffer exactly the channel capacity");
    }

    // --- SocketInputProvider ---

    #[tokio::test]
    async fn socket_input_provider_resumes_then_ends() {
        use tokio_util::sync::CancellationToken;

        let (tx, rx) = mpsc::channel::<AttachInbound>(16);
        let provider = SocketInputProvider {
            inbox: AsyncMutex::new(rx),
            status: None,
            idle_timeout: None,
            has_clients: Arc::new(AtomicUsize::new(0)),
        };
        let cancel = CancellationToken::new();

        // Send a UserMessage → next_input returns Some([user "go"]).
        tx.send(AttachInbound::UserMessage { text: "go".into() })
            .await
            .unwrap();
        let result = provider.next_input(&cancel).await;
        let msgs = result.expect("should resume with Some");
        assert_eq!(msgs.len(), 1);
        // The message should be a user message with "go" text.
        assert_eq!(msgs[0].role, caliban_provider::Role::User);
        let text = msgs[0].content.iter().find_map(|b| {
            if let caliban_provider::ContentBlock::Text(t) = b {
                Some(t.text.clone())
            } else {
                None
            }
        });
        assert_eq!(text.as_deref(), Some("go"));

        // Send EndInput → next_input returns None.
        tx.send(AttachInbound::EndInput).await.unwrap();
        let result2 = provider.next_input(&cancel).await;
        assert!(result2.is_none(), "EndInput should yield None");
    }

    #[tokio::test]
    async fn socket_input_provider_ends_on_closed_channel() {
        use tokio_util::sync::CancellationToken;

        let (tx, rx) = mpsc::channel::<AttachInbound>(16);
        let provider = SocketInputProvider {
            inbox: AsyncMutex::new(rx),
            status: None,
            idle_timeout: None,
            has_clients: Arc::new(AtomicUsize::new(0)),
        };
        let cancel = CancellationToken::new();

        // Drop the sender — channel closes → next_input returns None.
        drop(tx);
        let result = provider.next_input(&cancel).await;
        assert!(result.is_none(), "closed channel should yield None");
    }

    // --- StatusSink integration ---

    /// A recording sink that captures every status set during a test.
    struct RecordingSink(std::sync::Mutex<Vec<caliban_supervisor::proto::AgentStatus>>);

    #[async_trait::async_trait]
    impl StatusSink for RecordingSink {
        async fn set(&self, status: caliban_supervisor::proto::AgentStatus) {
            self.0.lock().unwrap().push(status);
        }
    }

    #[tokio::test]
    async fn socket_input_provider_reports_idle_then_running_on_resume() {
        use caliban_supervisor::proto::AgentStatus;
        use tokio_util::sync::CancellationToken;

        let sink = Arc::new(RecordingSink(std::sync::Mutex::new(Vec::new())));
        let (tx, rx) = mpsc::channel::<AttachInbound>(16);
        let provider = SocketInputProvider {
            inbox: AsyncMutex::new(rx),
            status: Some(Arc::clone(&sink) as Arc<dyn StatusSink>),
            idle_timeout: None,
            has_clients: Arc::new(AtomicUsize::new(0)),
        };
        let cancel = CancellationToken::new();

        // Pre-send a UserMessage so next_input can resolve without blocking.
        tx.send(AttachInbound::UserMessage { text: "go".into() })
            .await
            .unwrap();
        let result = provider.next_input(&cancel).await;
        assert!(result.is_some(), "should resume with Some");

        let recorded = sink.0.lock().unwrap().clone();
        assert_eq!(
            recorded,
            vec![AgentStatus::Idle, AgentStatus::Running],
            "expected [Idle, Running] on resume; got {recorded:?}"
        );
    }

    #[tokio::test]
    async fn socket_input_provider_reports_only_idle_on_end() {
        use caliban_supervisor::proto::AgentStatus;
        use tokio_util::sync::CancellationToken;

        let sink = Arc::new(RecordingSink(std::sync::Mutex::new(Vec::new())));
        let (tx, rx) = mpsc::channel::<AttachInbound>(16);
        let provider = SocketInputProvider {
            inbox: AsyncMutex::new(rx),
            status: Some(Arc::clone(&sink) as Arc<dyn StatusSink>),
            idle_timeout: None,
            has_clients: Arc::new(AtomicUsize::new(0)),
        };
        let cancel = CancellationToken::new();

        // Pre-send EndInput so next_input returns None.
        tx.send(AttachInbound::EndInput).await.unwrap();
        let result = provider.next_input(&cancel).await;
        assert!(result.is_none(), "EndInput should yield None");

        let recorded = sink.0.lock().unwrap().clone();
        assert_eq!(
            recorded,
            vec![AgentStatus::Idle],
            "expected only [Idle] on end; got {recorded:?}"
        );
    }

    // --- Idle-timeout tests (#81 ticket 5) ---

    /// With no clients attached and an 80ms timeout, `next_input` must return
    /// `None` before the 1s deadline. The inbox sender is kept alive so that a
    /// closed-channel `None` cannot race with the timeout `None`.
    #[tokio::test]
    async fn idle_timeout_ends_run_when_no_clients() {
        use tokio_util::sync::CancellationToken;

        let (tx, rx) = mpsc::channel::<AttachInbound>(16);
        let provider = SocketInputProvider {
            inbox: AsyncMutex::new(rx),
            status: None,
            idle_timeout: Some(std::time::Duration::from_millis(80)),
            has_clients: Arc::new(AtomicUsize::new(0)),
        };
        let cancel = CancellationToken::new();

        // Keep tx alive so the channel stays open — the TIMEOUT, not
        // channel-close, must be what returns None.
        let _tx_keepalive = tx;

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            provider.next_input(&cancel),
        )
        .await;

        assert!(
            result.is_ok(),
            "next_input did not resolve within 1 s — idle timeout did not fire"
        );
        assert!(
            result.unwrap().is_none(),
            "idle timeout must resolve to None"
        );
    }

    /// With one client attached (`has_clients = 1`) the idle timer must keep
    /// resetting, so `next_input` stays pending for the full 400ms window.
    /// Cancellation then drives it to `None`.
    #[tokio::test]
    async fn idle_timeout_does_not_fire_while_client_attached() {
        use tokio_util::sync::CancellationToken;

        let (tx, rx) = mpsc::channel::<AttachInbound>(16);
        let has_clients = Arc::new(AtomicUsize::new(1)); // one client present
        let provider = Arc::new(SocketInputProvider {
            inbox: AsyncMutex::new(rx),
            status: None,
            idle_timeout: Some(std::time::Duration::from_millis(80)),
            has_clients: Arc::clone(&has_clients),
        });
        let cancel = CancellationToken::new();

        // Keep tx alive so channel-close cannot race with the timeout check.
        let _tx_keepalive = tx;

        let cancel_clone = cancel.clone();
        let provider_clone = Arc::clone(&provider);
        let fut = tokio::spawn(async move { provider_clone.next_input(&cancel_clone).await });

        // Let the task run for 400ms; with a client attached the 80ms ticks
        // must keep resetting and the future must stay pending.
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        assert!(
            !fut.is_finished(),
            "next_input should still be pending while client is attached"
        );

        // Trigger cancellation and confirm it resolves to None.
        cancel.cancel();
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), fut)
            .await
            .expect("should resolve after cancel")
            .expect("task should not panic");
        assert!(result.is_none(), "cancelled next_input must return None");
    }

    #[tokio::test]
    async fn idle_timeout_resets_when_client_attaches_mid_idle() {
        use std::sync::atomic::Ordering;
        use tokio_util::sync::CancellationToken;

        let (tx, rx) = mpsc::channel::<AttachInbound>(16);
        // Start with NO client — the idle timer is armed.
        let has_clients = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(SocketInputProvider {
            inbox: AsyncMutex::new(rx),
            status: None,
            idle_timeout: Some(std::time::Duration::from_millis(100)),
            has_clients: Arc::clone(&has_clients),
        });
        let cancel = CancellationToken::new();
        let _tx_keepalive = tx;

        let cancel_clone = cancel.clone();
        let provider_clone = Arc::clone(&provider);
        let fut = tokio::spawn(async move { provider_clone.next_input(&cancel_clone).await });

        // A client attaches mid-idle, before the first 100ms tick can end the
        // run. The polling design must observe this and reset the timer.
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        has_clients.store(1, Ordering::Relaxed);

        // Well past several 100ms ticks: had the mid-idle attach NOT reset the
        // timer, the run would have ended at ~100ms. It must still be awaiting.
        tokio::time::sleep(std::time::Duration::from_millis(350)).await;
        assert!(
            !fut.is_finished(),
            "a client attaching mid-idle must reset the timeout and keep the run alive"
        );

        cancel.cancel();
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), fut)
            .await
            .expect("should resolve after cancel")
            .expect("task should not panic");
        assert!(result.is_none());
    }

    /// A custom idle timeout that is greater than the 5s poll interval and not
    /// a multiple of it (7s) must fire at ~7s, not overshoot to the next poll
    /// boundary (~10s). Uses the paused clock so the virtual elapsed time is
    /// exact and the test runs instantly. (#119)
    #[tokio::test(start_paused = true)]
    async fn idle_timeout_fires_at_deadline_not_next_poll_multiple() {
        use tokio_util::sync::CancellationToken;

        let (tx, rx) = mpsc::channel::<AttachInbound>(16);
        let provider = SocketInputProvider {
            inbox: AsyncMutex::new(rx),
            status: None,
            idle_timeout: Some(std::time::Duration::from_secs(7)),
            has_clients: Arc::new(AtomicUsize::new(0)),
        };
        let cancel = CancellationToken::new();
        // Keep tx alive so channel-close cannot race with the timeout.
        let _tx_keepalive = tx;

        let start = tokio::time::Instant::now();
        let result = provider.next_input(&cancel).await;
        let elapsed = start.elapsed();

        assert!(result.is_none(), "idle timeout must resolve to None");
        assert!(
            elapsed >= std::time::Duration::from_secs(7),
            "idle timeout fired too early at {elapsed:?}"
        );
        assert!(
            elapsed < std::time::Duration::from_millis(7500),
            "idle timeout overshot: fired at {elapsed:?}, expected ~7s not ~10s"
        );
    }

    /// When `idle_timeout` is `None`, the timer loop never auto-ends, so
    /// sending a `UserMessage` still resumes the run normally.
    #[tokio::test]
    async fn idle_timeout_none_idles_until_input() {
        use tokio_util::sync::CancellationToken;

        let (tx, rx) = mpsc::channel::<AttachInbound>(16);
        let provider = Arc::new(SocketInputProvider {
            inbox: AsyncMutex::new(rx),
            status: None,
            idle_timeout: None,
            has_clients: Arc::new(AtomicUsize::new(0)),
        });
        let cancel = CancellationToken::new();

        let cancel_clone = cancel.clone();
        let provider_clone = Arc::clone(&provider);
        let fut = tokio::spawn(async move { provider_clone.next_input(&cancel_clone).await });

        // Brief pause then send a message — the timeout must NOT have fired.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        tx.send(AttachInbound::UserMessage { text: "hi".into() })
            .await
            .unwrap();

        let result = tokio::time::timeout(std::time::Duration::from_secs(1), fut)
            .await
            .expect("should resolve")
            .expect("task should not panic");

        let msgs = result.expect("None timeout: idle_timeout=None should not auto-end");
        assert_eq!(msgs.len(), 1);
        let text = msgs[0].content.iter().find_map(|b| {
            if let caliban_provider::ContentBlock::Text(t) = b {
                Some(t.text.clone())
            } else {
                None
            }
        });
        assert_eq!(text.as_deref(), Some("hi"));
    }

    // --- parse_idle_timeout unit tests ---

    #[test]
    fn parse_idle_timeout_absent_gives_300s() {
        assert_eq!(
            parse_idle_timeout(None),
            Some(std::time::Duration::from_mins(5))
        );
    }

    #[test]
    fn parse_idle_timeout_zero_disables() {
        assert_eq!(parse_idle_timeout(Some("0")), None);
    }

    #[test]
    fn parse_idle_timeout_numeric_gives_that_duration() {
        assert_eq!(
            parse_idle_timeout(Some("120")),
            Some(std::time::Duration::from_mins(2))
        );
    }

    #[test]
    fn parse_idle_timeout_garbage_gives_300s() {
        assert_eq!(
            parse_idle_timeout(Some("notanumber")),
            Some(std::time::Duration::from_mins(5))
        );
    }

    // --- ClientCountGuard ---

    #[test]
    fn client_count_guard_increments_and_decrements() {
        let counter = Arc::new(AtomicUsize::new(0));
        assert_eq!(counter.load(Ordering::Relaxed), 0);
        {
            let _g = ClientCountGuard::new(Arc::clone(&counter));
            assert_eq!(counter.load(Ordering::Relaxed), 1);
            {
                let _g2 = ClientCountGuard::new(Arc::clone(&counter));
                assert_eq!(counter.load(Ordering::Relaxed), 2);
            }
            assert_eq!(counter.load(Ordering::Relaxed), 1);
        }
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }

    // --- Inherited hook config (#84) ---

    /// Round-trip through JSON exactly as the parent stamps and the worker
    /// reads. A `Deny "Bash"` rule in `Default` mode with audit off must
    /// survive the journey intact.
    #[test]
    fn inherited_hooks_selected_when_present() {
        use crate::hook_inherit::InheritableHookConfig;
        use caliban_agent_core::{Action, PermissionMode, Rule};

        let cfg = InheritableHookConfig {
            rules: vec![Rule {
                tool: "Bash".into(),
                action: Action::Deny,
                comment: None,
                reason: None,
                expires_at: None,
            }],
            mode: PermissionMode::Default,
            audit: false,
            runtime_rules: vec![],
        };
        let json = cfg.to_json().expect("to_json should succeed");

        // Simulate the worker path: from_json returns Some when the JSON is valid.
        let parsed =
            InheritableHookConfig::from_json(&json).expect("from_json should parse valid JSON");
        assert_eq!(parsed.rules.len(), 1);
        assert_eq!(parsed.rules[0].tool, "Bash");
        assert!(matches!(parsed.rules[0].action, Action::Deny));
        assert_eq!(parsed.mode, PermissionMode::Default);
        assert!(!parsed.audit);

        // Malformed input falls back to None (worker uses #75 default gate).
        assert!(InheritableHookConfig::from_json("not json at all").is_none());
    }

    /// Call `build_inherited_hooks` with a small non-Auto config and a real
    /// provider, asserting it builds without panicking. The chain components
    /// (`PermissionsHook`, `ModeFilter`, `wrap_with_audit`) are individually
    /// tested in caliban-agent-core; this verifies the wiring compiles and runs.
    #[test]
    fn build_inherited_hooks_default_mode_builds() {
        use crate::hook_inherit::InheritableHookConfig;
        use caliban_agent_core::{Action, PermissionMode, Rule};

        // Build a minimal provider for the factory.  We use the same path that
        // the worker uses: parse minimal args and call build_provider.
        let args = crate::args::Args::try_parse_from(["caliban", "--bare"])
            .expect("minimal args should parse");
        let pool = Arc::new(caliban_settings::ApiKeyHelperPool::from_raw(None));
        // build_provider may fail when no API key is present in the test env.
        // If so, skip rather than fail — the interesting logic is the chain
        // construction, not the provider itself.
        let Ok(provider) = crate::startup::build_provider(&args, &pool) else {
            return; // no key in CI env — skip
        };

        let cfg = InheritableHookConfig {
            rules: vec![Rule {
                tool: "Read".into(),
                action: Action::Allow,
                comment: None,
                reason: None,
                expires_at: None,
            }],
            mode: PermissionMode::Default,
            audit: false,
            runtime_rules: vec![],
        };

        // Must not panic.
        let _chain = build_inherited_hooks(cfg, &provider, "claude-3-haiku", "test-session".into());
    }

    /// Build a config where the static policy ALLOWS `Bash` but the inherited
    /// runtime rules DENY it. Because runtime rules outrank config rules, the
    /// worker's inherited hook chain must deny the call — proving the parent's
    /// live "Always deny" propagated through the spawn spec (#114). Uses a
    /// `MockProvider` so it runs without an API key (Default mode never touches
    /// the provider).
    #[tokio::test]
    async fn inherited_runtime_deny_overrides_config_allow() {
        use crate::hook_inherit::InheritableHookConfig;
        use caliban_agent_core::{
            Action, HookDecision, PermissionMode, Rule, RuntimeRule, ToolCtx,
        };
        use caliban_provider::MockProvider;

        let provider: Arc<dyn caliban_provider::Provider + Send + Sync> =
            Arc::new(MockProvider::new());
        let cfg = InheritableHookConfig {
            rules: vec![Rule {
                tool: "Bash".into(),
                action: Action::Allow,
                comment: None,
                reason: None,
                expires_at: None,
            }],
            mode: PermissionMode::Default,
            audit: false,
            runtime_rules: vec![RuntimeRule {
                pattern: "Bash".into(),
                action: Action::Deny,
            }],
        };
        let chain = build_inherited_hooks(cfg, &provider, "mock-model", "test-session".into());

        let input = serde_json::json!({"command": "rm -rf /"});
        let ctx = ToolCtx {
            session_id: "test-session",
            turn_index: 0,
            tool_use_id: "t1",
            tool_name: "Bash",
            input: &input,
            is_read_only: false,
        };
        let decision = chain.before_tool(&ctx).await.expect("hook decision");
        assert!(
            matches!(decision, HookDecision::Deny(_)),
            "inherited runtime deny must override config allow, got {decision:?}"
        );
    }

    /// Control for the test above: with NO inherited runtime rules, the same
    /// config `Allow Bash` stands. Confirms the deny in the sibling test comes
    /// from the inherited runtime rule, not the base policy.
    #[tokio::test]
    async fn without_inherited_runtime_rules_config_allow_stands() {
        use crate::hook_inherit::InheritableHookConfig;
        use caliban_agent_core::{Action, HookDecision, PermissionMode, Rule, ToolCtx};
        use caliban_provider::MockProvider;

        let provider: Arc<dyn caliban_provider::Provider + Send + Sync> =
            Arc::new(MockProvider::new());
        let cfg = InheritableHookConfig {
            rules: vec![Rule {
                tool: "Bash".into(),
                action: Action::Allow,
                comment: None,
                reason: None,
                expires_at: None,
            }],
            mode: PermissionMode::Default,
            audit: false,
            runtime_rules: vec![],
        };
        let chain = build_inherited_hooks(cfg, &provider, "mock-model", "test-session".into());

        let input = serde_json::json!({"command": "ls"});
        let ctx = ToolCtx {
            session_id: "test-session",
            turn_index: 0,
            tool_use_id: "t1",
            tool_name: "Bash",
            input: &input,
            is_read_only: false,
        };
        let decision = chain.before_tool(&ctx).await.expect("hook decision");
        assert!(
            matches!(decision, HookDecision::Allow),
            "config allow must stand without an inherited runtime rule, got {decision:?}"
        );
    }

    // --- attach over the network (#280 Task 8 acceptance test) ---

    /// The #280 acceptance criterion: a real per-agent listener bound with
    /// TCP+TLS+token, served by the worker's own `serve_attach_client`, and a
    /// client attaching *over the network* — asserting `TurnEvent` NDJSON
    /// flows outbound (rendered exactly as `agents attach` renders it) and an
    /// `AttachInbound::UserMessage` flows inbound to the worker's inbox.
    ///
    /// This exercises the actual worker path (real `Listener`/`connect` from
    /// `caliban_supervisor::transport`, the real `EventHub` +
    /// `serve_attach_client` + `read_inbound_frames`), not a stand-in server.
    /// It deliberately reads a bounded number of known NDJSON lines rather
    /// than driving `stream_attach` to EOF: `serve_attach_client` only closes
    /// the socket when its broadcast receiver reports `Closed` (all
    /// `Arc<EventHub>` clones dropped) or a write fails — neither of which a
    /// single still-attached client can trigger from the outside, mirroring
    /// how a real worker's socket is torn down by process exit, not a
    /// graceful in-band shutdown.
    #[tokio::test]
    async fn attach_over_tcp_tls_token_streams_turnevents_and_accepts_inbound() {
        use caliban_supervisor::transport::{
            BindSpec, ConnectSpec, Endpoint, Listener, connect, tls_client_from_pem,
            tls_server_from_pem,
        };
        use tokio::io::{AsyncBufReadExt as _, BufReader};

        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let cert_pem = cert.cert.pem().into_bytes();
        let key_pem = cert.key_pair.serialize_pem().into_bytes();
        let token = "worker-attach-tok".to_string();

        let tls_server = tls_server_from_pem(&cert_pem, &key_pem).unwrap();
        let bind = BindSpec {
            endpoint: Endpoint::Tcp {
                addr: "127.0.0.1:0".into(),
            },
            tls: Some(tls_server),
            token: Some(token.clone()),
        };
        let listener = Listener::bind(&bind).await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Publish exactly the two events the deliverable calls for: a text
        // delta ("hello world") and a RunEnd (renders as "done"). Order vs.
        // `listener.accept()` below doesn't matter — `EventHub` replays its
        // full history to a client that subscribes after publish (#79).
        let hub = EventHub::new();
        let delta = caliban_agent_core::TurnEvent::AssistantTextDelta {
            turn_index: 0,
            content_block_index: 0,
            text: "hello world".into(),
        };
        hub.publish(Arc::from(serde_json::to_string(&delta).unwrap().as_str()));
        let end = caliban_agent_core::TurnEvent::RunEnd {
            final_messages: vec![],
            total_usage: caliban_agent_core::Usage::default(),
            turn_count: 1,
            stopped_for: caliban_agent_core::StopCondition::EndOfTurn,
            turns_without_edit: 0,
            no_edit_nudge_emitted: false,
        };
        hub.publish(Arc::from(serde_json::to_string(&end).unwrap().as_str()));

        // Serve the one connection with the REAL worker attach path —
        // interactive mode, so inbound `AttachInbound` frames feed `inbox`.
        let has_clients = Arc::new(AtomicUsize::new(0));
        let (inbox_tx, mut inbox_rx) = mpsc::channel::<AttachInbound>(8);
        let server_hub = Arc::clone(&hub);
        let server = tokio::spawn(async move {
            let conn = listener.accept().await.unwrap();
            serve_attach_client(conn, server_hub, Some(inbox_tx), has_clients).await;
        });

        // Client attaches over TCP + TLS + token — the real network path.
        let tls_client = tls_client_from_pem(&cert_pem, "localhost").unwrap();
        let conn = connect(&ConnectSpec {
            endpoint: Endpoint::Tcp { addr },
            tls: Some(tls_client),
            token: Some(token),
        })
        .await
        .unwrap();
        let (read_half, mut write_half) = tokio::io::split(conn);
        let mut ndjson_reader = BufReader::new(read_half).lines();

        // OUTBOUND: real NDJSON `TurnEvent` lines over the wire, rendered
        // exactly as `caliban agents attach` renders them (#79).
        let delta_line = ndjson_reader
            .next_line()
            .await
            .unwrap()
            .expect("outbound line 1 (text delta)");
        let delta_event: caliban_agent_core::TurnEvent = serde_json::from_str(&delta_line).unwrap();
        let delta_rendered = crate::attach::render_event(&delta_event);
        assert!(
            delta_rendered.contains("hello world"),
            "got: {delta_rendered:?}"
        );

        let end_line = ndjson_reader
            .next_line()
            .await
            .unwrap()
            .expect("outbound line 2 (RunEnd)");
        let end_event: caliban_agent_core::TurnEvent = serde_json::from_str(&end_line).unwrap();
        let end_rendered = crate::attach::render_event(&end_event);
        assert!(end_rendered.contains("done"), "got: {end_rendered:?}");

        // INBOUND: the client writes an `AttachInbound::UserMessage` frame;
        // the worker's `read_inbound_frames` (spawned by `serve_attach_client`)
        // must forward it to the shared inbox — bidirectional attach over the
        // network (ADR 0047 / #81).
        let frame = AttachInbound::UserMessage {
            text: "ping from client".into(),
        };
        let mut buf = serde_json::to_vec(&frame).unwrap();
        buf.push(b'\n');
        write_half.write_all(&buf).await.unwrap();
        write_half.flush().await.unwrap();

        let got = tokio::time::timeout(std::time::Duration::from_secs(5), inbox_rx.recv())
            .await
            .expect("inbound frame timed out")
            .expect("inbox closed unexpectedly");
        assert_eq!(got, frame, "worker did not receive the inbound frame");

        server.abort();
    }

    /// The per-agent listener rejects a wrong bearer token exactly like the
    /// control-plane listener does (Task 7's `tcp_token_accept_and_reject` in
    /// `caliban-supervisor/src/transport.rs`) — same `Listener::accept()`
    /// code path, so the guarantee carries over unchanged to the worker's
    /// socket. Pinned here too so a worker-side regression (e.g. a future
    /// worker-specific accept wrapper that skips the token check) is caught
    /// where the worker actually uses it.
    #[tokio::test]
    async fn attach_listener_rejects_wrong_token() {
        use caliban_supervisor::transport::{BindSpec, ConnectSpec, Endpoint, Listener, connect};

        let bind = BindSpec {
            endpoint: Endpoint::Tcp {
                addr: "127.0.0.1:0".into(),
            },
            tls: None,
            token: Some("right-token".into()),
        };
        let listener = Listener::bind(&bind).await.unwrap();
        let addr = listener.local_addr().unwrap();

        let srv = tokio::spawn(async move { listener.accept().await });

        let mut bad_conn = connect(&ConnectSpec {
            endpoint: Endpoint::Tcp { addr },
            tls: None,
            token: Some("wrong-token".into()),
        })
        .await
        .unwrap();
        // Keep the connection open briefly so the server-side accept has a
        // chance to read the (bad) preamble before we drop it.
        let _ = bad_conn.write_all(b"x").await;

        let result = srv.await.unwrap();
        assert_eq!(
            result.err().map(|e| e.kind()),
            Some(std::io::ErrorKind::PermissionDenied),
            "wrong token must be rejected at accept-time"
        );
    }

    /// Regression for the fix-before-merge finding: the per-agent accept loop
    /// used to be `while let Ok(conn) = listener.accept().await { … }`, which
    /// permanently exits on the FIRST rejected connection (wrong token,
    /// failed TLS handshake, or a bare port-scan probe) — one bad dial would
    /// disable attach for the rest of the agent's life. `run_agent_accept_loop`
    /// must instead log the error and keep accepting.
    ///
    /// Drives the real accept loop end-to-end: one wrong-token connection
    /// (asserted rejected — the server closes it without ever streaming
    /// protocol data), then a good TLS+token connection on the SAME listener,
    /// asserting it still attaches and receives a published `TurnEvent`.
    #[tokio::test]
    async fn per_agent_accept_loop_survives_a_rejected_connection() {
        use caliban_supervisor::transport::{
            BindSpec, ConnectSpec, Endpoint, Listener, connect, tls_client_from_pem,
            tls_server_from_pem,
        };
        use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, BufReader};

        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let cert_pem = cert.cert.pem().into_bytes();
        let key_pem = cert.key_pair.serialize_pem().into_bytes();
        let token = "loop-survives-tok".to_string();

        let tls_server = tls_server_from_pem(&cert_pem, &key_pem).unwrap();
        let bind = BindSpec {
            endpoint: Endpoint::Tcp {
                addr: "127.0.0.1:0".into(),
            },
            tls: Some(tls_server),
            token: Some(token.clone()),
        };
        let listener = Listener::bind(&bind).await.unwrap();
        let addr = listener.local_addr().unwrap();

        let hub = EventHub::new();
        let end = caliban_agent_core::TurnEvent::RunEnd {
            final_messages: vec![],
            total_usage: caliban_agent_core::Usage::default(),
            turn_count: 1,
            stopped_for: caliban_agent_core::StopCondition::EndOfTurn,
            turns_without_edit: 0,
            no_edit_nudge_emitted: false,
        };
        hub.publish(Arc::from(serde_json::to_string(&end).unwrap().as_str()));

        let has_clients = Arc::new(AtomicUsize::new(0));
        tokio::spawn(run_agent_accept_loop(
            listener,
            Arc::clone(&hub),
            None,
            has_clients,
        ));

        // 1) A wrong-token connection: TLS handshake succeeds, but the token
        // preamble is wrong, so the server rejects it at accept-time (same
        // `PermissionDenied` path as `attach_listener_rejects_wrong_token`)
        // and closes the socket without ever reaching `serve_attach_client`.
        let bad_tls_client = tls_client_from_pem(&cert_pem, "localhost").unwrap();
        let mut bad_conn = connect(&ConnectSpec {
            endpoint: Endpoint::Tcp { addr: addr.clone() },
            tls: Some(bad_tls_client),
            token: Some("wrong-token".into()),
        })
        .await
        .unwrap();
        let mut buf = [0u8; 8];
        let read_result =
            tokio::time::timeout(std::time::Duration::from_secs(5), bad_conn.read(&mut buf))
                .await
                .expect("server should close the rejected connection promptly");
        // The server drops the TCP stream as soon as the token check fails,
        // without a graceful TLS `close_notify` — rustls surfaces that as an
        // `UnexpectedEof` error rather than a clean `Ok(0)`. Either outcome
        // means the same thing here: no protocol data was ever streamed.
        match read_result {
            Ok(n) => assert_eq!(
                n, 0,
                "a rejected connection must not stream any protocol data"
            ),
            Err(e) => assert_eq!(
                e.kind(),
                std::io::ErrorKind::UnexpectedEof,
                "expected a clean close (or EOF-without-close_notify) for a rejected connection, got {e:?}"
            ),
        }

        // 2) A good TLS+token connection on the SAME listener must still be
        // accepted and attach normally. Under the old `while let Ok` shape
        // the loop would already be dead at this point and this dial would
        // hang forever waiting for data that never comes.
        let good_tls_client = tls_client_from_pem(&cert_pem, "localhost").unwrap();
        let conn = connect(&ConnectSpec {
            endpoint: Endpoint::Tcp { addr },
            tls: Some(good_tls_client),
            token: Some(token),
        })
        .await
        .unwrap();
        let (read_half, _write_half) = tokio::io::split(conn);
        let mut ndjson_reader = BufReader::new(read_half).lines();
        let line =
            tokio::time::timeout(std::time::Duration::from_secs(5), ndjson_reader.next_line())
                .await
                .expect("accept loop must still be alive after a prior rejected connection")
                .unwrap()
                .expect("expected a replayed TurnEvent line");
        let event: caliban_agent_core::TurnEvent = serde_json::from_str(&line).unwrap();
        let rendered = crate::attach::render_event(&event);
        assert!(rendered.contains("done"), "got: {rendered:?}");
    }
}
