//! `caliban agents …` / `caliban daemon …` subcommand handling.
//!
//! Each handler returns a process exit code. They auto-spawn the
//! `caliband` daemon binary on first use and talk to it over the
//! per-repo Unix domain socket.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use caliban_supervisor::proto::{AgentRecord, AgentStatus, SpawnSpec};
use caliban_supervisor::{ClientError, Endpoint, SupervisorClient, repo_socket_path};

/// Discover the repo root containing `start_dir`. Walks up looking for
/// `.git/`. Falls back to `start_dir` itself if none is found (the
/// supervisor doesn't *require* a real repo, but logs cleanly).
pub(crate) fn discover_repo_root(start_dir: &Path) -> PathBuf {
    let mut cur: PathBuf = start_dir.to_path_buf();
    loop {
        if cur.join(".git").exists() {
            return cur;
        }
        match cur.parent() {
            Some(p) => cur = p.to_path_buf(),
            None => return start_dir.to_path_buf(),
        }
    }
}

/// Spawn the `caliband` binary as a child process so it can take over
/// the socket. We use `caliband` next to the caliban binary (i.e. same
/// `cargo target` dir). Best-effort: returns Ok even if we couldn't
/// spawn — the next request attempt will surface a clean "not running"
/// error.
fn try_spawn_daemon(repo_root: &Path, socket_path: &Path) -> Result<()> {
    let exe = std::env::current_exe().context("current_exe")?;
    let mut daemon_exe = exe.clone();
    daemon_exe.set_file_name("caliband");
    if !daemon_exe.exists() {
        // Fall back to PATH lookup.
        daemon_exe = PathBuf::from("caliband");
    }
    let mut cmd = std::process::Command::new(&daemon_exe);
    cmd.arg("--repo-root")
        .arg(repo_root)
        .arg("--socket-path")
        .arg(socket_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Best-effort: spawn and immediately detach.
    let _ = cmd.spawn();
    Ok(())
}

/// Public re-export of [`ensure_daemon`] for the `AgentTool` background
/// spawner wired in `main`.
pub(crate) async fn ensure_daemon_for_repo(repo_root: &Path) -> Result<SupervisorClient> {
    ensure_daemon(repo_root).await
}

/// Ensure a daemon is running for the given repo. Polls the socket for
/// existence up to 2s; returns a connected client.
async fn ensure_daemon(repo_root: &Path) -> Result<SupervisorClient> {
    let socket_path = repo_socket_path(repo_root);
    if !socket_path.exists() {
        try_spawn_daemon(repo_root, &socket_path)?;
        for _ in 0..200 {
            if socket_path.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    Ok(SupervisorClient::new(socket_path))
}

fn fmt_status(s: AgentStatus) -> &'static str {
    match s {
        AgentStatus::Spawning => "spawning",
        AgentStatus::Running => "running",
        AgentStatus::Idle => "idle",
        AgentStatus::Killed => "killed",
        AgentStatus::Done => "done",
        AgentStatus::Failed => "failed",
        AgentStatus::Crashed => "crashed",
    }
}

fn print_list(agents: &[AgentRecord]) {
    if agents.is_empty() {
        println!("no background agents registered");
        return;
    }
    println!(
        "{:<14}  {:<24}  {:<10}  {:<25}  SOCKET",
        "ID", "NAME", "STATUS", "STARTED"
    );
    for a in agents {
        println!(
            "{:<14}  {:<24}  {:<10}  {:<25}  {}",
            a.id,
            truncate(&a.name, 24),
            fmt_status(a.status),
            a.started_at,
            fmt_endpoint(&a.endpoint)
        );
    }
}

/// Render an endpoint for human-readable CLI output (Unix path or `host:port`).
fn fmt_endpoint(e: &Endpoint) -> String {
    match e {
        Endpoint::Unix { path } => path.display().to_string(),
        Endpoint::Tcp { addr } => addr.clone(),
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn map_client_error(e: ClientError) -> i32 {
    match e {
        ClientError::NotRunning(path) => {
            eprintln!(
                "caliban: daemon not running at {} (try again; the binary auto-spawns it)",
                path.display()
            );
            74 // EX_IOERR-ish
        }
        other => {
            eprintln!("caliban: {other}");
            1
        }
    }
}

/// Handle `caliban agents list` / `agents <verb>`.
#[allow(clippy::too_many_lines)]
pub(crate) async fn run_agents(cmd: &crate::AgentsCommand, repo_root: &Path) -> i32 {
    let client = match ensure_daemon(repo_root).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("caliban: could not start daemon: {e}");
            return 1;
        }
    };
    match cmd {
        crate::AgentsCommand::List => match client.list().await {
            Ok(agents) => {
                print_list(&agents);
                0
            }
            Err(e) => map_client_error(e),
        },
        crate::AgentsCommand::Attach { id } => match client.attach(id.clone()).await {
            Ok(endpoint) => {
                if let Endpoint::Unix { path } = &endpoint {
                    run_attach(path, id).await
                } else {
                    // Unreachable in Unix-only mode (this task never produces
                    // a TCP endpoint); Task 6 generalizes `run_attach` to
                    // dial any `Endpoint`.
                    eprintln!("caliban: attach to a non-Unix endpoint is not yet supported");
                    1
                }
            }
            Err(e) => map_client_error(e),
        },
        crate::AgentsCommand::Logs { id } => {
            // Logs live at <agent-store>/<id>/stdout.ndjson — the transcript
            // the worker actually writes. The supervisor's `Attach` reply gives
            // us the per-agent socket, but for now we read the persisted
            // transcript from the registry's `session_dir`.
            match client.list().await {
                Ok(agents) => {
                    if let Some(rec) = agents.into_iter().find(|a| a.id == *id) {
                        let log_path = agent_log_path(&rec.session_dir);
                        match std::fs::read_to_string(&log_path) {
                            Ok(body) => {
                                println!("{body}");
                                0
                            }
                            Err(e) => {
                                eprintln!("caliban: no log at {}: {e}", log_path.display());
                                1
                            }
                        }
                    } else {
                        // Same spelling the supervisor's NotFound thiserror
                        // emits, so the user sees one canonical phrase.
                        eprintln!("caliban: daemon: agent not found: {id}");
                        1
                    }
                }
                Err(e) => map_client_error(e),
            }
        }
        crate::AgentsCommand::Kill { id } => match client.kill(id.clone()).await {
            Ok(()) => {
                println!("killed {id}");
                0
            }
            Err(e) => map_client_error(e),
        },
        crate::AgentsCommand::Respawn { id } => match client.respawn(id.clone()).await {
            Ok(new_id) => {
                println!("respawned {id} -> {new_id}");
                0
            }
            Err(e) => map_client_error(e),
        },
        crate::AgentsCommand::Rm { id, force } => match client.rm(id.clone(), *force).await {
            Ok(()) => {
                println!("removed {id}");
                0
            }
            Err(e) => map_client_error(e),
        },
        crate::AgentsCommand::Spawn {
            prompt,
            label,
            interactive,
            provider,
        } => {
            let spec = SpawnSpec {
                label: label.clone(),
                frontmatter_path: None,
                initial_prompt: prompt.clone(),
                model: None,
                provider: (*provider).map(|p| crate::provider_name(p).to_string()),
                tool_allowlist: None,
                isolation_worktree: false,
                inherit_hooks: true,
                interactive: *interactive,
                inherited_hooks_config: None,
            };
            match client.spawn(spec).await {
                Ok((id, endpoint)) => {
                    println!("spawned {id} (socket: {})", fmt_endpoint(&endpoint));
                    0
                }
                Err(e) => map_client_error(e),
            }
        }
    }
}

/// Handle `caliban daemon <verb>`.
///
/// Neither `status` nor `stop` auto-spawn the daemon — querying state
/// or asking it to shut down shouldn't side-effect-start a fresh one.
/// When the socket is absent we report "not running" and exit cleanly
/// (status: 0 with a "not running" line, stop: 0 with a "no daemon"
/// line — both are valid steady states).
pub(crate) async fn run_daemon(cmd: &crate::DaemonCommand, repo_root: &Path) -> i32 {
    let socket_path = caliban_supervisor::repo_socket_path(repo_root);
    if !socket_path.exists() {
        match cmd {
            crate::DaemonCommand::Status => {
                println!("daemon not running (socket={})", socket_path.display());
            }
            crate::DaemonCommand::Stop => {
                println!("no daemon to stop (socket={})", socket_path.display());
            }
        }
        return 0;
    }
    let client = caliban_supervisor::SupervisorClient::new(socket_path);
    match cmd {
        crate::DaemonCommand::Status => match client.status().await {
            Ok(s) => {
                println!(
                    "pid={}  agents={}  uptime_secs={}  socket={}",
                    s.pid,
                    s.agents,
                    s.uptime_secs,
                    fmt_endpoint(&s.endpoint),
                );
                0
            }
            Err(e) => map_client_error(e),
        },
        crate::DaemonCommand::Stop => match client.shutdown().await {
            Ok(()) => {
                println!("daemon shutdown requested");
                0
            }
            Err(e) => map_client_error(e),
        },
    }
}

/// Connect to a worker's per-agent socket and stream its transcript to
/// stdout until the agent finishes (EOF) or the user detaches with Ctrl+C.
/// Also pumps operator stdin as `AttachInbound` NDJSON frames to the write
/// half of the socket (bidirectional attach for interactive agents, ADR 0047 / #81).
async fn run_attach(socket_path: &Path, id: &str) -> i32 {
    use tokio::net::UnixStream;
    let stream = match UnixStream::connect(socket_path).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "caliban: cannot attach to {id} at {} ({e}); the agent may have finished \u{2014} try `caliban logs {id}`",
                socket_path.display()
            );
            return 74;
        }
    };
    eprintln!(
        "caliban: attached to {id} (type to send \u{00b7} Ctrl+D end-of-input \u{00b7} Ctrl+C detach)"
    );
    let (read_half, write_half) = stream.into_split();

    // Pump operator stdin → inbound frames on a background task. Harmless
    // for non-interactive agents (the worker drops its read half; our
    // writes simply stop on error). Aborted when we detach / the agent ends.
    let send = tokio::spawn(crate::attach::stdin_to_frames(
        tokio::io::stdin(),
        write_half,
    ));

    let mut out = std::io::stdout();
    let code = tokio::select! {
        r = crate::attach::stream_attach(read_half, &mut out) => match r {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("caliban: attach stream error: {e}");
                1
            }
        },
        _ = tokio::signal::ctrl_c() => {
            eprintln!("\ncaliban: detached from {id}");
            0
        }
    };
    send.abort();
    code
}

