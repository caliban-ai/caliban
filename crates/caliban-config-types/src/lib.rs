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
//! This first slice (#708) carries the MCP config types; permission and hook
//! types follow in #709.

pub mod mcp;

pub use mcp::{
    ManualOauthConfig, McpConfig, OauthMode, ServerConfig, ServerPermissions, TransportKind,
};
