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
                provider: None,
                tool_allowlist: None,
                isolation_worktree: false,
                inherit_hooks: true,
                interactive: false,
                inherited_hooks_config: None,
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

    #[test]
    fn write_manifest_preserves_all_fields() {
        let dir = tempfile::tempdir().unwrap();
        let store = AgentStore::new(dir.path().join("agents"));
        let mut rec = fake_record("full");
        rec.status = AgentStatus::Running;
        rec.session_dir = PathBuf::from("/data/sessions/full");
        rec.socket_path = PathBuf::from("/data/sessions/full/agent.sock");
        rec.spec = SpawnSpec {
            label: Some("worker".into()),
            frontmatter_path: Some(PathBuf::from("/fm/agent.md")),
            initial_prompt: "do the thing".into(),
            model: Some("opus".into()),
            provider: None,
            tool_allowlist: Some(vec!["read".into(), "write".into()]),
            isolation_worktree: true,
            inherit_hooks: false,
            interactive: false,
            inherited_hooks_config: None,
        };
        store.write_manifest(&rec).unwrap();

        let loaded = store.load_manifest("full").unwrap().unwrap();
        assert_eq!(loaded.id, "full");
        assert_eq!(loaded.status, AgentStatus::Running);
        assert_eq!(loaded.started_at, rec.started_at);
        assert_eq!(loaded.session_dir, PathBuf::from("/data/sessions/full"));
        assert_eq!(
            loaded.socket_path,
            PathBuf::from("/data/sessions/full/agent.sock")
        );
        assert_eq!(loaded.spec.label.as_deref(), Some("worker"));
        assert_eq!(
            loaded.spec.frontmatter_path,
            Some(PathBuf::from("/fm/agent.md"))
        );
        assert_eq!(loaded.spec.initial_prompt, "do the thing");
        assert_eq!(loaded.spec.model.as_deref(), Some("opus"));
        assert_eq!(
            loaded.spec.tool_allowlist,
            Some(vec!["read".into(), "write".into()])
        );
        assert!(loaded.spec.isolation_worktree);
        assert!(!loaded.spec.inherit_hooks);
    }

    #[test]
    fn write_manifest_overwrites_existing() {
        let dir = tempfile::tempdir().unwrap();
        let store = AgentStore::new(dir.path().join("agents"));
        let mut rec = fake_record("over");
        store.write_manifest(&rec).unwrap();
        rec.status = AgentStatus::Done;
        rec.name = "renamed".into();
        store.write_manifest(&rec).unwrap();

        let loaded = store.load_manifest("over").unwrap().unwrap();
        assert_eq!(loaded.status, AgentStatus::Done);
        assert_eq!(loaded.name, "renamed");
    }

    #[test]
    fn load_manifest_absent_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let store = AgentStore::new(dir.path().join("agents"));
        assert!(store.load_manifest("nope").unwrap().is_none());
    }

    #[test]
    fn load_manifest_corrupt_json_errors() {
        let dir = tempfile::tempdir().unwrap();
        let store = AgentStore::new(dir.path().join("agents"));
        let sdir = store.ensure_dir("bad").unwrap();
        fs::write(sdir.join("manifest.json"), b"{ not valid json").unwrap();

        let err = store.load_manifest("bad").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Other);
    }

    #[test]
    fn load_manifest_wrong_schema_errors() {
        let dir = tempfile::tempdir().unwrap();
        let store = AgentStore::new(dir.path().join("agents"));
        let sdir = store.ensure_dir("schema").unwrap();
        // Valid JSON, but missing required AgentRecord fields.
        fs::write(sdir.join("manifest.json"), br#"{"id":"schema"}"#).unwrap();

        assert!(store.load_manifest("schema").is_err());
    }

    #[test]
    fn list_empty_when_base_missing() {
        let dir = tempfile::tempdir().unwrap();
        let store = AgentStore::new(dir.path().join("does-not-exist"));
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn list_empty_when_base_present_but_no_agents() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("agents");
        fs::create_dir_all(&base).unwrap();
        let store = AgentStore::new(base);
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn list_ignores_non_dir_entries() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("agents");
        let store = AgentStore::new(&base);
        store.write_manifest(&fake_record("real")).unwrap();
        // A stray file directly under base should be skipped.
        fs::write(base.join("stray.txt"), b"junk").unwrap();

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "real");
    }

    #[test]
    fn list_skips_dirs_without_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("agents");
        let store = AgentStore::new(&base);
        store.write_manifest(&fake_record("has")).unwrap();
        // A session dir with no manifest.json must be skipped silently.
        fs::create_dir_all(base.join("empty-dir")).unwrap();

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "has");
    }

    #[test]
    fn remove_nonexistent_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let store = AgentStore::new(dir.path().join("agents"));
        // Must not error even though the dir was never created.
        store.remove("ghost").unwrap();
    }

    #[test]
    fn ensure_dir_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let store = AgentStore::new(dir.path().join("agents"));
        let first = store.ensure_dir("idem").unwrap();
        let second = store.ensure_dir("idem").unwrap();
        assert_eq!(first, second);
        assert!(first.is_dir());
        assert_eq!(first, store.session_dir("idem"));
    }

    #[test]
    fn base_and_session_dir_paths() {
        let base = std::env::temp_dir().join("caliban-test-base");
        let store = AgentStore::new(&base);
        assert_eq!(store.base(), base.as_path());
        assert_eq!(store.session_dir("k"), base.join("k"));
    }

    #[test]
    fn default_for_includes_projects_and_agents_and_sanitizes() {
        let store = AgentStore::default_for(Path::new("/home/me/my repo"));
        let base = store.base();
        let s = base.to_string_lossy();
        assert!(s.ends_with("agents"), "got: {s}");
        assert!(s.contains("projects"), "got: {s}");
        // The space in "my repo" must be sanitized away (no raw space).
        let sanitized_component = base
            .components()
            .nth_back(1)
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .unwrap();
        assert!(
            !sanitized_component.contains(' '),
            "got: {sanitized_component}"
        );
    }

    #[test]
    fn sanitize_path_keeps_safe_chars_and_replaces_others() {
        let out = sanitize_path(Path::new("/a-b_c.9/x y!z"));
        // Slash, space, and '!' become '-'; alnum + . - _ are kept.
        assert!(
            out.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
        );
        assert!(out.contains("a-b_c.9"));
        assert!(!out.contains(' '));
        assert!(!out.contains('/'));
        assert!(!out.contains('!'));
    }

    // --- SpawnSpec.provider serde (#93) ---

    #[test]
    fn provider_field_roundtrips_through_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let store = AgentStore::new(dir.path().join("agents"));
        let mut rec = fake_record("prov");
        rec.spec.provider = Some("ollama".into());
        store.write_manifest(&rec).unwrap();
        let loaded = store.load_manifest("prov").unwrap().unwrap();
        assert_eq!(loaded.spec.provider.as_deref(), Some("ollama"));
    }

    #[test]
    fn provider_absent_in_old_manifest_deserializes_to_none() {
        // Simulate a manifest written before #93 (no "provider" key).
        let dir = tempfile::tempdir().unwrap();
        let store = AgentStore::new(dir.path().join("agents"));
        let sdir = store.ensure_dir("old").unwrap();
        // Write a valid AgentRecord JSON without a "provider" field in spec.
        let json = r#"{
            "id": "old",
            "name": "old",
            "status": "spawning",
            "started_at": "2026-01-01T00:00:00Z",
            "session_dir": "/tmp/old",
            "socket_path": "/tmp/old.sock",
            "spec": {
                "initial_prompt": "hi",
                "inherit_hooks": true
            }
        }"#;
        fs::write(sdir.join("manifest.json"), json.as_bytes()).unwrap();
        let loaded = store.load_manifest("old").unwrap().unwrap();
        assert_eq!(loaded.spec.provider, None);
    }
}
