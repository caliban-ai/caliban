//! Filesystem-backed session store (the gonzalo-free default).
use std::cmp::Reverse;
use std::path::PathBuf;

use async_trait::async_trait;

use crate::backend::SessionBackend;
use crate::error::{Error, Result};
use crate::session::PersistedSession;
use crate::store::SessionMetadata;

/// Filesystem session store: one pretty-JSON `<name>.json` per session.
#[derive(Debug, Clone)]
pub struct FsSessionBackend {
    root: PathBuf,
}

const MAX_NAME_LEN: usize = 64;

/// Validate a session name: non-empty, `<= 64` chars, `[a-zA-Z0-9_-]+`.
pub(crate) fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > MAX_NAME_LEN {
        return Err(Error::InvalidName(name.into()));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(Error::InvalidName(name.into()));
    }
    Ok(())
}

impl FsSessionBackend {
    /// Construct a backend over `root`. The directory need not exist yet.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path_for(&self, name: &str) -> PathBuf {
        self.root.join(format!("{name}.json"))
    }
}

#[async_trait]
impl SessionBackend for FsSessionBackend {
    async fn save(&self, session: &PersistedSession) -> Result<()> {
        validate_name(&session.name)?;
        std::fs::create_dir_all(&self.root)?;
        let serialized = serde_json::to_vec_pretty(session)?;
        let target = self.path_for(&session.name);
        caliban_common::fs::write_atomic(&target, &serialized)
            .map_err(|e| Error::Persist(e.to_string()))?;
        Ok(())
    }

    async fn load(&self, name: &str) -> Result<Option<PersistedSession>> {
        validate_name(name)?;
        match std::fs::read(self.path_for(name)) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn list(&self) -> Result<Vec<SessionMetadata>> {
        let mut out = Vec::new();
        let entries = match std::fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e.into()),
        };
        for entry in entries {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let Ok(session): std::result::Result<PersistedSession, _> =
                serde_json::from_slice(&bytes)
            else {
                continue;
            };
            out.push(SessionMetadata::from_session(&session));
        }
        out.sort_by_key(|b| Reverse(b.updated_at));
        Ok(out)
    }

    async fn delete(&self, name: &str) -> Result<()> {
        validate_name(name)?;
        match std::fs::remove_file(self.path_for(name)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::conformance::run_session_backend_conformance;

    #[tokio::test]
    async fn fs_backend_passes_conformance() {
        let tmp = tempfile::tempdir().unwrap();
        let be = FsSessionBackend::new(tmp.path().to_path_buf());
        run_session_backend_conformance(&be).await;
    }

    #[tokio::test]
    async fn save_writes_pretty_json_file_named_by_session() {
        let tmp = tempfile::tempdir().unwrap();
        let be = FsSessionBackend::new(tmp.path().to_path_buf());
        be.save(&PersistedSession::new("mysess", "anthropic", "m"))
            .await
            .unwrap();
        let path = tmp.path().join("mysess.json");
        assert!(path.exists());
        let bytes = std::fs::read(&path).unwrap();
        // pretty JSON has newlines
        assert!(String::from_utf8_lossy(&bytes).contains('\n'));
    }

    #[tokio::test]
    async fn list_skips_broken_and_non_json_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path()).unwrap();
        std::fs::write(tmp.path().join("notjson.txt"), b"x").unwrap();
        std::fs::write(tmp.path().join("broken.json"), b"{ not json").unwrap();
        let be = FsSessionBackend::new(tmp.path().to_path_buf());
        be.save(&PersistedSession::new("good", "anthropic", "m"))
            .await
            .unwrap();
        let list = be.list().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "good");
    }

    #[tokio::test]
    async fn invalid_name_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let be = FsSessionBackend::new(tmp.path().to_path_buf());
        let bad = PersistedSession::new("../escape", "anthropic", "m");
        assert!(matches!(
            be.save(&bad).await,
            Err(crate::error::Error::InvalidName(_))
        ));
    }
}
