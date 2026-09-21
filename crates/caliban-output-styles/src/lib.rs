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
/// The env var's canonical name, for user-facing messages. It is **not read
/// here**: the settings env layer folds `CALIBAN_OUTPUT_STYLE` into the
/// `output_style` settings key at load time (env > file), so this crate resolves
/// purely from the (already env-folded) settings value (#701; env registry
/// #538). It remains the highest-precedence surface for choosing a style.
pub const ACTIVE_STYLE_ENV: &str = "CALIBAN_OUTPUT_STYLE";

/// Resolve the requested output-style name.
///
/// Precedence: the `output_style` settings value (`setting`), then the built-in
/// `default`. Blank (empty / whitespace-only) values are ignored. The
/// `CALIBAN_OUTPUT_STYLE` env var is honored upstream — it is folded into
/// `setting` by the settings env layer (#701) — so it is not read here; that
/// keeps env-vs-settings precedence in one place and lets `caliban config print`
/// attribute the value.
#[must_use]
pub fn requested(setting: Option<&str>) -> String {
    if let Some(s) = setting {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    "default".to_string()
}
