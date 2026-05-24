//! MCP client — Phase A (stdio wiring).
//!
//! See `docs/superpowers/specs/2026-05-24-mcp-v2-design.md` and
//! `adrs/0023-mcp-v2-transports-and-oauth.md`. v1's config/manager scaffold
//! (`adrs/0017`) is now backed by real spawn / handshake / `list_tools` via
//! `rmcp 1.7`'s child-process transport.
//!
//! HTTP/SSE transports (Phase B) and OAuth/elicitation/resources (Phase C)
//! land in follow-up PRs; the `Transport` enum already includes those
//! variants but `Conn::start` returns `McpError::TransportNotYetImplemented`
//! for them.

#![allow(clippy::multiple_crate_versions)]

pub mod client;
pub mod config;
pub mod error;
pub mod manager;
pub mod registry;
pub mod tool;

pub use client::{Conn, Transport};
pub use config::{McpConfig, ServerConfig, discovery_paths, is_valid_server_name, load_config};
pub use error::{McpError, Result};
pub use manager::{DEFAULT_STARTUP_TIMEOUT, DEFAULT_TOOL_TIMEOUT, McpClientManager, StartOptions};
pub use registry::{ServerStatus, ServerSummary};
pub use tool::{McpTool, full_tool_name, normalize_tool_name};
