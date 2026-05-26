//! `SessionStore` — disk-backed CRUD over `PersistedSession`.
//!
//! Writes go through a [`DebouncedWriter`](crate::debounced) so a flurry
//! of intra-turn snapshots collapses into a single atomic file write
//! (see `docs/superpowers/specs/2026-05-25-cleanup-and-perf-sprint-design.md`,
//! PR-T4-B). Reads (`load`, `list`) and deletes call [`SessionStore::flush`]
//! first so callers see a consistent on-disk state.

use std::cmp::Reverse;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::debounced::DebouncedWriter;
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

/// On-disk session store. Cheap to clone (the writer task is shared
/// across all clones via `Arc`).
#[derive(Debug, Clone)]
pub struct SessionStore {
    inner: Arc<StoreInner>,
}

#[derive(Debug)]
struct StoreInner {
    root: PathBuf,
    writer: DebouncedWriter,
}

impl SessionStore {
    /// Construct a store with the given root directory.
    ///
    /// Spawns the background writer thread that owns the debounce
    /// window. The thread is shut down (and any pending write drained)
    /// when the last clone of the returned `SessionStore` is dropped.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self {
            inner: Arc::new(StoreInner {
                root,
                writer: DebouncedWriter::new(),
            }),
        }
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
        self.inner.root.join(format!("{name}.json"))
    }

    /// Load a session by name. Returns Ok(None) if the file doesn't exist.
    ///
    /// Flushes any pending debounced write first so callers always see
    /// the latest persisted state, even mid-debounce-window.
    ///
    /// # Errors
    /// I/O, deserialization, or name-validation errors.
    pub fn load(&self, name: &str) -> Result<Option<PersistedSession>> {
        validate_name(name)?;
        // Drain any pending write so the on-disk view is current.
        self.inner.writer.flush();
        let path = self.path_for(name);
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Save a session.
    ///
    /// The actual disk write is deferred: this call validates the
    /// session name, ensures the destination directory exists,
    /// serializes the session JSON, and hands it off to a background
    /// writer task that flushes after a 250 ms debounce window (or
    /// sooner via [`SessionStore::flush`] / drop).
    ///
    /// Returns `Ok(())` once the request is enqueued. I/O failures
    /// during the eventual write are logged at `warn` rather than
    /// surfaced to the caller — the calling turn has already
    /// completed. To force a synchronous flush (and observe its
    /// outcome only via a subsequent `load`), call
    /// [`SessionStore::flush`].
    ///
    /// # Errors
    /// Serialization, name-validation, or directory-creation errors.
    pub fn save(&self, session: &PersistedSession) -> Result<()> {
        validate_name(&session.name)?;
        std::fs::create_dir_all(&self.inner.root)?;
        let serialized = serde_json::to_vec_pretty(session)?;
        let target = self.path_for(&session.name);
        self.inner.writer.request(target, serialized);
        Ok(())
    }

    /// Block until any pending debounced write has been flushed to
    /// disk.
    ///
    /// Useful for tests and for clean-shutdown paths that want to be
    /// sure the latest session state hit the disk before continuing.
    /// Returns immediately if there is nothing pending.
    pub fn flush(&self) {
        self.inner.writer.flush();
    }

    /// List sessions (their metadata) sorted by `updated_at` descending.
    ///
    /// Flushes pending writes first so a freshly created session shows
    /// up in the listing.
    ///
    /// # Errors
    /// I/O errors. Individual broken files are SKIPPED with no error.
    pub fn list(&self) -> Result<Vec<SessionMetadata>> {
        self.inner.writer.flush();
        let mut out = Vec::new();
        let entries = match std::fs::read_dir(&self.inner.root) {
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
    /// Flushes pending writes first so an in-flight write of `name`
    /// cannot resurrect the file after the delete returns.
    ///
    /// # Errors
    /// I/O or name-validation errors.
    pub fn delete(&self, name: &str) -> Result<()> {
        validate_name(name)?;
        self.inner.writer.flush();
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
