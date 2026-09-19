//! `caliban-contract` — the thin, dependency-light contract between caliban and
//! the components that launch and observe it.
//!
//! caliban's launch and control-plane contract used to be implicit and
//! stringly-typed, and was hand-copied into other repos: caliban-operator
//! hand-typed caliband's flag/env names, and prospero hand-mirrored the
//! supervisor wire types — because the only published crate, `caliban-supervisor`,
//! is the whole daemon (tokio-full, TLS, git, …), too heavy to depend on. Every
//! drift surfaced silently at runtime (#656).
//!
//! This crate holds that contract as a **single definition** with minimal
//! dependencies (`serde`, `serde_json`, `thiserror` — no async, no other caliban
//! crate), so an out-of-tree driver can depend on it directly:
//!
//! - [`wire`] — the supervisor control-plane wire types ([`wire::CtlRequest`],
//!   [`wire::CtlReply`], [`wire::AgentRecord`], [`wire::Endpoint`], …).
//!   `caliban-supervisor` re-exports these, so they have one home.
//! - [`launch`] — the [`launch::CalibandLaunch`] builder that turns a typed launch
//!   spec into caliband's argv / env, plus the flag and env-var **name
//!   constants** both the builder and caliband's own parser reference, so the two
//!   cannot drift.

pub mod launch;
pub mod wire;
