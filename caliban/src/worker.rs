//! The `caliban __agent-worker` entry point (ADR 0037, #71).
//!
//! Launched by the `caliband` supervisor as a child process. Reads the
//! agent's manifest, binds the per-agent socket, runs the agent loop to
//! completion, and exits with a code the supervisor maps to a terminal
//! status (0 = Done, non-zero = Failed).

use std::path::Path;

use caliban_supervisor::proto::AgentRecord;

/// Load the `AgentRecord` the supervisor wrote for this worker.
pub(crate) fn load_record(manifest: &Path) -> std::io::Result<AgentRecord> {
    let body = std::fs::read(manifest)?;
    serde_json::from_slice(&body).map_err(std::io::Error::other)
}

/// Entry point body. Returns the process exit code.
#[allow(clippy::unused_async)] // Task 7 will add `.await` points (socket binding + agent loop)
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
    // Plan A Task 7 fills in socket binding + the agent loop.
    let _ = (&record, socket);
    0
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
