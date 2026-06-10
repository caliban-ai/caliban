//! End-to-end IPC tests for the supervisor daemon (in-process; the
//! supervisor runs on a tokio task while the client connects over the
//! same Unix socket).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use caliban_supervisor::proto::{AgentRecord, AgentStatus, SpawnSpec};
use caliban_supervisor::store::AgentStore;
use caliban_supervisor::{Supervisor, SupervisorClient, WorkerHandle, WorkerLauncher};

/// Fake launcher: runs `/bin/sh -c <script>`; exports the per-agent
/// socket path as $SOCK so the script can create it.
struct ShLauncher {
    script: String,
}

impl WorkerLauncher for ShLauncher {
    fn launch(&self, record: &AgentRecord) -> std::io::Result<WorkerHandle> {
        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.arg("-c")
            .arg(&self.script)
            .env("SOCK", &record.socket_path);
        let child = cmd.spawn()?;
        let pid = child.id().expect("sh pid");
        Ok(WorkerHandle { pid, child })
    }
}

fn spec() -> SpawnSpec {
    SpawnSpec {
        label: Some("test".into()),
        frontmatter_path: None,
        initial_prompt: "hi".into(),
        model: None,
        tool_allowlist: None,
        isolation_worktree: false,
        inherit_hooks: true,
    }
}

