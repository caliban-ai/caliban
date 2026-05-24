//! File-backed memory tiers spliced into the caliban system prompt.
//!
//! See `docs/superpowers/specs/2026-05-23-memory-tier-1-design.md` and
//! `adrs/0018-memory-tier-model.md`.

#![allow(clippy::multiple_crate_versions)]

pub mod config;
pub mod error;
pub mod loader;
pub mod prefix;
pub mod sanitize;
pub mod walk;

pub use config::MemoryConfig;
pub use error::{MemoryError, Result};
pub use loader::{estimate_tokens, load};
pub use prefix::{MemoryPrefix, TierFile, TierKind};
pub use sanitize::sanitize_workspace;
pub use walk::walk_up_for_file;
