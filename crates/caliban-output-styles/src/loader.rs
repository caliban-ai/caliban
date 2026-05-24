//! Filesystem walker + frontmatter parser for output-style `.md` files.
//!
//! Discovery roots (priority order, first-wins):
//!
//! 1. `<workspace_root>/.caliban/output-styles/<name>.md` (project)
//! 2. `$XDG_CONFIG_HOME/caliban/output-styles/<name>.md` (user)
//! 3. `$XDG_DATA_HOME/caliban/plugins/<plugin>/output-styles/<name>.md`
//!    (plugin — namespaced as `<plugin>:<name>`)
//! 4. The four embedded built-ins (always present).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::style::{Frontmatter, OutputStyle, OutputStyleSource};

/// Errors surfaced by [`load_one`].
#[derive(Debug, Error)]
pub enum OutputStyleError {
    /// The file could not be read from disk.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// The frontmatter delimiters were missing or malformed.
    #[error("frontmatter: {0}")]
    Frontmatter(String),

    /// The YAML inside the frontmatter could not be parsed.
    #[error("yaml: {0}")]
    Yaml(#[from] serde_yaml::Error),

    /// The `name:` field does not match the file stem.
    #[error("style name '{name}' does not match file stem '{stem}'")]
    NameStemMismatch {
        /// The frontmatter-declared name.
        name: String,
        /// The on-disk filename stem.
        stem: String,
    },

    /// The `description:` field was empty after trimming.
    #[error("description must be non-empty")]
    EmptyDescription,

    /// The name contains characters outside `[a-z0-9_-]+`.
    #[error("invalid name '{0}': must match [a-z0-9_-]+ and be lowercase")]
    InvalidName(String),
}

/// Built-in style files embedded at compile time.
const BUILTIN_DEFAULT: &str = include_str!("builtins/default.md");
const BUILTIN_PROACTIVE: &str = include_str!("builtins/proactive.md");
const BUILTIN_EXPLANATORY: &str = include_str!("builtins/explanatory.md");
const BUILTIN_LEARNING: &str = include_str!("builtins/learning.md");

const BUILTINS: &[(&str, &str)] = &[
    ("default", BUILTIN_DEFAULT),
    ("proactive", BUILTIN_PROACTIVE),
    ("explanatory", BUILTIN_EXPLANATORY),
    ("learning", BUILTIN_LEARNING),
];

/// Per-source discovery roots used by [`load_styles`].
#[must_use]
pub fn default_roots(workspace_root: &Path) -> DiscoveryRoots {
    let project = workspace_root.join(".caliban").join("output-styles");
    let user = dirs::config_dir().map(|d| d.join("caliban").join("output-styles"));
    let plugins_root = dirs::data_local_dir().map(|d| d.join("caliban").join("plugins"));
    DiscoveryRoots {
        project,
        user,
        plugins_root,
    }
}

/// The set of filesystem roots scanned for output-style files.
#[derive(Debug, Clone)]
pub struct DiscoveryRoots {
    /// `<workspace>/.caliban/output-styles/`.
    pub project: PathBuf,
    /// `$XDG_CONFIG_HOME/caliban/output-styles/`.
    pub user: Option<PathBuf>,
    /// `$XDG_DATA_HOME/caliban/plugins/` — each subdirectory is a plugin,
    /// and styles are loaded from `<plugin>/output-styles/`.
    // v2: plugin styles are loaded but inert (`force_for_plugin` is ignored)
    // until the plugin system from ADR 0030 lands.
    pub plugins_root: Option<PathBuf>,
}

/// Load all available output styles in priority order.
///
/// Priority: project > user > plugin > built-in. The first occurrence of a
/// given `name` wins; subsequent occurrences are shadowed and logged at
/// `tracing::debug!`.
///
/// Plugin-supplied styles are namespaced `<plugin_name>:<style_name>` in
/// the returned list, so they cannot collide with bare names by accident.
#[must_use]
pub fn load_styles(roots: &DiscoveryRoots) -> Vec<OutputStyle> {
    let mut by_name: HashMap<String, OutputStyle> = HashMap::new();

    // 1. project
    scan_flat_dir(&roots.project, &OutputStyleKind::Project, &mut by_name);

    // 2. user
    if let Some(user) = roots.user.as_ref() {
        scan_flat_dir(user, &OutputStyleKind::User, &mut by_name);
    }

    // 3. plugin (scan each `<plugins_root>/<plugin>/output-styles/`).
    if let Some(plugins_root) = roots.plugins_root.as_ref()
        && plugins_root.exists()
        && let Ok(rd) = std::fs::read_dir(plugins_root)
    {
        for entry in rd.flatten() {
            let plugin_dir = entry.path();
            if !plugin_dir.is_dir() {
                continue;
            }
            let Some(plugin_name) = plugin_dir
                .file_name()
                .and_then(|s| s.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            let styles_dir = plugin_dir.join("output-styles");
            scan_flat_dir(
                &styles_dir,
                &OutputStyleKind::Plugin {
                    plugin_name: plugin_name.clone(),
                },
                &mut by_name,
            );
        }
    }

    // 4. built-ins (always present, lowest priority)
    for (name, raw) in BUILTINS {
        if by_name.contains_key(*name) {
            tracing::debug!(
                target: "caliban::output_styles",
                name = name,
                "skipping shadowed built-in (overridden by higher-priority source)",
            );
            continue;
        }
        match parse_raw(raw, name, OutputStyleSource::BuiltIn) {
            Ok(style) => {
                by_name.insert(style.name.clone(), style);
            }
            Err(e) => {
                tracing::error!(
                    target: "caliban::output_styles",
                    name = name,
                    error = %e,
                    "embedded built-in failed to parse — this is a bug",
                );
            }
        }
    }

    let mut out: Vec<OutputStyle> = by_name.into_values().collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Internal: which kind of source we're scanning. Carries the plugin name
/// when applicable so we can namespace style names.
enum OutputStyleKind {
    Project,
    User,
    Plugin { plugin_name: String },
}

fn scan_flat_dir(dir: &Path, kind: &OutputStyleKind, by_name: &mut HashMap<String, OutputStyle>) {
    if !dir.exists() {
        return;
    }
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => {
            tracing::warn!(
                target: "caliban::output_styles",
                dir = %dir.display(),
                error = %e,
                "could not read output-styles directory",
            );
            return;
        }
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let source = match kind {
            OutputStyleKind::Project => OutputStyleSource::Project { path: path.clone() },
            OutputStyleKind::User => OutputStyleSource::User { path: path.clone() },
            OutputStyleKind::Plugin { plugin_name } => OutputStyleSource::Plugin {
                plugin_name: plugin_name.clone(),
                path: path.clone(),
            },
        };
        match load_one(&path, source) {
            Ok(mut style) => {
                // Namespace plugin styles as "<plugin>:<name>" so they don't
                // collide with bare names from project/user/built-in.
                if let OutputStyleSource::Plugin { plugin_name, .. } = &style.source {
                    style.name = format!("{plugin_name}:{}", style.name);
                }
                if by_name.contains_key(&style.name) {
                    tracing::debug!(
                        target: "caliban::output_styles",
                        name = %style.name,
                        path = %path.display(),
                        "skipping shadowed style (already loaded from higher-priority root)",
                    );
                } else {
                    by_name.insert(style.name.clone(), style);
                }
            }
            Err(e) => {
                tracing::warn!(
                    target: "caliban::output_styles",
                    path = %path.display(),
                    error = %e,
                    "skipping malformed output style",
                );
            }
        }
    }
}

/// Parse a single output-style `.md` file from disk, attributing it to the
/// given `source`.
///
/// # Errors
///
/// Returns [`OutputStyleError`] when the file cannot be read, the
/// frontmatter is missing/malformed, the YAML is invalid, the name field
/// doesn't match the file stem, or the description is empty.
pub fn load_one(path: &Path, source: OutputStyleSource) -> Result<OutputStyle, OutputStyleError> {
    let raw = std::fs::read_to_string(path)?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    parse_raw(&raw, stem, source)
}

/// Parse raw markdown-with-frontmatter into an [`OutputStyle`].
///
/// `expected_stem` is the file stem (or built-in key) used for the
/// name-mismatch check.
fn parse_raw(
    raw: &str,
    expected_stem: &str,
    source: OutputStyleSource,
) -> Result<OutputStyle, OutputStyleError> {
    let raw_trim = raw.trim_start_matches('\u{feff}');
    let delim = "---\n";
    if !raw_trim.starts_with(delim) {
        return Err(OutputStyleError::Frontmatter(
            "missing leading `---` frontmatter delimiter".into(),
        ));
    }
    let after_start = &raw_trim[delim.len()..];
    // Look for the closing `\n---` (with or without trailing newline / content).
    let Some(end_idx) = find_closing(after_start) else {
        return Err(OutputStyleError::Frontmatter(
            "missing closing `---` frontmatter delimiter".into(),
        ));
    };
    let yaml_chunk = &after_start[..end_idx];
    // Body is whatever follows "\n---" (skipping the newline after, if any).
    let after_close = &after_start[end_idx..];
    // `after_close` starts with "\n---". Skip those four bytes and one
    // optional trailing newline.
    let mut body_start = "\n---".len();
    if after_close.as_bytes().get(body_start).copied() == Some(b'\n') {
        body_start += 1;
    }
    let body = if body_start >= after_close.len() {
        String::new()
    } else {
        after_close[body_start..].to_string()
    };

    let fm: Frontmatter = serde_yaml::from_str(yaml_chunk)?;

    if fm.description.trim().is_empty() {
        return Err(OutputStyleError::EmptyDescription);
    }
    if !is_valid_name(&fm.name) {
        return Err(OutputStyleError::InvalidName(fm.name));
    }
    if fm.name != expected_stem {
        return Err(OutputStyleError::NameStemMismatch {
            name: fm.name,
            stem: expected_stem.to_string(),
        });
    }

    Ok(OutputStyle {
        name: fm.name,
        description: fm.description,
        body,
        keep_coding_instructions: fm.keep_coding_instructions,
        force_for_plugin: fm.force_for_plugin,
        source,
    })
}

/// Returns the byte offset of the closing `\n---` marker in `s`, if any.
///
/// Tolerates both `\n---\n` (closing followed by body) and `\n---` at EOF.
fn find_closing(s: &str) -> Option<usize> {
    // Prefer the strict `\n---\n` form; fall back to a trailing `\n---` at EOF.
    if let Some(i) = s.find("\n---\n") {
        return Some(i);
    }
    if let Some(i) = s.rfind("\n---")
        && s[i + "\n---".len()..].chars().all(char::is_whitespace)
    {
        return Some(i);
    }
    None
}

fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Select the active style from the loaded list given a requested name and
/// the set of enabled plugins.
///
/// Selection precedence:
///
/// 1. If any plugin-sourced style has `force_for_plugin = true` *and* its
///    plugin appears in `enabled_plugins`, that style wins regardless of
///    `requested`.
/// 2. Otherwise, the style whose `name` exactly matches `requested` is
///    returned.
/// 3. Failing that, the built-in `default` style is returned and a warning
///    is logged.
#[must_use]
pub fn select_active(
    all: &[OutputStyle],
    requested: &str,
    enabled_plugins: &[String],
) -> Option<OutputStyle> {
    // v2: plugin force-override path. Today no plugins ship with caliban,
    // so this branch is exercised only by tests until ADR 0030 lands.
    for s in all {
        if !s.force_for_plugin {
            continue;
        }
        if let OutputStyleSource::Plugin { plugin_name, .. } = &s.source
            && enabled_plugins.iter().any(|n| n == plugin_name)
        {
            tracing::debug!(
                target: "caliban::output_styles",
                style = %s.name,
                plugin = %plugin_name,
                "plugin-forced output style active (overrides operator selection)",
            );
            return Some(s.clone());
        } else if !matches!(&s.source, OutputStyleSource::Plugin { .. }) {
            // Sideload (user/project/built-in) with force_for_plugin = true is ignored.
            tracing::debug!(
                target: "caliban::output_styles",
                style = %s.name,
                "ignoring force_for_plugin on non-plugin style",
            );
        }
    }

    if let Some(s) = all.iter().find(|s| s.name == requested) {
        return Some(s.clone());
    }

    tracing::warn!(
        target: "caliban::output_styles",
        requested = requested,
        "unknown output style; falling back to built-in default",
    );
    all.iter().find(|s| s.name == "default").cloned()
}
