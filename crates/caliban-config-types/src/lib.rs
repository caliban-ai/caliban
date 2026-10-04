//! Leaf crate of shared configuration **data types** for caliban.
//!
//! `caliban-settings` hosts the configuration substrate, but the shapes it
//! stores and projects (MCP servers, permission rules, hooks) are *owned* by
//! the crates that consume them. Historically those types lived in the
//! consumer crates and `caliban-settings` depended **upward** on them to name
//! them — inverting the layering (epic #539). This crate breaks that cycle: it
//! holds the genuinely-shared data types and depends on no consumer crate, so
//! both `caliban-settings` and its consumers can depend on it *downward*.
//!
//! See ADR 0061.
//!
//! This crate carries the MCP config types (#708) and the permission, permission-mode,
//! and hook config types (#709).

pub mod hooks_config;
pub mod mcp;
pub mod permission_mode;
pub mod permissions;

pub use hooks_config::{HookHandlerConfig, HookHandlerType, HooksConfig, HooksConfigError};
#[allow(deprecated)]
pub use mcp::load_config;
pub use mcp::{
    ConfigError, ManualOauthConfig, McpConfig, OauthMode, ServerConfig, ServerPermissions,
    TransportKind, discovery_paths, is_valid_server_name,
};
pub use permission_mode::PermissionMode;
pub use permissions::{Action, PermissionsLoadError, Rule, default_rules};
#[allow(deprecated)]
pub use permissions::{load_rules, load_rules_file};
