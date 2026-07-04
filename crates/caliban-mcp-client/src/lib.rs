//! MCP client — Phase C (OAuth + elicitation + resources).
//!
//! See `docs/superpowers/specs/2026-05-24-mcp-v2-design.md` and
//! `docs/adr/0023-mcp-v2-transports-and-oauth.md`. Phase A's stdio wiring and
//! Phase B's HTTP/SSE transports carry over unchanged; Phase C adds:
//!
//! * [`oauth`] — PKCE + loopback callback, RFC 8414 discovery, manual config,
//!   keyring-backed token cache with file-store fallback, inline refresh
//!   when the access token nears expiry, 401-on-use cache invalidation.
//! * [`elicitation`] — `ElicitationBridge` plumbing server-side prompts
//!   through to the TUI modal with a 5-minute hard cap and non-interactive
//!   auto-decline.
//! * [`resource`] — `@<server>:<resource>` mentions, lazy `resources/list`
//!   cache, `resources/list_changed` invalidation, `resources/read` inlining,
//!   URI-template positional expansion.

#![allow(clippy::multiple_crate_versions)]

pub mod client;
pub mod config;
pub mod elicitation;
pub mod error;
pub mod manager;
pub mod oauth;
pub mod permissions;
pub mod registry;
pub mod resource;
pub mod tool;

pub use client::{Conn, Transport};
pub use config::{
    McpConfig, OauthMode, ServerConfig, ServerPermissions, TransportKind, discovery_paths,
    is_valid_server_name,
};
// `load_config` is `#[deprecated]` in favor of `caliban-settings` (PR-T3-B).
// Re-exported with `#[allow(deprecated)]` so the lint surfaces at the call
// site, not at the crate boundary.
/// Re-export — actual definition in `caliban-agent-core::mcp_activation`
/// so `caliban-tools-builtin` can consume it without depending on this crate.
pub use caliban_agent_core::mcp_activation::McpToolInfo;
#[allow(deprecated)]
pub use config::load_config;
pub use elicitation::{
    DEFAULT_ELICITATION_TIMEOUT, ElicitationBridge, ElicitationError, ElicitationReceiver,
    ElicitationRequest, ElicitationResponse, SharedElicitationBridge, elicit_rule_pattern,
};
pub use error::{McpError, Result};
pub use manager::{DEFAULT_STARTUP_TIMEOUT, DEFAULT_TOOL_TIMEOUT, McpClientManager, StartOptions};
pub use oauth::{
    FileStore, KEYRING_SERVICE, KeyringStore, ManualOauthConfig, MemoryStore, OauthAuthenticator,
    OauthEndpoints, OauthFlow, OauthFlowOptions, OauthTokens, PORT_ENV_VAR, REFRESH_MARGIN,
    RegisteredClient, TokenStore, default_store, discover_endpoints, endpoints_from_manual,
    refresh_tokens, register_client,
};
pub use permissions::{compile_server_permission_rules, merge_with_global};
pub use registry::{ServerStatus, ServerSummary};
pub use resource::{McpResource, ResourceEntry, ResourceMention, expand_template};
pub use tool::{McpTool, full_tool_name, normalize_tool_name};
