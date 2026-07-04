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
        cmd.arg("-c").arg(&self.script).env(
            "SOCK",
            record
                .unix_socket_path()
                .expect("test launcher only ever registers Unix endpoints"),
        );
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
        provider: None,
        tool_allowlist: None,
        isolation_worktree: false,
        inherit_hooks: true,
        interactive: false,
        inherited_hooks_config: None,
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
    let (id, endpoint) = client.spawn(spec()).await.unwrap();
    let caliban_supervisor::Endpoint::Unix { path: sock } = endpoint else {
        panic!("expected unix endpoint")
    };
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
    let sock = sup
        .socket_path()
        .expect("unix control socket in default mode")
        .to_path_buf();
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
    // Stay alive ~1s so the per-agent socket is observably present before
    // exit — the monitor now unlinks it on worker exit (#77), so a
    // fast-exit worker would race the socket-created assertion below.
    let launcher = Arc::new(ShLauncher {
        script: "touch \"$SOCK\"; sleep 1".into(),
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
        provider: None,
        tool_allowlist: None,
        isolation_worktree: false,
        inherit_hooks: true,
        interactive: false,
        inherited_hooks_config: None,
    };
    let (id, endpoint) = client.spawn(spec).await.unwrap();
    let caliban_supervisor::Endpoint::Unix { path: socket_path } = endpoint else {
        panic!("expected unix endpoint")
    };
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
    let (id, endpoint) = client.spawn(spec()).await.unwrap();
    let caliban_supervisor::Endpoint::Unix { path: sock } = endpoint else {
        panic!("expected unix endpoint")
    };

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

#[tokio::test]
async fn rm_force_signals_running_worker() {
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    // Same pid-recording trick as the kill test: `exec sleep` so the
    // recorded pid is the one the supervisor signals (#76).
    let launcher = Arc::new(ShLauncher {
        script: r#"echo $$ > "${SOCK}.pid"; touch "$SOCK"; exec sleep 30"#.into(),
    });
    let (_d, sup, _h, client) = boot_with(launcher).await;
    let (id, endpoint) = client.spawn(spec()).await.unwrap();
    let caliban_supervisor::Endpoint::Unix { path: sock } = endpoint else {
        panic!("expected unix endpoint")
    };

    // Wait for Running.
    let mut running = false;
    for _ in 0..200 {
        if client
            .list()
            .await
            .unwrap()
            .iter()
            .any(|a| a.id == id && a.status == AgentStatus::Running)
        {
            running = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(running, "worker never reached Running");

    // Read the worker pid from the file the script wrote.
    let pid_file = {
        let mut p = sock.into_os_string();
        p.push(".pid");
        PathBuf::from(p)
    };
    let mut pid_str: Option<String> = None;
    for _ in 0..200 {
        if let Ok(s) = std::fs::read_to_string(&pid_file)
            && !s.trim().is_empty()
        {
            pid_str = Some(s);
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    #[allow(clippy::cast_possible_wrap)] // pids fit in i32 on all supported unix platforms
    let pid: i32 = pid_str
        .expect("pid file never appeared")
        .trim()
        .parse()
        .expect("pid file did not contain an integer");
    assert!(
        kill(Pid::from_raw(pid), None).is_ok(),
        "worker {pid} should be alive before rm"
    );

    // rm --force on a Running agent must signal the worker AND remove it.
    client.rm(&id, true).await.unwrap();

    // Registry entry is gone.
    assert!(
        !client.list().await.unwrap().iter().any(|a| a.id == id),
        "agent should be removed after rm --force"
    );

    // Worker process is actually terminated (not orphaned).
    let mut gone = false;
    for _ in 0..200 {
        if kill(Pid::from_raw(pid), None).is_err() {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(gone, "rm --force did not terminate worker {pid}");

    sup.cancel_token().cancel();
}

#[tokio::test]
async fn report_status_transitions_idle_running() {
    // Use a long-running fake worker so the agent stays Running (not Done)
    // while we drive status transitions from the client side.
    let launcher = Arc::new(ShLauncher {
        script: "touch \"$SOCK\"; sleep 30".into(),
    });
    let (_d, sup, _h, client) = boot_with(launcher).await;
    let (id, _) = client.spawn(spec()).await.unwrap();

    // Poll until agent reaches Running.
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
    assert!(reached_running, "agent never reached Running");

    // Worker reports Idle → list shows Idle.
    client.report_status(&id, AgentStatus::Idle).await.unwrap();
    let agents = client.list().await.unwrap();
    let a = agents.iter().find(|a| a.id == id).unwrap();
    assert_eq!(a.status, AgentStatus::Idle, "expected Idle after report");

    // Worker reports Running → list shows Running again.
    client
        .report_status(&id, AgentStatus::Running)
        .await
        .unwrap();
    let agents = client.list().await.unwrap();
    let a = agents.iter().find(|a| a.id == id).unwrap();
    assert_eq!(
        a.status,
        AgentStatus::Running,
        "expected Running after second report"
    );

    sup.cancel_token().cancel();
}

/// A [`Signaller`] that blocks the first signal in-flight until the test
/// releases it, recording every pid it was asked to signal. This lets the
/// test freeze `Kill` at the exact instant it delivers SIGTERM and observe
/// whether a concurrent `Respawn` can supersede the agent in that window.
struct BlockingSignaller {
    /// pids passed to `signal_term`, in order.
    signaled: std::sync::Mutex<Vec<u32>>,
    /// Fires (once) when the first signal is entered and now blocking.
    entered: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    /// The first signal blocks on this until the test sends `()`.
    release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    /// Ensures only the first signal blocks.
    blocked_once: std::sync::atomic::AtomicBool,
}

impl caliban_supervisor::Signaller for BlockingSignaller {
    fn signal_term(&self, pid: u32) -> bool {
        self.signaled.lock().unwrap().push(pid);
        if !self
            .blocked_once
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            if let Some(tx) = self.entered.lock().unwrap().take() {
                let _ = tx.send(());
            }
            // Block this signal in-flight (sync, on a tokio worker thread)
            // until the test releases it.
            let _ = self.release.lock().unwrap().recv();
        }
        true
    }
}

/// Deterministic regression test for the `Kill`/`Respawn` lock-ordering race
/// (#115). With `Kill`'s signal frozen in-flight, a concurrent `Respawn` must
/// NOT be able to supersede the agent — otherwise `Kill` would have signaled a
/// pid that is being respawned away (a stale signal against a superseded pid).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[cfg(unix)]
async fn respawn_cannot_supersede_agent_while_kill_signal_in_flight() {
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let signaller = Arc::new(BlockingSignaller {
        signaled: std::sync::Mutex::new(Vec::new()),
        entered: std::sync::Mutex::new(Some(entered_tx)),
        release: std::sync::Mutex::new(release_rx),
        blocked_once: std::sync::atomic::AtomicBool::new(false),
    });

    // Long-lived worker so its pid stays in `procs` for Kill to signal. The
    // blocking signaller never delivers a real signal, so workers survive the
    // test; record pids to reap them at the end.
    let launcher = Arc::new(ShLauncher {
        script: r#"echo $$ > "${SOCK}.pid"; touch "$SOCK"; sleep 30"#.into(),
    });
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("caliband.sock");
    let agent_dir = dir.path().join("agents-rt");
    let store = AgentStore::new(dir.path().join("data"));
    let supervisor = Arc::new(
        Supervisor::with_launcher(socket_path.clone(), store, agent_dir, launcher)
            .with_signaller(Arc::clone(&signaller) as Arc<dyn caliban_supervisor::Signaller>),
    );
    let server = Arc::clone(&supervisor);
    let _h = tokio::spawn(async move { Arc::clone(&server).serve().await });
    for _ in 0..200 {
        if socket_path.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let client = SupervisorClient::new(socket_path.clone());
    let (id, _) = client.spawn(spec()).await.unwrap();
    // Wait until Running so the worker pid is tracked in `procs`.
    for _ in 0..200 {
        if client
            .list()
            .await
            .unwrap()
            .iter()
            .any(|a| a.id == id && a.status == AgentStatus::Running)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // Start Kill; it will read the pid, deliver the (blocking) signal, and
    // freeze in-flight.
    let kill_client = SupervisorClient::new(socket_path.clone());
    let kill_id = id.clone();
    let kill_task = tokio::spawn(async move { kill_client.kill(&kill_id).await });
    entered_rx.await.expect("Kill never entered the signaller");

    // With Kill's signal frozen, fire a concurrent Respawn of the same agent.
    let respawn_client = SupervisorClient::new(socket_path.clone());
    let respawn_id = id.clone();
    let mut respawn_task = tokio::spawn(async move { respawn_client.respawn(&respawn_id).await });

    // The fix serializes Kill and Respawn under the registry lock, so Respawn
    // must NOT complete while Kill holds it mid-signal. The bug lets Respawn
    // run to completion here, superseding the just-signaled worker.
    let progressed = tokio::time::timeout(Duration::from_secs(2), &mut respawn_task).await;
    assert!(
        progressed.is_err(),
        "Respawn superseded the agent while Kill's signal was in flight — \
         Kill signaled a pid that was being respawned away (stale signal \
         against a superseded pid)"
    );

    // Release Kill's signal and let both operations drain.
    release_tx.send(()).unwrap();
    let _ = kill_task.await;
    let _ = respawn_task.await;

    // Reap surviving `sleep 30` workers (the blocking signaller never killed
    // them).
    if let Ok(entries) = std::fs::read_dir(dir.path().join("agents-rt")) {
        for entry in entries.flatten() {
            let path = entry.path();
            #[allow(clippy::cast_possible_wrap)] // pids fit in i32 on unix
            if path.extension().is_some_and(|e| e == "pid")
                && let Ok(s) = std::fs::read_to_string(&path)
                && let Ok(pid) = s.trim().parse::<i32>()
            {
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(pid),
                    nix::sys::signal::Signal::SIGKILL,
                );
            }
        }
    }

    supervisor.cancel_token().cancel();
}

/// Deterministic regression test for the `Rm --force`/`Respawn` lock-ordering
/// race (#138, same class as #115). With `rm --force`'s signal frozen
/// in-flight, a concurrent `Respawn` must NOT be able to supersede the agent —
/// otherwise `rm` would have signaled a pid that is being respawned away.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[cfg(unix)]
async fn respawn_cannot_supersede_agent_while_rm_force_signal_in_flight() {
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let signaller = Arc::new(BlockingSignaller {
        signaled: std::sync::Mutex::new(Vec::new()),
        entered: std::sync::Mutex::new(Some(entered_tx)),
        release: std::sync::Mutex::new(release_rx),
        blocked_once: std::sync::atomic::AtomicBool::new(false),
    });

    // Long-lived worker so its pid stays in `procs` for rm --force to signal.
    let launcher = Arc::new(ShLauncher {
        script: r#"echo $$ > "${SOCK}.pid"; touch "$SOCK"; sleep 30"#.into(),
    });
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("caliband.sock");
    let agent_dir = dir.path().join("agents-rt");
    let store = AgentStore::new(dir.path().join("data"));
    let supervisor = Arc::new(
        Supervisor::with_launcher(socket_path.clone(), store, agent_dir, launcher)
            .with_signaller(Arc::clone(&signaller) as Arc<dyn caliban_supervisor::Signaller>),
    );
    let server = Arc::clone(&supervisor);
    let _h = tokio::spawn(async move { Arc::clone(&server).serve().await });
    for _ in 0..200 {
        if socket_path.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let client = SupervisorClient::new(socket_path.clone());
    let (id, _) = client.spawn(spec()).await.unwrap();
    // Wait until Running so the worker pid is tracked in `procs`.
    for _ in 0..200 {
        if client
            .list()
            .await
            .unwrap()
            .iter()
            .any(|a| a.id == id && a.status == AgentStatus::Running)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // Start `rm --force`; it will read the pid, deliver the (blocking) signal,
    // and freeze in-flight.
    let rm_client = SupervisorClient::new(socket_path.clone());
    let rm_id = id.clone();
    let rm_task = tokio::spawn(async move { rm_client.rm(&rm_id, true).await });
    entered_rx
        .await
        .expect("rm --force never entered the signaller");

    // With rm's signal frozen, fire a concurrent Respawn of the same agent.
    let respawn_client = SupervisorClient::new(socket_path.clone());
    let respawn_id = id.clone();
    let mut respawn_task = tokio::spawn(async move { respawn_client.respawn(&respawn_id).await });

    // The fix serializes rm --force and Respawn under the registry lock, so
    // Respawn must NOT complete while rm holds it mid-signal. The bug lets
    // Respawn run to completion here, superseding the just-signaled worker.
    let progressed = tokio::time::timeout(Duration::from_secs(2), &mut respawn_task).await;
    assert!(
        progressed.is_err(),
        "Respawn superseded the agent while rm --force's signal was in flight — \
         rm signaled a pid that was being respawned away (stale signal against \
         a superseded pid)"
    );

    // Release rm's signal and let both operations drain.
    release_tx.send(()).unwrap();
    let _ = rm_task.await;
    let _ = respawn_task.await;

    // Reap surviving `sleep 30` workers (the blocking signaller never killed
    // them).
    if let Ok(entries) = std::fs::read_dir(dir.path().join("agents-rt")) {
        for entry in entries.flatten() {
            let path = entry.path();
            #[allow(clippy::cast_possible_wrap)] // pids fit in i32 on unix
            if path.extension().is_some_and(|e| e == "pid")
                && let Ok(s) = std::fs::read_to_string(&path)
                && let Ok(pid) = s.trim().parse::<i32>()
            {
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(pid),
                    nix::sys::signal::Signal::SIGKILL,
                );
            }
        }
    }

    supervisor.cancel_token().cancel();
}

#[tokio::test]
async fn socket_file_removed_after_worker_exits() {
    // Worker creates the socket file then exits 0 → Done. The monitor task
    // must unlink the now-stale socket after the worker exits (#77).
    let launcher = Arc::new(ShLauncher {
        script: r#"touch "$SOCK"; exit 0"#.into(),
    });
    let (_d, sup, _h, client) = boot_with(launcher).await;
    let (id, endpoint) = client.spawn(spec()).await.unwrap();
    let caliban_supervisor::Endpoint::Unix { path: sock } = endpoint else {
        panic!("expected unix endpoint")
    };

    // Wait until the agent reaches Done.
    let mut done = false;
    for _ in 0..200 {
        if client
            .list()
            .await
            .unwrap()
            .iter()
            .any(|a| a.id == id && a.status == AgentStatus::Done)
        {
            done = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(done, "worker never reached Done");

    // The per-agent socket file must be cleaned up. The monitor sets the
    // terminal status then unlinks, so absence may lag the Done status.
    let mut socket_gone = false;
    for _ in 0..200 {
        if !sock.exists() {
            socket_gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        socket_gone,
        "per-agent socket {} was not cleaned up after exit",
        sock.display()
    );

    sup.cancel_token().cancel();
}
