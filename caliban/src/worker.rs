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
use tokio::net::UnixListener;
use tokio::sync::{Mutex as AsyncMutex, broadcast, mpsc};

use crate::attach::AttachInbound;

/// Fan-out hub for a worker's `TurnEvent` NDJSON stream. Holds the full
/// history of serialized event lines plus a broadcast channel for live
/// delivery. The lock makes "append + broadcast" and "snapshot + subscribe"
/// atomic with respect to each other, so a client that attaches mid-run
/// receives every event exactly once (no gap between the historical
/// snapshot and the live tail).
struct EventHub {
    history: Mutex<Vec<Arc<str>>>,
    tx: broadcast::Sender<Arc<str>>,
}

impl EventHub {
    fn new() -> Arc<Self> {
        let (tx, _rx) = broadcast::channel(1024);
        Arc::new(Self {
            history: Mutex::new(Vec::new()),
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
        hist.push(Arc::clone(&line));
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
        (hist.clone(), rx)
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
        // Poll on a tick (<= the timeout) so a client attaching mid-idle resets
        // the accumulated idle time before it can trip the timeout.
        let tick = self
            .idle_timeout
            .map_or(std::time::Duration::from_secs(5), |d| {
                d.min(std::time::Duration::from_secs(5))
            });
        let mut idle_elapsed = std::time::Duration::ZERO;
        let out = loop {
            tokio::select! {
                () = cancel.cancelled() => break None,
                frame = rx.recv() => break match frame {
                    Some(AttachInbound::UserMessage { text }) =>
                        Some(vec![caliban_provider::Message::user_text(text)]),
                    Some(AttachInbound::EndInput) | None => None,
                },
                () = tokio::time::sleep(tick) => {
                    if self.has_clients.load(Ordering::Relaxed) > 0 {
                        idle_elapsed = std::time::Duration::ZERO; // operator present — reset
                        continue;
                    }
                    if let Some(limit) = self.idle_timeout {
                        idle_elapsed += tick;
                        if idle_elapsed >= limit {
                            tracing::info!(
                                "interactive agent idle timeout with no clients — ending"
                            );
                            break None;
                        }
                    }
                    // None: no timeout configured — keep waiting
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
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncBufReadExt as _;
    let mut lines = tokio::io::BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(frame) = serde_json::from_str::<AttachInbound>(&line)
            && inbox.send(frame).await.is_err()
        {
            break;
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

/// Entry point body. Returns the process exit code.
#[allow(clippy::too_many_lines)]
pub(crate) async fn run(manifest: &Path, socket: &Path, control_socket: Option<&Path>) -> i32 {
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

    // --- Bind the per-agent Unix socket.
    // The socket file's existence signals to `caliband` and `caliban agents
    // attach` that this worker is alive.
    if let Some(parent) = socket.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let _ = tokio::fs::remove_file(socket).await;
    let listener = match UnixListener::bind(socket) {
        Ok(l) => l,
        Err(e) => {
            eprintln!(
                "[caliban __agent-worker] bind {} failed: {e}",
                socket.display()
            );
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
        // Build a status sink if a control socket was provided.
        let status_sink: Option<Arc<dyn StatusSink>> =
            control_socket.map(|ctl| -> Arc<dyn StatusSink> {
                Arc::new(ControlSocketStatus {
                    client: caliban_supervisor::SupervisorClient::new(ctl),
                    id: record.id.clone(),
                })
            });
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
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let conn_hub = Arc::clone(&accept_hub);
            let conn_inbox = accept_inbox_tx.clone();
            let conn_clients = Arc::clone(&accept_has_clients);
            tokio::spawn(serve_attach_client(
                stream,
                conn_hub,
                conn_inbox,
                conn_clients,
            ));
        }
    });
    // Keep the sender alive so the channel doesn't close between connections.
    let _ = &inbox_keepalive;

    // --- Build the agent. ---
    let _ = tokio::fs::create_dir_all(&record.session_dir).await;
    let ndjson_path = record.session_dir.join("stdout.ndjson");

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
    let inner: Arc<dyn caliban_agent_core::Hooks> =
        Arc::new(PermissionsHook::new(cfg.rules, ask, Arc::new(NoopHooks)));

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
    stream: tokio::net::UnixStream,
    hub: Arc<EventHub>,
    inbox: Option<mpsc::Sender<AttachInbound>>,
    clients: Arc<AtomicUsize>,
) {
    // Hold the guard for the entire lifetime of this connection — even early
    // returns (e.g. write errors in history replay) are covered by Drop.
    let _client_guard = ClientCountGuard::new(clients);

    let (read_half, mut write_half) = stream.into_split();

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
async fn write_line(
    stream: &mut tokio::net::unix::OwnedWriteHalf,
    line: &str,
) -> std::io::Result<()> {
    stream.write_all(line.as_bytes()).await?;
    stream.write_all(b"\n").await
}

#[cfg(test)]
mod tests {
    use super::*;
    use caliban_agent_core::{Action, default_rules};

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
            socket_path: dir.path().join("w1.sock"),
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
        };

        // Must not panic.
        let _chain = build_inherited_hooks(cfg, &provider, "claude-3-haiku", "test-session".into());
    }
}
