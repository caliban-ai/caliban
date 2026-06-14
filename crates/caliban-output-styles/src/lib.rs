//! Output styles for the caliban agent harness.
//!
//! Splices an `<output-style name="...">...</output-style>` block into the
//! system prompt to nudge the model toward a particular response shape
//! (explanatory commentary, learning-paced prompts with `TODO(human)`
//! markers, etc.) without touching tools, permissions, or hooks.
//!
//! See `docs/superpowers/specs/2026-05-24-output-styles-design.md` and
//! `docs/adr/0031-output-styles.md`.

#![allow(clippy::multiple_crate_versions)]

pub mod learning;
pub mod loader;
pub mod prefix;
pub mod registry;
pub mod style;

pub use learning::{IdentityPostProcessor, LearningPostProcessor, insert_todo_human_markers};
pub use loader::{
    DiscoveryRoots, OutputStyleError, default_roots, load_one, load_styles, select_active,
};
pub use prefix::OutputStylePrefix;
pub use registry::OutputStylesRegistry;
pub use style::{OutputStyle, OutputStyleSource};

/// Environment variable that selects the active output style by name.
///
/// Until ADR 0026 lands the settings hierarchy, this env var is the
/// operator's surface for choosing a style. When unset (or empty), the
/// built-in `default` style is used.
pub const ACTIVE_STYLE_ENV: &str = "CALIBAN_OUTPUT_STYLE";

/// Read the requested output-style name from the environment.
///
/// Returns `"default"` when [`ACTIVE_STYLE_ENV`] is unset or empty.
#[must_use]
pub fn requested_from_env() -> String {
    std::env::var(ACTIVE_STYLE_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "default".to_string())
}
