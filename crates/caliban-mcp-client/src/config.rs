//! `mcp.toml` config — re-export shim.
//!
//! The config data types, the `mcp.toml` discovery/merge loader, and its
//! `ConfigError` now live in the `caliban-config-types` leaf crate (epic #539 /
//! ADR 0061) so `caliban-settings` can read `mcp.toml` without depending on this
//! runtime crate. Re-exported here so `caliban_mcp_client::config::*` (and the
//! crate-root re-exports in `lib.rs`) keep resolving unchanged.

#[allow(deprecated)]
pub use caliban_config_types::mcp::load_config;
pub use caliban_config_types::mcp::{
    ConfigError, ManualOauthConfig, McpConfig, OauthMode, ServerConfig, ServerPermissions,
    TransportKind, discovery_paths, is_valid_server_name,
};
