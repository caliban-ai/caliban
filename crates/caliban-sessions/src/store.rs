//! `SessionStore` — disk-backed CRUD over `PersistedSession`.

use std::cmp::Reverse;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::error::{Error, Result};
use crate::session::PersistedSession;

const MAX_NAME_LEN: usize = 64;

fn validate_name(name: &str) -> Result<()> {
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

/// On-disk session store.
#[derive(Debug, Clone)]
pub struct SessionStore {
    root: PathBuf,
}

impl SessionStore {
    /// Construct a store with the given root directory.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Resolve the default root: `$XDG_DATA_HOME/caliban/sessions`
    /// or `$HOME/.local/share/caliban/sessions`.
    ///
    /// # Errors
    /// Returns `Error::NoHome` if neither `XDG_DATA_HOME` nor HOME are available.
    pub fn default_root() -> Result<PathBuf> {
        let base = dirs::data_dir().ok_or(Error::NoHome)?;
        Ok(base.join("caliban").join("sessions"))
    }

    /// Get the path for a named session.
    #[must_use]
    pub fn path_for(&self, name: &str) -> PathBuf {
        self.root.join(format!("{name}.json"))
    }

    /// Load a session by name. Returns Ok(None) if the file doesn't exist.
    ///
    /// # Errors
    /// I/O, deserialization, or name-validation errors.
    pub fn load(&self, name: &str) -> Result<Option<PersistedSession>> {
        validate_name(name)?;
        let path = self.path_for(name);
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Save a session atomically.
    ///
    /// # Errors
    /// I/O, serialization, or name-validation errors.
    pub fn save(&self, session: &PersistedSession) -> Result<()> {
        validate_name(&session.name)?;
        std::fs::create_dir_all(&self.root)?;
        let serialized = serde_json::to_vec_pretty(session)?;
        // Atomic write: write to temp file in the same dir, then persist (rename).
        let tmp = NamedTempFile::new_in(&self.root)?;
        std::fs::write(tmp.path(), &serialized)?;
        let target = self.path_for(&session.name);
        tmp.persist(&target).map_err(|e| Error::Io(e.error))?;
        Ok(())
    }

    /// List sessions (their metadata) sorted by `updated_at` descending.
    ///
    /// # Errors
    /// I/O errors. Individual broken files are SKIPPED with no error.
    pub fn list(&self) -> Result<Vec<SessionMetadata>> {
        let mut out = Vec::new();
        let entries = match std::fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
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
            out.push(SessionMetadata {
                name: session.name,
                updated_at: session.updated_at,
                turn_count: u32::try_from(
                    session
                        .messages
                        .iter()
                        .filter(|m| m.role == caliban_provider::Role::Assistant)
                        .count(),
                )
                .unwrap_or(u32::MAX),
                total_tokens: session
                    .total_usage
                    .input_tokens
                    .saturating_add(session.total_usage.output_tokens),
            });
        }
        out.sort_by_key(|b| Reverse(b.updated_at));
        Ok(out)
    }

    /// Delete a session.
    ///
    /// # Errors
    /// I/O or name-validation errors.
    pub fn delete(&self, name: &str) -> Result<()> {
        validate_name(name)?;
        let path = self.path_for(name);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// Metadata returned by `SessionStore::list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    /// The session name.
    pub name: String,
    /// When the session was last modified.
    pub updated_at: DateTime<Utc>,
    /// Number of completed assistant turns.
    pub turn_count: u32,
    /// Total tokens consumed (input + output) across all turns.
    pub total_tokens: u32,
}
