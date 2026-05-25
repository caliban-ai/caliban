//! MCP client — Phase B (HTTP + SSE transports).
//!
//! See `docs/superpowers/specs/2026-05-24-mcp-v2-design.md` and
//! `adrs/0023-mcp-v2-transports-and-oauth.md`. Phase A's stdio wiring carries
//! over unchanged; Phase B adds HTTP/SSE via rmcp 1.7's streamable-http client
//! and per-server permission scoping. OAuth (auto/manual), elicitation, and
//! resources land in Phase C.

#![allow(clippy::multiple_crate_versions)]

pub mod client;
pub mod config;
pub mod error;
pub mod manager;
pub mod permissions;
pub mod registry;
pub mod tool;

pub use client::{Conn, Transport};
pub use config::{
    McpConfig, OauthMode, ServerConfig, ServerPermissions, TransportKind, discovery_paths,
    is_valid_server_name, load_config,
};
pub use error::{McpError, Result};
pub use manager::{DEFAULT_STARTUP_TIMEOUT, DEFAULT_TOOL_TIMEOUT, McpClientManager, StartOptions};
pub use permissions::{compile_server_permission_rules, merge_with_global};
pub use registry::{ServerStatus, ServerSummary};
pub use tool::{McpTool, full_tool_name, normalize_tool_name};
