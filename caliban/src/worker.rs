//! The `caliban __agent-worker` entry point (ADR 0037, #71).
//!
//! Launched by the `caliband` supervisor as a child process. Reads the
//! agent's manifest, binds the per-agent socket, runs the agent loop to
//! completion, and exits with a code the supervisor maps to a terminal
//! status (0 = Done, non-zero = Failed).

use std::path::Path;
use std::sync::{Arc, Mutex};

use caliban_supervisor::proto::AgentRecord;
use clap::Parser as _;
use futures::StreamExt as _;
use tokio::io::AsyncWriteExt as _;
use tokio::net::UnixListener;
use tokio::sync::broadcast;

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
        let mut hist = self.history.lock().expect("event hub lock");
        hist.push(Arc::clone(&line));
        // Err just means there are no live subscribers right now — fine.
        let _ = self.tx.send(line);
    }

    /// Atomically snapshot the history and subscribe for live events. The
    /// returned receiver yields only events published AFTER this call;
    /// combined with the snapshot, the caller sees each event exactly once.
    fn subscribe(&self) -> (Vec<Arc<str>>, broadcast::Receiver<Arc<str>>) {
        let hist = self.history.lock().expect("event hub lock");
        let rx = self.tx.subscribe();
        (hist.clone(), rx)
    }
}

/// Load the `AgentRecord` the supervisor wrote for this worker.
pub(crate) fn load_record(manifest: &Path) -> std::io::Result<AgentRecord> {
    let body = std::fs::read(manifest)?;
    serde_json::from_slice(&body).map_err(std::io::Error::other)
}

/// Entry point body. Returns the process exit code.
#[allow(clippy::too_many_lines)]
pub(crate) async fn run(manifest: &Path, socket: &Path) -> i32 {
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
    // Accept loop: replay history then forward live events to each client.
    let accept_hub = Arc::clone(&hub);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let conn_hub = Arc::clone(&accept_hub);
            tokio::spawn(serve_attach_client(stream, conn_hub));
        }
    });

    // --- Build the agent. ---
    let _ = tokio::fs::create_dir_all(&record.session_dir).await;
    let ndjson_path = record.session_dir.join("stdout.ndjson");

    // Construct minimal Args with the spec's model override.
    // Args does not impl Default, so we parse a minimal invocation.
    //
    // Permission posture (#75): the worker runs with `--bare` (no TUI/MCP)
    // and a real permission gate. `spec.tool_allowlist` filters the registry
    // AND seeds Allow rules that precede the `default_rules()` tail, so only
    // the tools the spawner explicitly granted are accessible; anything else
    // falls through to the defaults (read-only Allow; Bash/Write/Edit Ask →
    // denied non-interactively because `auto_allow: false`).
    //
    // Full `inherit_hooks` (parent-config inheritance) is deferred to #84 and
    // requires SpawnSpec proto changes; regardless of `spec.inherit_hooks`, the
    // worker uses this binary-default gate for now.
    let mut args = match crate::args::Args::try_parse_from(["caliban", "--bare"]) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("[caliban __agent-worker] args construction failed: {e}");
            return 70;
        }
    };
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

    let rules = build_worker_rules(record.spec.tool_allowlist.as_deref());
    let ask: Arc<dyn caliban_agent_core::AskHandler> =
        Arc::new(caliban_agent_core::NonInteractiveAskHandler { auto_allow: false });
    let permissions: Arc<dyn caliban_agent_core::Hooks + Send + Sync> =
        Arc::new(caliban_agent_core::PermissionsHook::new(
            rules,
            ask,
            Arc::new(caliban_agent_core::NoopHooks),
        ));

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
    let mut stream = agent.stream_until_done(messages, cancel);

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
async fn serve_attach_client(mut stream: tokio::net::UnixStream, hub: Arc<EventHub>) {
    let (history, mut rx) = hub.subscribe();
    for line in &history {
        if write_line(&mut stream, line).await.is_err() {
            return;
        }
    }
    loop {
        match rx.recv().await {
            Ok(line) => {
                if write_line(&mut stream, &line).await.is_err() {
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
    let _ = stream.shutdown().await;
}

/// Write one NDJSON line (event + trailing newline) to the client.
async fn write_line(stream: &mut tokio::net::UnixStream, line: &str) -> std::io::Result<()> {
    stream.write_all(line.as_bytes()).await?;
    stream.write_all(b"\n").await
}

#[cfg(test)]
mod tests {
    use super::*;
    use caliban_agent_core::{Action, default_rules};

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
                tool_allowlist: None,
                isolation_worktree: false,
                inherit_hooks: true,
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
}
