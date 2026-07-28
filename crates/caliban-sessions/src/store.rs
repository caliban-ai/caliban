//! `SessionStore` — disk-backed CRUD over `PersistedSession`.
//!
//! Writes go through a [`DebouncedWriter`](crate::debounced) so a flurry
//! of intra-turn snapshots collapses into a single atomic file write
//! (see `docs/superpowers/specs/2026-05-25-cleanup-and-perf-sprint-design.md`,
//! PR-T4-B). Reads (`load`, `list`) and deletes call [`SessionStore::flush`]
//! first so callers see a consistent on-disk state.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::backend::{FsSessionBackend, SessionBackend};
use crate::debounced::DebouncedWriter;
use crate::error::{Error, Result};
use crate::session::PersistedSession;

/// Session store over a [`SessionBackend`]. Cheap to clone (the writer task is
/// shared across all clones via `Arc`).
#[derive(Debug, Clone)]
pub struct SessionStore {
    inner: Arc<StoreInner>,
}

#[derive(Debug)]
struct StoreInner {
    writer: DebouncedWriter,
}

impl SessionStore {
    /// Construct a store backed by the filesystem at `root` (the default).
    ///
    /// Spawns the background writer thread that owns the debounce
    /// window. The thread is shut down (and any pending write drained)
    /// when the last clone of the returned `SessionStore` is dropped.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self::with_backend(Arc::new(FsSessionBackend::new(root)))
    }

    /// Construct a store over an arbitrary [`SessionBackend`] (e.g. a gonzalo
    /// substrate). Spawns the shared debounced writer over that backend.
    #[must_use]
    pub fn with_backend(backend: Arc<dyn SessionBackend>) -> Self {
        Self {
            inner: Arc::new(StoreInner {
                writer: DebouncedWriter::new(backend),
            }),
        }
    }

    /// Resolve the default root: `$XDG_DATA_HOME/caliban/sessions`
    /// or `$HOME/.local/share/caliban/sessions`.
    ///
    /// # Errors
    /// Returns `Error::NoHome` if neither `XDG_DATA_HOME` nor HOME are available.
    pub fn default_root() -> Result<PathBuf> {
        let base = caliban_common::paths::platform_data_dir().ok_or(Error::NoHome)?;
        Ok(base.join("caliban").join("sessions"))
    }

    /// Load a session by name. Returns Ok(None) if it doesn't exist.
    ///
    /// The read is round-tripped through the background writer, which drains
    /// any pending debounced write first so callers always see the latest
    /// persisted state, even mid-debounce-window.
    ///
    /// # Errors
    /// I/O, deserialization, or name-validation errors surfaced by the backend.
    pub fn load(&self, name: &str) -> Result<Option<PersistedSession>> {
        crate::backend::fs::validate_name(name)?;
        self.inner.writer.load(name).map_err(Error::Persist)
    }

    /// Save a session.
    ///
    /// The actual persist is deferred: this call hands a clone of `session`
    /// off to a background writer task that saves it through the backend after
    /// a 250 ms debounce window (or sooner via [`SessionStore::flush`] / drop).
    /// Name validation, serialization, and directory creation are the backend's
    /// responsibility and happen at drain time.
    ///
    /// Returns `Ok(())` once the request is enqueued. A failure of the
    /// eventual deferred write is warn-logged *and* recorded, so it is
    /// observable via [`SessionStore::flush`] (which returns the outcome of
    /// the write it forces) or [`SessionStore::last_write_error`] (a health
    /// signal that also catches timer-flushed failures) — no longer a silent
    /// `Ok` (#414).
    ///
    /// # Errors
    /// Returns `Error::InvalidName` synchronously if `session.name` fails
    /// validation — matching the pre-debounce behavior where a bad name was
    /// rejected before any write was attempted. Once past that check, this
    /// never errors on enqueue; deferred-write failures surface via `flush` /
    /// `last_write_error`. This is a narrower synchronous contract than
    /// pre-#471, where directory-creation and serialization failures were
    /// also returned from `save` directly: those now happen at drain time
    /// alongside the write itself, so they only surface via `flush()` /
    /// `last_write_error()`, never as an `Err` from this call.
    pub fn save(&self, session: &PersistedSession) -> Result<()> {
        crate::backend::fs::validate_name(&session.name)?;
        self.inner.writer.request(session.clone());
        Ok(())
    }

    /// Block until any pending debounced write has been flushed to
    /// disk, returning the drain outcome.
    ///
    /// Useful for tests and for clean-shutdown paths that want to be
    /// sure the latest session state hit the disk before continuing.
    /// Returns `Ok(())` immediately if there is nothing pending.
    ///
    /// # Errors
    /// [`Error::Persist`] if the forced write failed — so a persist failure is
    /// observable instead of only warn-logged (#414).
    pub fn flush(&self) -> Result<()> {
        self.inner.writer.flush().map_err(Error::Persist)
    }

    /// The most recent deferred-write failure (a health signal), if the last
    /// write to that session has not since succeeded; `None` when all writes
    /// are healthy.
    ///
    /// Complements [`SessionStore::flush`]: it catches failures that were
    /// flushed by the debounce timer rather than an explicit `flush`, so
    /// [`SessionStore::save`]'s fire-and-forget failures remain observable
    /// (#414).
    #[must_use]
    pub fn last_write_error(&self) -> Option<String> {
        self.inner.writer.last_error()
    }

    /// List sessions (their metadata) sorted by `updated_at` descending.
    ///
    /// Round-tripped through the writer, which drains pending writes first so
    /// a freshly created session shows up in the listing.
    ///
    /// # Errors
    /// I/O errors surfaced by the backend. Individual broken files are SKIPPED.
    pub fn list(&self) -> Result<Vec<SessionMetadata>> {
        self.inner.writer.list().map_err(Error::Persist)
    }

    /// Delete a session.
    ///
    /// Round-tripped through the writer, which drains pending writes first so
    /// an in-flight write of `name` cannot resurrect the file after the delete
    /// returns.
    ///
    /// # Errors
    /// I/O or name-validation errors surfaced by the backend.
    pub fn delete(&self, name: &str) -> Result<()> {
        crate::backend::fs::validate_name(name)?;
        self.inner.writer.delete(name).map_err(Error::Persist)
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

impl SessionMetadata {
    /// Derive list metadata from a full session (shared by all backends).
    #[must_use]
    pub fn from_session(session: &PersistedSession) -> Self {
        Self {
            name: session.name.clone(),
            updated_at: session.updated_at,
            turn_count: session.turn_count(),
            total_tokens: session
                .total_usage
                .input_tokens
                .saturating_add(session.total_usage.output_tokens),
        }
    }
}
