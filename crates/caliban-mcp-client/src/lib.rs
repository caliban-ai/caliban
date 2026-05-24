//! MCP client (stdio v1, scaffold).
//!
//! See `docs/superpowers/specs/2026-05-23-mcp-client-design.md` and
//! `adrs/0017-mcp-client-architecture.md`.
//!
//! **Status:** v1 ships the config schema + manager scaffold; spawn + tool
//! registration via `rmcp` is the next sub-PR.

#![allow(clippy::multiple_crate_versions)]

pub mod config;
pub mod error;
pub mod manager;

pub use config::{McpConfig, ServerConfig, discovery_paths, is_valid_server_name, load_config};
pub use error::{McpError, Result};
pub use manager::McpClientManager;
