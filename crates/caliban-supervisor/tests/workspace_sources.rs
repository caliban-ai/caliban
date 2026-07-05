//! Acceptance integration test for #281 (workspace-scoped caliband): one
//! caliband instance supervises agents across >= 2 source repos in a single
//! workspace, with per-source worktree isolation.
//!
//! The worktree is created by the daemon during `Spawn` dispatch, before the
//! worker is ever launched (see `server.rs`), so a fake `WorkerLauncher` (no
//! LLM, no real agent) is sufficient here: the worktrees themselves are real
//! (materialized via `caliban_worktrees::WorktreeManager` against real git
//! checkouts) and `AgentRecord.working_dir` is populated by the daemon before
//! the worker ever starts. This test asserts against the daemon's `list()`
//! and `spawn()` replies, not worker behavior.

use std::sync::Arc;
use std::time::Duration;

use caliban_supervisor::proto::{AgentRecord, SpawnSpec};
use caliban_supervisor::store::AgentStore;
use caliban_supervisor::{Supervisor, SupervisorClient, WorkerHandle, WorkerLauncher};

/// Minimal fake launcher: runs a trivial, near-instant child process so the
/// supervisor has a real PID to track. It never touches the per-agent
/// socket, so status stays whatever the registration set it to — this test
/// only cares about `working_dir` from `list()`/`spawn()`, not worker
/// lifecycle.
struct NoopLauncher;

impl WorkerLauncher for NoopLauncher {
    fn launch(&self, _record: &AgentRecord) -> std::io::Result<WorkerHandle> {
        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.arg("-c").arg("sleep 0.1");
        let child = cmd.spawn()?;
        let pid = child.id().expect("child pid");
        Ok(WorkerHandle { pid, child })
    }
}

fn spawn_spec(source: Option<&str>, isolation_worktree: bool) -> SpawnSpec {
    SpawnSpec {
        label: None,
        frontmatter_path: None,
        initial_prompt: "hi".into(),
        model: None,
        provider: None,
        tool_allowlist: None,
        isolation_worktree,
        inherit_hooks: true,
        interactive: false,
        inherited_hooks_config: None,
        source: source.map(str::to_string),
    }
}

/// Initialize a real git checkout at `dir` with one empty commit, so it has
/// a valid HEAD for `WorktreeManager` to branch from.
fn init_git_checkout(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).expect("create source dir");
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir)
        .status()
        .expect("git init");
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
        .current_dir(dir)
        .status()
        .expect("git commit --allow-empty");
}

async fn find_record(client: &SupervisorClient, id: &str) -> AgentRecord {
    client
        .list()
        .await
        .expect("list")
        .into_iter()
        .find(|a| a.id == id)
        .unwrap_or_else(|| panic!("agent {id} not found in list()"))
}

#[tokio::test]
async fn one_caliband_supervises_two_sources_with_worktree_isolation() {
    // --- Arrange: a workspace with two real, independent git checkouts.
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace_root = dir.path().join("workspace");
    let alpha = workspace_root.join("alpha");
    let beta = workspace_root.join("beta");
    init_git_checkout(&alpha);
    init_git_checkout(&beta);

    let socket_path = dir.path().join("caliband.sock");
    let agent_dir = dir.path().join("agents-rt");
    let store = AgentStore::new(dir.path().join("data"));
    let launcher = Arc::new(NoopLauncher);
    let supervisor = Arc::new(
        Supervisor::with_launcher(socket_path.clone(), store, agent_dir, launcher)
            .with_workspace_root(&workspace_root),
    );
    let server = Arc::clone(&supervisor);
    let _handle = tokio::spawn(async move { Arc::clone(&server).serve().await });
    for _ in 0..200 {
        if socket_path.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(socket_path.exists(), "supervisor should have bound socket");
    let client = SupervisorClient::new(socket_path);

    // --- Act: spawn one agent per source, each isolated in its own worktree.
    let (id_a, _) = client
        .spawn(spawn_spec(Some("alpha"), true))
        .await
        .expect("spawn agent A (alpha) should succeed");
    let (id_b, _) = client
        .spawn(spawn_spec(Some("beta"), true))
        .await
        .expect("spawn agent B (beta) should succeed");

    let record_a = find_record(&client, &id_a).await;
    let record_b = find_record(&client, &id_b).await;

    // --- Assert 1: each agent's working_dir is under its own source's
    // `.caliban/worktrees/` directory.
    let alpha_worktrees = alpha.join(".caliban").join("worktrees");
    let beta_worktrees = beta.join(".caliban").join("worktrees");
    assert!(
        record_a.working_dir.starts_with(&alpha_worktrees),
        "agent A working_dir {:?} should be under {:?}",
        record_a.working_dir,
        alpha_worktrees
    );
    assert!(
        record_b.working_dir.starts_with(&beta_worktrees),
        "agent B working_dir {:?} should be under {:?}",
        record_b.working_dir,
        beta_worktrees
    );

    // --- Assert 2: the two working dirs are different directories.
    assert_ne!(
        record_a.working_dir, record_b.working_dir,
        "per-source worktrees must isolate the two agents into distinct directories"
    );

    // --- Assert 3: write isolation — a file written into A's worktree must
    // not appear under B's worktree (or vice versa).
    assert!(
        record_a.working_dir.is_dir(),
        "agent A working_dir {:?} should exist on disk",
        record_a.working_dir
    );
    assert!(
        record_b.working_dir.is_dir(),
        "agent B working_dir {:?} should exist on disk",
        record_b.working_dir
    );
    let marker_name = "alpha-only-marker.txt";
    std::fs::write(record_a.working_dir.join(marker_name), b"alpha wrote this")
        .expect("write marker file into agent A's worktree");
    assert!(
        record_a.working_dir.join(marker_name).exists(),
        "marker file should exist in A's own worktree"
    );
    assert!(
        !record_b.working_dir.join(marker_name).exists(),
        "marker file written into A's worktree must not leak into B's worktree"
    );

    // --- Back-compat: an agent with no source and no isolation gets the
    // workspace root as its working_dir.
    let (id_c, _) = client
        .spawn(spawn_spec(None, false))
        .await
        .expect("spawn agent C (no source) should succeed");
    let record_c = find_record(&client, &id_c).await;
    assert_eq!(
        record_c.working_dir, workspace_root,
        "an agent with source: None, isolation_worktree: false should get the workspace root as its working_dir"
    );

    supervisor.cancel_token().cancel();
}
