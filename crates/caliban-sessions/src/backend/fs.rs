//! Filesystem-backed session store (the gonzalo-free default).
use std::path::PathBuf;

use async_trait::async_trait;

use crate::backend::SessionBackend;
use crate::error::Result;
use crate::session::PersistedSession;
use crate::store::SessionMetadata;

/// Filesystem session store: one pretty-JSON `<name>.json` per session.
#[derive(Debug, Clone)]
pub struct FsSessionBackend {
    // Read by the real impl in Task 2; the Task 1 stub never touches it.
    #[allow(dead_code)]
    root: PathBuf,
}

impl FsSessionBackend {
    /// Construct a backend over `root`. The directory need not exist yet.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

#[async_trait]
impl SessionBackend for FsSessionBackend {
    async fn save(&self, _session: &PersistedSession) -> Result<()> {
        unimplemented!("Task 2")
    }
    async fn load(&self, _name: &str) -> Result<Option<PersistedSession>> {
        unimplemented!("Task 2")
    }
    async fn list(&self) -> Result<Vec<SessionMetadata>> {
        unimplemented!("Task 2")
    }
    async fn delete(&self, _name: &str) -> Result<()> {
        unimplemented!("Task 2")
    }
}
