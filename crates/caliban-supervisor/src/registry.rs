//! In-memory registry of running sub-agents.
//!
//! The supervisor stores live agent state here and persists to
//! [`crate::store::AgentStore`] on every mutation so a daemon crash +
//! restart can reconstruct the world.

use std::collections::HashMap;

use chrono::Utc;

use crate::proto::{AgentId, AgentRecord, AgentStatus, SpawnSpec, SupervisorError};
use crate::store::AgentStore;

/// Live registry. Cheap to construct; one per daemon.
#[derive(Debug)]
pub struct Registry {
    by_id: HashMap<AgentId, AgentRecord>,
    store: AgentStore,
}

impl Registry {
    /// Build a registry backed by `store`. Loads any existing manifests.
    pub fn new(store: AgentStore) -> Self {
        let mut by_id = HashMap::new();
        if let Ok(records) = store.list() {
            for r in records {
                by_id.insert(r.id.clone(), r);
            }
        }
        Self { by_id, store }
    }

    /// Iterate over registered agents.
    pub fn list(&self) -> Vec<AgentRecord> {
        let mut out: Vec<_> = self.by_id.values().cloned().collect();
        out.sort_by(|a, b| a.started_at.cmp(&b.started_at));
        out
    }

    /// Number of registered agents.
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// True iff the registry has no agents.
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Look up an agent by id.
    pub fn get(&self, id: &str) -> Option<&AgentRecord> {
        self.by_id.get(id)
    }

    /// Register a new agent. Picks a fresh id, persists the manifest,
    /// and returns the assigned record.
    pub fn register(&mut self, spec: SpawnSpec, socket_path: std::path::PathBuf) -> AgentRecord {
        let id = new_id();
        let session_dir = self.store.session_dir(&id);
        let record = AgentRecord {
            id: id.clone(),
            name: spec.label.clone().unwrap_or_else(|| format!("agent-{id}")),
            status: AgentStatus::Spawning,
            started_at: Utc::now().to_rfc3339(),
            session_dir,
            socket_path,
            spec,
        };
        // Best-effort persistence — IO errors get logged but don't block
        // registration. (Tests use tempdirs so this is reliable.)
        if let Err(e) = self.store.write_manifest(&record) {
            tracing::warn!(error = %e, "manifest write failed (registration continues)");
        }
        self.by_id.insert(id, record.clone());
        record
    }

    /// Mutate the status of a registered agent (e.g. on `kill`,
    /// completion). Persists the manifest. Returns `NotFound` if the id
    /// is unknown.
    pub fn set_status(
        &mut self,
        id: &str,
        status: AgentStatus,
    ) -> Result<AgentRecord, SupervisorError> {
        let rec = self
            .by_id
            .get_mut(id)
            .ok_or_else(|| SupervisorError::NotFound { id: id.into() })?;
        rec.status = status;
        let cloned = rec.clone();
        if let Err(e) = self.store.write_manifest(&cloned) {
            tracing::warn!(error = %e, "manifest write failed during set_status");
        }
        Ok(cloned)
    }

    /// Set status to `to` ONLY if the agent is currently `Running` or
    /// `Spawning`. Used by the worker monitor task so a late child exit
    /// can't clobber a terminal state already set by `Kill`/`Rm`.
    /// Returns `true` if the transition was applied.
    pub fn set_status_if_running(&mut self, id: &str, to: AgentStatus) -> bool {
        let Some(rec) = self.by_id.get_mut(id) else {
            return false;
        };
        if !matches!(rec.status, AgentStatus::Running | AgentStatus::Spawning) {
            return false;
        }
        rec.status = to;
        let cloned = rec.clone();
        if let Err(e) = self.store.write_manifest(&cloned) {
            tracing::warn!(error = %e, "manifest write failed during set_status_if_running");
        }
        true
    }

    /// Remove an agent from the registry. Refuses removal of running
    /// agents unless `force` is set. Always best-effort deletes the
    /// on-disk session dir.
    pub fn remove(&mut self, id: &str, force: bool) -> Result<(), SupervisorError> {
        let status = self
            .by_id
            .get(id)
            .map(|r| r.status)
            .ok_or_else(|| SupervisorError::NotFound { id: id.into() })?;
        let stopped = matches!(
            status,
            AgentStatus::Killed | AgentStatus::Done | AgentStatus::Failed | AgentStatus::Crashed
        );
        if !stopped && !force {
            return Err(SupervisorError::InvalidState {
                op: "rm".into(),
                id: id.into(),
                status,
            });
        }
        self.by_id.remove(id);
        if let Err(e) = self.store.remove(id) {
            tracing::warn!(error = %e, "session-dir cleanup failed during rm");
        }
        Ok(())
    }

