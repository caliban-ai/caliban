//! On-disk store for sub-agent session directories.
//!
//! Layout (per `<base>/agents/<id>/`):
//! - `manifest.json` — JSON copy of `AgentRecord` (frontmatter + state +
//!   spawn time). Atomically written via tempfile + rename.
//! - `session.json` — caliban-sessions format (created by the sub-agent
//!   runtime itself, not by us).
//! - `stdout.ndjson` — append-only `TurnEvent` stream.
//! - `agent.sock` — per-agent Unix socket (managed by the daemon).
//!
//! The `<base>` defaults to `$XDG_DATA_HOME/caliban/projects/<sanitized-cwd>`
//! (matches the user's auto-memory layout); see [`AgentStore::default_for`].

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::proto::AgentRecord;

/// On-disk store for agent state. Cheap to clone (one `PathBuf`).
#[derive(Debug, Clone)]
pub struct AgentStore {
    base: PathBuf,
}

impl AgentStore {
    /// Open a store rooted at `base`. Caller is responsible for picking
    /// the right `<base>` (typically the project-scoped data dir).
    pub fn new(base: impl Into<PathBuf>) -> Self {
        Self { base: base.into() }
    }

    /// Default `<base>/agents` directory for the given repo root.
    /// Falls back to `<tempdir>/caliban-agents/<sanitized>` when the
    /// user has no data dir configured.
    #[must_use]
    pub fn default_for(repo_root: &Path) -> Self {
        let sanitized = sanitize_path(repo_root);
        let data_root = dirs::data_dir()
            .map_or_else(std::env::temp_dir, |d| d.join("caliban"))
            .join("projects")
            .join(sanitized)
            .join("agents");
        Self::new(data_root)
    }

    /// Base directory (under which `<id>/manifest.json` lives).
    pub fn base(&self) -> &Path {
        &self.base
    }

    /// Path to the session dir for the given agent id.
    pub fn session_dir(&self, id: &str) -> PathBuf {
        self.base.join(id)
    }

    /// Create the session dir for an agent (idempotent).
    pub fn ensure_dir(&self, id: &str) -> io::Result<PathBuf> {
        let dir = self.session_dir(id);
        fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    /// Atomically write a manifest for the given agent.
    pub fn write_manifest(&self, record: &AgentRecord) -> io::Result<()> {
        let dir = self.ensure_dir(&record.id)?;
        let manifest_path = dir.join("manifest.json");
        let tmp = dir.join("manifest.json.tmp");
        let body = serde_json::to_vec_pretty(record).map_err(io::Error::other)?;
        fs::write(&tmp, &body)?;
        fs::rename(&tmp, &manifest_path)?;
        Ok(())
    }

    /// Load a manifest from disk. Returns `None` if the file is absent.
    pub fn load_manifest(&self, id: &str) -> io::Result<Option<AgentRecord>> {
        let path = self.session_dir(id).join("manifest.json");
        if !path.exists() {
            return Ok(None);
        }
        let body = fs::read(&path)?;
        let rec: AgentRecord = serde_json::from_slice(&body).map_err(io::Error::other)?;
        Ok(Some(rec))
    }

    /// Remove a session dir entirely.
    pub fn remove(&self, id: &str) -> io::Result<()> {
        let dir = self.session_dir(id);
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        Ok(())
    }

    /// Enumerate all manifests under the base.
    pub fn list(&self) -> io::Result<Vec<AgentRecord>> {
        if !self.base.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in fs::read_dir(&self.base)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if let Some(record) = self.load_manifest(&name)? {
                out.push(record);
            }
        }
        Ok(out)
    }
}

fn sanitize_path(p: &Path) -> String {
    let s = p.to_string_lossy().to_string();
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{AgentRecord, AgentStatus, SpawnSpec};

    fn fake_record(id: &str) -> AgentRecord {
        AgentRecord {
            id: id.into(),
            name: "rec".into(),
            status: AgentStatus::Spawning,
            started_at: "2026-05-24T00:00:00Z".into(),
            session_dir: PathBuf::from("/tmp/x"),
            socket_path: PathBuf::from("/tmp/x.sock"),
            spec: SpawnSpec {
                label: None,
                frontmatter_path: None,
                initial_prompt: "hi".into(),
                model: None,
                tool_allowlist: None,
                isolation_worktree: false,
                inherit_hooks: true,
            },
        }
    }

    #[test]
    fn write_and_load_manifest_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let store = AgentStore::new(dir.path().join("agents"));
        let rec = fake_record("abc");
        store.write_manifest(&rec).unwrap();
        let loaded = store.load_manifest("abc").unwrap().unwrap();
        assert_eq!(loaded.id, "abc");
        assert_eq!(loaded.name, "rec");
    }

    #[test]
    fn list_returns_all_manifests() {
        let dir = tempfile::tempdir().unwrap();
        let store = AgentStore::new(dir.path().join("agents"));
        store.write_manifest(&fake_record("a")).unwrap();
        store.write_manifest(&fake_record("b")).unwrap();
        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 2);
    }

    #[test]
    fn remove_drops_session_dir() {
        let dir = tempfile::tempdir().unwrap();
        let store = AgentStore::new(dir.path().join("agents"));
        let rec = fake_record("z");
        store.write_manifest(&rec).unwrap();
        store.remove("z").unwrap();
        assert!(store.load_manifest("z").unwrap().is_none());
    }
}
