//! End-to-end IPC tests for the supervisor daemon (in-process; the
//! supervisor runs on a tokio task while the client connects over the
//! same Unix socket).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use caliban_supervisor::proto::{AgentStatus, SpawnSpec};
use caliban_supervisor::store::AgentStore;
use caliban_supervisor::{Supervisor, SupervisorClient};

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

async fn boot() -> (
    tempfile::TempDir,
    Arc<Supervisor>,
    tokio::task::JoinHandle<std::io::Result<()>>,
    SupervisorClient,
) {
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("caliband.sock");
    let agent_dir = dir.path().join("agents-rt");
    let store = AgentStore::new(dir.path().join("data"));
    let supervisor = Arc::new(Supervisor::new(socket_path.clone(), store, agent_dir));
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
    let (_d, sup, _h, client) = boot().await;
    let (id, sock) = client.spawn(spec()).await.unwrap();
    assert!(!id.is_empty());
    assert!(
        sock.to_string_lossy().ends_with("-agent.sock"),
        "got {sock:?}"
    );
    let list = client.list().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, id);
    assert_eq!(list[0].status, AgentStatus::Spawning);
    sup.cancel_token().cancel();
}

#[tokio::test]
async fn kill_marks_agent_killed() {
    let (_d, sup, _h, client) = boot().await;
    let (id, _) = client.spawn(spec()).await.unwrap();
    client.kill(&id).await.unwrap();
    let list = client.list().await.unwrap();
    assert_eq!(list[0].status, AgentStatus::Killed);
    sup.cancel_token().cancel();
}

#[tokio::test]
async fn respawn_drops_old_and_creates_new_id() {
    let (_d, sup, _h, client) = boot().await;
    let (old_id, _) = client.spawn(spec()).await.unwrap();
    let new_id = client.respawn(&old_id).await.unwrap();
    assert_ne!(old_id, new_id);
    let list = client.list().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, new_id);
    sup.cancel_token().cancel();
}

#[tokio::test]
async fn rm_requires_stopped_state_without_force() {
    let (_d, sup, _h, client) = boot().await;
    let (id, _) = client.spawn(spec()).await.unwrap();
    let err = client.rm(&id, false).await.unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("invalid state"), "got {msg}");
    sup.cancel_token().cancel();
}

#[tokio::test]
async fn rm_with_force_succeeds() {
    let (_d, sup, _h, client) = boot().await;
    let (id, _) = client.spawn(spec()).await.unwrap();
    client.rm(&id, true).await.unwrap();
    assert!(client.list().await.unwrap().is_empty());
    sup.cancel_token().cancel();
}

#[tokio::test]
async fn rm_after_kill_succeeds_without_force() {
    let (_d, sup, _h, client) = boot().await;
    let (id, _) = client.spawn(spec()).await.unwrap();
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
