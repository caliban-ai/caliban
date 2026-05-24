//! `OutputStyle` struct + frontmatter shape.

use std::path::PathBuf;

use serde::Deserialize;

/// Provenance of a loaded style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputStyleSource {
    /// One of the four embedded built-ins.
    BuiltIn,
    /// Operator-global style from `$XDG_CONFIG_HOME/caliban/output-styles/`.
    User {
        /// Absolute path the style was loaded from.
        path: PathBuf,
    },
    /// Project-scoped style from `<workspace>/.caliban/output-styles/`.
    Project {
        /// Absolute path the style was loaded from.
        path: PathBuf,
    },
    /// Plugin-supplied style. The picker namespaces these as
    /// `<plugin_name>:<style_name>`.
    // v2: plugin loading lands with ADR 0030. Until then this variant is
    // constructed only by the loader scanning the plugin directory (which is
    // expected to be empty in v1).
    Plugin {
        /// Name of the plugin that owns this style.
        plugin_name: String,
        /// Absolute path the style was loaded from.
        path: PathBuf,
    },
}

/// A parsed output style: frontmatter fields plus the markdown body.
#[derive(Debug, Clone)]
pub struct OutputStyle {
    /// Style identifier. For plugin styles this is `<plugin>:<name>`.
    pub name: String,
    /// One-line description surfaced in the `/output-style` picker.
    pub description: String,
    /// Markdown body (everything after the closing `---`). Empty for the
    /// default no-op style.
    pub body: String,
    /// When `false`, the caller skips the default coding-assistant block
    /// and lets the style's body provide all guidance.
    pub keep_coding_instructions: bool,
    /// Forward-looking: only honored when this style is plugin-sourced and
    /// the plugin is enabled. v1 ignores this for non-plugin styles.
    // v2: wired but inert until ADR 0030 plugin system ships.
    pub force_for_plugin: bool,
    /// Where this style came from.
    pub source: OutputStyleSource,
}

/// Raw frontmatter shape used during parsing. Not exposed.
///
/// Accepts both `snake_case` (`keep_coding_instructions`) and `kebab-case`
/// (`keep-coding-instructions`) for Claude-Code-format compatibility via
/// `#[serde(alias = "...")]`.
#[derive(Debug, Deserialize)]
pub(crate) struct Frontmatter {
    pub(crate) name: String,
    pub(crate) description: String,
    #[serde(default = "default_true", alias = "keep-coding-instructions")]
    pub(crate) keep_coding_instructions: bool,
    #[serde(default, alias = "force-for-plugin")]
    pub(crate) force_for_plugin: bool,
}

const fn default_true() -> bool {
    true
}
