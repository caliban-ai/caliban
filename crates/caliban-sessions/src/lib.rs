//! Session persistence for the caliban agent harness.
//!
//! Stores conversation history as JSON files under
//! `$XDG_DATA_HOME/caliban/sessions/` (default
//! `$HOME/.local/share/caliban/sessions/`).

#![allow(clippy::multiple_crate_versions)]

pub mod error;
pub mod session;
pub mod store;

pub use error::{Error, Result};
pub use session::PersistedSession;
pub use store::{SessionMetadata, SessionStore};