    /// On daemon startup, sweep `Running` / `Working` agents to
    /// `Crashed`. Returns the list of swept ids.
    pub fn sweep_crashed(&mut self) -> Vec<AgentId> {
        let mut out = Vec::new();
        for (id, rec) in &mut self.by_id {
            if matches!(rec.status, AgentStatus::Running | AgentStatus::Spawning) {
                rec.status = AgentStatus::Crashed;
                out.push(id.clone());
            }
        }
        // Persist the changes.
        for id in &out {
            if let Some(rec) = self.by_id.get(id).cloned()
                && let Err(e) = self.store.write_manifest(&rec)
            {
                tracing::warn!(error = %e, "manifest write failed during sweep");
            }
        }
        out
    }
}

/// Generate a 12-char id (8 random bytes hex-encoded → 16 chars,
/// truncated to 12 for legibility).
fn new_id() -> AgentId {
    use std::fmt::Write as _;
    let mut bytes = [0u8; 8];
    // Use a basic time-based salt + UUID for randomness; we don't need
    // crypto-grade entropy here.
    let id = uuid::Uuid::new_v4();
    bytes.copy_from_slice(&id.as_bytes()[..8]);
    let mut s = String::with_capacity(16);
    for b in &bytes {
        let _ = write!(s, "{b:02x}");
    }
    s.truncate(12);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, AgentStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = AgentStore::new(dir.path().join("agents"));
        (dir, store)
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

    #[test]
    fn empty_registry_lists_nothing() {
        let (_d, s) = store();
        let r = Registry::new(s);
        assert!(r.is_empty());
        assert_eq!(r.list().len(), 0);
    }

    #[test]
    fn register_assigns_id_and_persists() {
        let (_d, s) = store();
        let mut r = Registry::new(s.clone());
        let rec = r.register(spec(), std::path::PathBuf::from("/tmp/x.sock"));
        assert_eq!(rec.status, AgentStatus::Spawning);
        assert_eq!(r.len(), 1);
        // Persisted to disk.
        let loaded = s.load_manifest(&rec.id).unwrap().unwrap();
        assert_eq!(loaded.id, rec.id);
    }

    #[test]
    fn rm_refuses_running_without_force() {
        let (_d, s) = store();
        let mut r = Registry::new(s);
        let rec = r.register(spec(), std::path::PathBuf::from("/tmp/x.sock"));
        let err = r.remove(&rec.id, false).unwrap_err();
        assert!(matches!(err, SupervisorError::InvalidState { .. }));
    }

    #[test]
    fn rm_with_force_drops_running() {
        let (_d, s) = store();
        let mut r = Registry::new(s);
        let rec = r.register(spec(), std::path::PathBuf::from("/tmp/x.sock"));
        r.remove(&rec.id, true).unwrap();
        assert!(r.get(&rec.id).is_none());
    }

    #[test]
    fn rm_stopped_succeeds() {
        let (_d, s) = store();
        let mut r = Registry::new(s);
        let rec = r.register(spec(), std::path::PathBuf::from("/tmp/x.sock"));
        r.set_status(&rec.id, AgentStatus::Done).unwrap();
        r.remove(&rec.id, false).unwrap();
        assert!(r.get(&rec.id).is_none());
    }

    #[test]
    fn set_status_if_running_guards_terminal_states() {
        let (_d, s) = store();
        let mut r = Registry::new(s);
        let rec = r.register(spec(), std::path::PathBuf::from("/tmp/x.sock"));
        // Spawning -> Running allowed.
        assert!(r.set_status_if_running(&rec.id, AgentStatus::Running));
        // Move to a terminal state directly.
        r.set_status(&rec.id, AgentStatus::Killed).unwrap();
        // Guard refuses to move a Killed agent.
        assert!(!r.set_status_if_running(&rec.id, AgentStatus::Done));
        assert_eq!(r.get(&rec.id).unwrap().status, AgentStatus::Killed);
    }

    #[test]
    fn sweep_crashed_marks_running_agents() {
        let (_d, s) = store();
        let mut r = Registry::new(s);
        let rec = r.register(spec(), std::path::PathBuf::from("/tmp/x.sock"));
        let swept = r.sweep_crashed();
        assert_eq!(swept, vec![rec.id.clone()]);
        assert_eq!(r.get(&rec.id).unwrap().status, AgentStatus::Crashed);
    }
}