/// Start a supervisor with a caller-supplied launcher and return the
/// temp dir (keep alive), supervisor handle, serve join-handle, and client.
async fn boot_with(
    launcher: Arc<dyn WorkerLauncher>,
) -> (
    tempfile::TempDir,
    Arc<Supervisor>,
    tokio::task::JoinHandle<std::io::Result<()>>,
    SupervisorClient,
) {
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("caliband.sock");
    let agent_dir = dir.path().join("agents-rt");
    let store = AgentStore::new(dir.path().join("data"));
    let supervisor = Arc::new(Supervisor::with_launcher(
        socket_path.clone(),
        store,
        agent_dir,
        launcher,
    ));
    let server = Arc::clone(&supervisor);
    let handle = tokio::spawn(async move { Arc::clone(&server).serve().await });
    // Poll for socket existence (the supervisor binds in `serve`).
    for _ in 0..200 {
        if socket_path.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(socket_path.exists(), "supervisor should have bound socket");
    let client = SupervisorClient::new(socket_path);
    (dir, supervisor, handle, client)
}

/// Convenience wrapper: start with a quick-exit fake launcher.
/// Tests that do not spawn (or don't care about worker lifecycle) use this.
async fn boot() -> (
    tempfile::TempDir,
    Arc<Supervisor>,
    tokio::task::JoinHandle<std::io::Result<()>>,
    SupervisorClient,
) {
    let launcher = Arc::new(ShLauncher {
        script: "touch \"$SOCK\"; exit 0".into(),
    });
    boot_with(launcher).await
}

#[tokio::test]
async fn list_empty_returns_no_agents() {
    let (_d, sup, _h, client) = boot().await;
    let agents = client.list().await.unwrap();
    assert!(agents.is_empty());
    sup.cancel_token().cancel();
}

#[tokio::test]
async fn status_returns_pid_and_zero_agents() {
    let (_d, sup, _h, client) = boot().await;
    let s = client.status().await.unwrap();
    assert_eq!(s.pid, std::process::id());
    assert_eq!(s.agents, 0);
    sup.cancel_token().cancel();
}

#[tokio::test]
async fn spawn_registers_and_returns_socket() {
    // Use a long-running fake so the worker is still alive when we poll,
    // giving us a deterministic Running status to assert.
    let launcher = Arc::new(ShLauncher {
        script: "touch \"$SOCK\"; sleep 5".into(),
    });
    let (_d, sup, _h, client) = boot_with(launcher).await;
    let (id, sock) = client.spawn(spec()).await.unwrap();
    assert!(!id.is_empty());
    assert!(
        sock.to_string_lossy().ends_with("-agent.sock"),
        "got {sock:?}"
    );
    let list = client.list().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, id);
    // Poll until the agent reaches Running (the fake worker touches $SOCK
    // then sleeps, so it will be alive for the duration of this poll).
    let mut reached_running = false;
    for _ in 0..200 {
        let agents = client.list().await.unwrap();
        if agents
            .iter()
            .any(|a| a.id == id && a.status == AgentStatus::Running)
        {
            reached_running = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(reached_running, "agent never reached Running status");
    sup.cancel_token().cancel();
}

#[tokio::test]
async fn kill_marks_agent_killed() {
    // Worker must be alive when we issue kill, so use a long-running fake.
    let launcher = Arc::new(ShLauncher {
        script: "touch \"$SOCK\"; sleep 5".into(),
    });
    let (_d, sup, _h, client) = boot_with(launcher).await;
    let (id, _) = client.spawn(spec()).await.unwrap();
    // Poll until Running before killing, so kill acts on a live worker.
    for _ in 0..200 {
        let agents = client.list().await.unwrap();
        if agents
            .iter()
            .any(|a| a.id == id && a.status == AgentStatus::Running)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    client.kill(&id).await.unwrap();
    let list = client.list().await.unwrap();
    assert_eq!(list[0].status, AgentStatus::Killed);
    sup.cancel_token().cancel();
}

#[tokio::test]
async fn respawn_drops_old_and_creates_new_id() {
    // Quick-exit fake is fine; respawn only needs the agent to exist.
    let launcher = Arc::new(ShLauncher {
        script: "touch \"$SOCK\"; exit 0".into(),
    });
    let (_d, sup, _h, client) = boot_with(launcher).await;
    let (old_id, _) = client.spawn(spec()).await.unwrap();
    let new_id = client.respawn(&old_id).await.unwrap();
    assert_ne!(old_id, new_id);
    let list = client.list().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, new_id);
    sup.cancel_token().cancel();
}

#[tokio::test]
async fn respawn_launches_a_fresh_worker() {
    // Quick-exit worker → Done. Respawn must produce a NEW id whose
    // worker also runs to Done.
    let launcher = Arc::new(ShLauncher {
        script: "touch \"$SOCK\"; exit 0".into(),
    });
    let (_d, sup, _h, client) = boot_with(launcher).await;
    let (id, _) = client.spawn(spec()).await.unwrap();
    let new_id = client.respawn(&id).await.unwrap();
    assert_ne!(new_id, id, "respawn must assign a new id");
    // New agent reaches Done.
    let mut done = false;
    for _ in 0..200 {
        let agents = client.list().await.unwrap();
        if agents
            .iter()
            .any(|a| a.id == new_id && a.status == AgentStatus::Done)
        {
            done = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(done, "respawned agent never reached Done");
    // Old id is gone from the registry.
    let agents = client.list().await.unwrap();
    assert!(
        !agents.iter().any(|a| a.id == id),
        "old id should be removed after respawn"
    );
    sup.cancel_token().cancel();
}

#[tokio::test]
async fn rm_requires_stopped_state_without_force() {
    // Use a long-running fake launcher so the agent stays Running
    // (not Failed) by the time we issue rm.
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("caliband.sock");
    let agent_dir = dir.path().join("agents-rt");
    let store = AgentStore::new(dir.path().join("data"));
    let launcher = Arc::new(ShLauncher {
        script: "sleep 30".into(),
    });
    let supervisor = Arc::new(Supervisor::with_launcher(
        socket_path.clone(),
        store,
        agent_dir,
        launcher,
    ));
    let server = Arc::clone(&supervisor);
    let _handle = tokio::spawn(async move { Arc::clone(&server).serve().await });
    for _ in 0..200 {
        if socket_path.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let client = SupervisorClient::new(socket_path);
    let (id, _) = client.spawn(spec()).await.unwrap();
    let err = client.rm(&id, false).await.unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("invalid state"), "got {msg}");
    supervisor.cancel_token().cancel();
}

#[tokio::test]
async fn rm_with_force_succeeds() {
    // Quick-exit fake: agent will be Done or at least tracked, force-rm works regardless.
    let launcher = Arc::new(ShLauncher {
        script: "touch \"$SOCK\"; exit 0".into(),
    });
    let (_d, sup, _h, client) = boot_with(launcher).await;
    let (id, _) = client.spawn(spec()).await.unwrap();
    client.rm(&id, true).await.unwrap();
    assert!(client.list().await.unwrap().is_empty());
    sup.cancel_token().cancel();
}

#[tokio::test]
async fn rm_after_kill_succeeds_without_force() {
    // Worker must be alive when killed; use a long-running fake.
    let launcher = Arc::new(ShLauncher {
        script: "touch \"$SOCK\"; sleep 5".into(),
    });
    let (_d, sup, _h, client) = boot_with(launcher).await;
    let (id, _) = client.spawn(spec()).await.unwrap();
    // Wait for Running before issuing kill.
    for _ in 0..200 {
        let agents = client.list().await.unwrap();
        if agents
            .iter()
            .any(|a| a.id == id && a.status == AgentStatus::Running)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    client.kill(&id).await.unwrap();
    client.rm(&id, false).await.unwrap();
    sup.cancel_token().cancel();
}

#[tokio::test]
async fn rm_unknown_agent_errors() {
    let (_d, sup, _h, client) = boot().await;
    let err = client.rm("nope", true).await.unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("not found"), "got {msg}");
    sup.cancel_token().cancel();
}

#[tokio::test]
async fn shutdown_drops_socket() {
    let (_d, sup, h, client) = boot().await;
    let sock = sup.socket_path().to_path_buf();
    client.shutdown().await.unwrap();
    // Server should exit cleanly within a generous timeout.
    let _ = tokio::time::timeout(Duration::from_secs(2), h)
        .await
        .unwrap();
    assert!(
        !sock.exists(),
        "shutdown should unlink the bind socket: {sock:?}"
    );
}

#[tokio::test]
async fn spawn_launches_worker_and_reaches_done() {
    let tmp = tempfile::tempdir().unwrap();
    let ctl = tmp.path().join("ctl.sock");
    let agents_dir = tmp.path().join("agents");
    let store = AgentStore::new(tmp.path().join("store"));
    let launcher = Arc::new(ShLauncher {
        script: "touch \"$SOCK\"; exit 0".into(),
    });
    let sup = Arc::new(Supervisor::with_launcher(
        ctl.clone(),
        store,
        agents_dir,
        launcher,
    ));
    let cancel = sup.cancel_token();
    let serve = tokio::spawn(Arc::clone(&sup).serve());
    for _ in 0..200 {
        if ctl.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let client = SupervisorClient::new(ctl.clone());
    let spec = SpawnSpec {
        label: Some("w".into()),
        frontmatter_path: None,
        initial_prompt: "hi".into(),
        model: None,
        tool_allowlist: None,
        isolation_worktree: false,
        inherit_hooks: true,
    };
    let (id, socket_path) = client.spawn(spec).await.unwrap();
    let mut socket_created = false;
    for _ in 0..200 {
        if socket_path.exists() {
            socket_created = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(socket_created, "worker never created the per-agent socket");
    let mut reached_done = false;
    for _ in 0..200 {
        let agents = client.list().await.unwrap();
        if agents
            .iter()
            .any(|a| a.id == id && a.status == AgentStatus::Done)
        {
            reached_done = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(reached_done, "agent never reached Done");
    cancel.cancel();
    let _ = serve.await;
}

#[tokio::test]
#[cfg(unix)]
async fn kill_signals_the_worker_child() {
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    // The script records sh's own PID to ${SOCK}.pid, then uses `exec` to
    // replace sh with `sleep 30`.  After exec the OS pid is unchanged but
    // now belongs to the `sleep` process, so the pid file records the exact
    // pid the supervisor will SIGTERM.  Touching $SOCK signals Running.
    let launcher = Arc::new(ShLauncher {
        script: r#"echo $$ > "${SOCK}.pid"; touch "$SOCK"; exec sleep 30"#.into(),
    });
    let (_d, sup, _h, client) = boot_with(launcher).await;
    let (id, sock) = client.spawn(spec()).await.unwrap();

    // Poll until the agent reaches Running.
    let mut running = false;
    for _ in 0..200 {
        let agents = client.list().await.unwrap();
        if agents
            .iter()
            .any(|a| a.id == id && a.status == AgentStatus::Running)
        {
            running = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(running, "worker never reached Running");

    // Read the worker's pid from the file the script wrote.  The file is
    // written by the shell process before it exec's into sleep, so we
    // must poll: the Running status is set the moment cmd.spawn() returns
    // (before sh has executed any instructions).
    let pid_file = {
        let mut p = sock.clone().into_os_string();
        p.push(".pid");
        std::path::PathBuf::from(p)
    };
    let mut pid_str_opt: Option<String> = None;
    for _ in 0..200 {
        if let Ok(s) = std::fs::read_to_string(&pid_file)
            && !s.trim().is_empty()
        {
            pid_str_opt = Some(s);
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let pid_str = pid_str_opt.unwrap_or_else(|| panic!("pid file never appeared at {pid_file:?}"));
    #[allow(clippy::cast_possible_wrap)] // pids fit in i32 on all supported unix platforms
    let pid: i32 = pid_str
        .trim()
        .parse()
        .unwrap_or_else(|e| panic!("pid file did not contain an integer: {e}"));

    // Confirm the process is alive before we kill it.
    assert!(
        kill(Pid::from_raw(pid), None).is_ok(),
        "worker process {pid} should be alive before kill"
    );

    client.kill(&id).await.unwrap();

    // Assert the registry status is Killed.
    let agents = client.list().await.unwrap();
    let a = agents.iter().find(|a| a.id == id).unwrap();
    assert_eq!(a.status, AgentStatus::Killed);

    // Assert the OS process is actually gone (signal 0 = existence probe;
    // Err(ESRCH) means the process no longer exists).
    let mut process_gone = false;
    for _ in 0..200 {
        if kill(Pid::from_raw(pid), None).is_err() {
            process_gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        process_gone,
        "worker process {pid} was not terminated by SIGTERM within the poll window"
    );

    sup.cancel_token().cancel();
}

#[tokio::test]
async fn socket_path_auto_creates_parent_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let socket_path: PathBuf = dir.path().join("nested").join("deep").join("d.sock");
    let agent_dir = dir.path().join("agents");
    let store = AgentStore::new(dir.path().join("data"));
    let supervisor = Arc::new(Supervisor::new(socket_path.clone(), store, agent_dir));
    let server = Arc::clone(&supervisor);
    let _h = tokio::spawn(async move { Arc::clone(&server).serve().await });
    for _ in 0..200 {
        if socket_path.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(socket_path.exists(), "expected auto-created parent dirs");
    supervisor.cancel_token().cancel();
}
