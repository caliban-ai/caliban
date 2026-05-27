//! Unified, layered settings for caliban (ADR 0026).
//!
//! A single `settings.json` (or `settings.toml`) at four canonical scopes —
//! managed > user > project > local — composes into one merged
//! [`Settings`] struct that the binary hands to the existing per-feature
//! crates (`caliban-mcp-client`, `caliban-agent-core` permissions/hooks,
//! the model router) instead of letting them load their own TOMLs.
//!
//! ## Scope order (default)
//!
//! ```text
//! priority: Cli > Local > Project > User > Managed
//! ```
//!
//! When the managed scope sets `parent_settings_behavior: "block"`, the
//! managed layer moves to the *top* of the chain to enforce enterprise
//! lockdown.
//!
//! ## Merge semantics
//!
//! - Scalars: higher-priority scope wins.
//! - Permission arrays (`permissions.allow|ask|deny`) and the other
//!   array fields documented in the spec **concatenate** in priority
//!   order.
//! - Nested objects (`mcp_servers.<name>`, `env`, etc.) **deep-merge**
//!   key-by-key.
//!
//! ## Format
//!
//! JSON is the canonical format (`settings.json`); a `.toml` file at the
//! same path is parsed identically. When both exist in the same scope,
//! the JSON file wins and a `WARN` is logged.
//!
//! ## Live reload
//!
//! [`SettingsHandle::watch`] returns a watcher that emits a notification
//! whenever any of the loaded scope paths changes. The watcher is `notify`-
//! based and debounced; the caller is responsible for re-loading the
//! settings on notification (this crate exposes the helpers needed; it
//! does not own the global event loop).

#![allow(clippy::missing_errors_doc)]

mod api_key_helper;
mod compat;
mod loader;
mod merge;
mod overlay;
mod schema;
mod scope;
mod settings;
mod statusline;
mod watcher;

pub use api_key_helper::{ApiKeyHelperPool, ApiKeyHelperSpec, AuthOutcome};
pub use compat::{maybe_load_legacy_hooks, maybe_load_legacy_mcp, maybe_load_legacy_permissions};
pub use loader::{LoadError, LoadOptions, LoadOutcome, ScopeSource, load_settings};
pub use merge::{ChangedKey, RestartImpact, diff_settings, merge_values};
pub use overlay::{ConfigRow, get, render_rows, render_text};
pub use schema::{SCHEMA_JSON, validate_value};
pub use scope::{Scope, ScopePaths};
pub use settings::{ApiKeyHelperRaw, McpServerSetting, ModelSelector, Permissions, Settings};
pub use statusline::{StatuslineConfig, StatuslineContext, StatuslineRunner};
pub use watcher::{SettingsWatcher, WatcherEvent, is_settings_path, watch_paths_from_sources};

use std::sync::Arc;

use arc_swap::ArcSwap;

/// Atomic handle to the currently-effective merged [`Settings`].
///
/// Wraps an `Arc<ArcSwap<Settings>>` so the binary can hand the same
/// handle to every subsystem and the live-reload path can swap the
/// pointer atomically without coordination.
#[derive(Clone)]
pub struct SettingsHandle {
    inner: Arc<ArcSwap<Settings>>,
}

impl SettingsHandle {
    /// Construct a handle from an already-loaded `Settings`.
    #[must_use]
    pub fn new(settings: Settings) -> Self {
        Self {
            inner: Arc::new(ArcSwap::from_pointee(settings)),
        }
    }

    /// Load the current snapshot.
    #[must_use]
    pub fn current(&self) -> Arc<Settings> {
        self.inner.load_full()
    }

    /// Atomically swap in a new snapshot. Returns the previous one.
    pub fn store(&self, settings: Settings) -> Arc<Settings> {
        let prev = self.inner.load_full();
        self.inner.store(Arc::new(settings));
        prev
    }
}

impl std::fmt::Debug for SettingsHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsHandle").finish_non_exhaustive()
    }
}
