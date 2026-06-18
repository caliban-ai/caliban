//! Cross-crate plumbing for the caliban agent harness.
//!
//! Holds the small, opinionated helpers that had drifted into per-crate
//! implementations: env-var expansion, atomic file writes, XDG paths,
//! workspace-path sanitization, ancestor-walk file discovery, narrow glob
//! matching, markdown frontmatter splitting, and the canonical set of
//! `tracing` target strings.
//!
//! Nothing fancy lives here — no traits with multiple implementations, no
//! generic frameworks. Just pure functions and one or two constructors.

pub mod expand;
pub mod frontmatter;
pub mod fs;
pub mod glob_match;
pub mod http;
pub mod paths;
pub mod tracing_targets;
