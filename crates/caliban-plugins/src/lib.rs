//! Plugin packaging orchestrator (ADR 0030).
//!
//! See `docs/superpowers/specs/2026-05-24-plugin-system-design.md` for the
//! authoritative design. The crate is intentionally a *thin* orchestrator:
//! it parses `plugin.json`, applies discovery/filter rules, namespaces
//! items, and hands roots + configs to the existing skill / hook / agent /
//! MCP / output-style loaders. It does not duplicate per-surface logic.

#![allow(clippy::multiple_crate_versions)]

pub mod aggregate;
pub mod cli;
pub mod discovery;
pub mod error;
pub mod expand;
pub mod filter;
pub mod loaded;
pub mod manager;
pub mod manifest;
pub mod marketplace;
pub mod overlay;
pub mod trust;

pub use cli::{Cli, ListedPlugin};
pub use discovery::{DirectorySource, PluginSourceProvider};
pub use error::PluginError;
pub use expand::{expand as expand_plugin_root, expand_json_in_place};
pub use loaded::{Discovered, LoadedPlugin, NamespacedItem, PluginSource};
pub use manager::{
    PluginLoadFailure, PluginManager, PluginRoots, PluginSettings, default_managed_dir,
};
pub use manifest::{
    ComponentSpec, InlineMcpServer, PathList, PluginManifest, ResolvedComponents, is_valid_name,
};
pub use marketplace::{
    Marketplace, MarketplaceClient, MarketplaceEntry, MarketplaceSettings, MarketplaceVersion,
    TrustDecision,
};
pub use overlay::render_overlay;
pub use trust::{MarketplacesAllowlist, PluginTrustRecord, TrustFile, TrustStore};
