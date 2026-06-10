//! The `caliban __agent-worker` entry point (ADR 0037, #71).
//!
//! Launched by the `caliband` supervisor as a child process. Reads the
//! agent's manifest, binds the per-agent socket, runs the agent loop to
//! completion, and exits with a code the supervisor maps to a terminal
//! status (0 = Done, non-zero = Failed).

use std::path::Path;
use std::sync::Arc;

use caliban_supervisor::proto::AgentRecord;
use clap::Parser as _;
use futures::StreamExt as _;
use tokio::io::AsyncWriteExt as _;
use tokio::net::UnixListener;

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

    // --- Bind the per-agent Unix socket (Plan A: accept-and-close drain).
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
    // Keep the socket live for the worker's lifetime. Plan B replaces this
    // accept-and-close drain with the real attach protocol.
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let _ = stream.shutdown().await;
        }
    });

    // --- Build the agent. ---
    let _ = tokio::fs::create_dir_all(&record.session_dir).await;
    let ndjson_path = record.session_dir.join("stdout.ndjson");

    // Construct minimal Args with the spec's model override.
    // Args does not impl Default, so we parse a minimal invocation.
    //
    // Plan A posture (see #71): the worker runs with `--no-permissions`
    // (no permission gate) and the default tool set (Bash/Write/Edit/Web),
    // and currently honors only `spec.model` + `spec.initial_prompt`. The
    // remaining SpawnSpec fields — `tool_allowlist`, `inherit_hooks`,
    // `isolation_worktree`, `frontmatter_path` — are intentionally NOT yet
    // applied. Plan B wires those, most importantly `tool_allowlist` + a
    // permission policy, so background sub-agents stop running unguarded.
    let mut args =
        match crate::args::Args::try_parse_from(["caliban", "--no-permissions", "--bare"]) {
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

    let agent = Arc::new(
        caliban_agent_core::Agent::builder()
            .provider(provider)
            .tools(registry)
            .config(caliban_agent_core::AgentConfig {
                model: model.clone(),
                ..caliban_agent_core::AgentConfig::default()
            })
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
                if let Ok(mut line) = serde_json::to_vec(&ev) {
                    line.push(b'\n');
                    let _ = ndjson.write_all(&line).await;
                }
            }
            Err(e) => {
                eprintln!("[caliban __agent-worker] stream error: {e}");
                return 1;
            }
        }
    }
    let _ = ndjson.flush().await;

    crate::startup::stop_condition_exit_code(&stop)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
