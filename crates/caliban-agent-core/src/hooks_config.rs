//! `hooks.toml` parser + in-memory config model (ADR 0024).
//!
//! The types and loader now live in the `caliban-config-types` leaf crate
//! (epic #539 / ADR 0061) so `caliban-settings` can consume them without
//! inverting the layering. Re-exported here so `caliban_agent_core::hooks_config::*`
//! (and the crate-root re-exports in `lib.rs`) keep resolving unchanged.

pub use caliban_config_types::hooks_config::{
    HookHandlerConfig, HookHandlerType, HooksConfig, HooksConfigError,
};
