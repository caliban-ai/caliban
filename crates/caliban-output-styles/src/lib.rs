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
/// It is the highest-precedence surface for choosing a style; the
/// `output_style` settings key is the persistent fallback (see [`requested`]).
/// When neither is set, the built-in `default` style is used.
pub const ACTIVE_STYLE_ENV: &str = "CALIBAN_OUTPUT_STYLE";

/// Resolve the requested output-style name.
///
/// Precedence: the [`ACTIVE_STYLE_ENV`] environment variable wins, then the
/// `output_style` settings value (`setting`), then the built-in `default`
/// (#620 — the setting was previously parsed but never consulted). Blank
/// (empty / whitespace-only) values at either level are ignored.
#[must_use]
pub fn requested(setting: Option<&str>) -> String {
    if let Ok(v) = std::env::var(ACTIVE_STYLE_ENV)
        && !v.trim().is_empty()
    {
        return v;
    }
    if let Some(s) = setting {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    "default".to_string()
}

/// Read the requested output-style name from the environment only.
///
/// Equivalent to [`requested(None)`](requested); returns `"default"` when
/// [`ACTIVE_STYLE_ENV`] is unset or empty.
#[must_use]
pub fn requested_from_env() -> String {
    requested(None)
}