/// Handle the top-level `--bg "<task>"` shortcut.
pub(crate) async fn run_bg(task: &str, repo_root: &Path) -> i32 {
    let client = match ensure_daemon(repo_root).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("caliban: could not start daemon: {e}");
            return 1;
        }
    };
    let spec = SpawnSpec {
        label: None,
        frontmatter_path: None,
        initial_prompt: task.to_string(),
        model: None,
        // `run_bg` is the bare `--bg` shortcut with no ambient parent
        // session/Args to inherit from, so it uses caliban's default
        // provider (anthropic). Provider selection for the fleet flows
        // through `agents spawn --provider` / the parent `bg_spawner` (#93).
        provider: None,
        tool_allowlist: None,
        isolation_worktree: false,
        inherit_hooks: true,
        interactive: false,
        inherited_hooks_config: None,
    };
    match client.spawn(spec).await {
        Ok((id, endpoint)) => {
            println!("backgrounded as {id} (socket: {})", fmt_endpoint(&endpoint));
            0
        }
        Err(e) => map_client_error(e),
    }
}

/// Path to the transcript that `agents logs` prints for an agent, given its
/// session dir. This MUST match the file the worker actually writes
/// (`worker.rs`), so both reference [`caliban_supervisor::store::TRANSCRIPT_FILE`].
fn agent_log_path(session_dir: &Path) -> PathBuf {
    session_dir.join(caliban_supervisor::store::TRANSCRIPT_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_log_path_points_at_worker_transcript() {
        // Regression #143: `agents logs` read `session.json`, which the
        // background worker never writes — it writes `stdout.ndjson`. The two
        // must name the same file.
        let dir = Path::new("/var/agents/abc123");
        assert_eq!(
            agent_log_path(dir),
            dir.join(caliban_supervisor::store::TRANSCRIPT_FILE),
            "agents logs must read the worker's transcript file"
        );
        assert_eq!(
            agent_log_path(dir).file_name().and_then(|f| f.to_str()),
            Some("stdout.ndjson"),
        );
    }

    #[test]
    fn discover_repo_root_walks_up() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b").join("c");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        assert_eq!(discover_repo_root(&nested), dir.path());
    }

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate("abc", 10), "abc");
    }

    #[test]
    fn truncate_long_string_ellipsized() {
        let out = truncate("abcdefghijklmnop", 5);
        assert_eq!(out, "abcd…");
    }
}
