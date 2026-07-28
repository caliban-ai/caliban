//! Session persistence for the caliban agent harness.
//!
//! Stores conversation history as JSON files under
//! `$XDG_DATA_HOME/caliban/sessions/` (default
//! `$HOME/.local/share/caliban/sessions/`).

#![allow(clippy::multiple_crate_versions)]

mod debounced;
pub mod error;
pub mod session;
pub mod store;

pub mod backend;

#[cfg(feature = "gonzalo")]
pub use backend::GonzaloSessionBackend;
pub use backend::{FsSessionBackend, SessionBackend};
pub use error::{Error, Result};
pub use session::PersistedSession;
pub use store::{SessionMetadata, SessionStore};
