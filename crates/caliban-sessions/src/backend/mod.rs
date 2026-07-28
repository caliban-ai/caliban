//! The storage seam for persisted sessions.
use async_trait::async_trait;

use crate::error::Result;
use crate::session::PersistedSession;
use crate::store::SessionMetadata;

pub(crate) mod fs;
pub use fs::FsSessionBackend;

#[cfg(feature = "gonzalo")]
pub mod gonzalo;
#[cfg(feature = "gonzalo")]
pub use gonzalo::GonzaloSessionBackend;

#[cfg(test)]
pub(crate) mod conformance;

/// Substrate-neutral CRUD over persisted sessions.
#[async_trait]
pub trait SessionBackend: Send + Sync {
    /// Persist `session` (create or overwrite by name).
    async fn save(&self, session: &PersistedSession) -> Result<()>;
    /// Load a session by name; `Ok(None)` if it does not exist.
    async fn load(&self, name: &str) -> Result<Option<PersistedSession>>;
    /// List session metadata, sorted by `updated_at` descending.
    async fn list(&self) -> Result<Vec<SessionMetadata>>;
    /// Delete a session by name; idempotent (`Ok(())` if absent).
    async fn delete(&self, name: &str) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MockBackend {
        saved: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl SessionBackend for MockBackend {
        async fn save(&self, session: &PersistedSession) -> Result<()> {
            self.saved.lock().unwrap().push(session.name.clone());
            Ok(())
        }
        async fn load(&self, _name: &str) -> Result<Option<PersistedSession>> {
            Ok(None)
        }
        async fn list(&self) -> Result<Vec<SessionMetadata>> {
            Ok(Vec::new())
        }
        async fn delete(&self, _name: &str) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn trait_is_object_safe_and_delegates_as_dyn() {
        let be: Box<dyn SessionBackend> = Box::new(MockBackend::default());
        be.save(&PersistedSession::new("s", "anthropic", "m"))
            .await
            .unwrap();
        assert!(be.load("s").await.unwrap().is_none());
        assert!(be.list().await.unwrap().is_empty());
    }
}
