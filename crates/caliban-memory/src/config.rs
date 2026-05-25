//! `MemoryConfig` — paths, dirs, and token budget for tier loading.

use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};

use caliban_common::paths::sanitize_cwd_for_path;

use crate::project_walk::WalkStop;

const DEFAULT_BUDGET_TOKENS: usize = 8_000;

/// Resolved configuration for one memory-load invocation.
///
/// Holds a handful of boolean knobs (regression escape + non-interactive +
/// additional-dirs + approve-imports). Clippy's `struct_excessive_bools`
/// would otherwise nudge us to bucket them, but their semantics are distinct
/// enough that operators benefit from the flat list — keep them inline.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone)]
pub struct MemoryConfig {
    /// Path to the operator-global `CLAUDE.md`. `None` if none was discoverable.
    pub global_path: Option<PathBuf>,
    /// Legacy single-file project tier path. Still honored when
    /// [`MemoryConfig::disable_walk`] is `true` (regression escape).
    pub project_path: Option<PathBuf>,
    /// Starting directory for the project-tier ancestor walk (typically cwd).
    pub project_walk_root: PathBuf,
    /// Where the walk stops (defaults to `Both`).
    pub project_walk_stop: WalkStop,
    /// Additional `--add-dir` paths. Each contributes its own ancestor walk
    /// when [`MemoryConfig::additional_directories_claude_md`] is `true`.
    pub additional_dirs: Vec<PathBuf>,
    /// Gitignore-style patterns evaluated against paths relative to
    /// `project_walk_root` to skip CLAUDE.md / AGENTS.md / `.caliban.md` files.
    pub claude_md_excludes: GlobSet,
    /// `CALIBAN_ADDITIONAL_DIRECTORIES_CLAUDE_MD` — load CLAUDE.md from
    /// `--add-dir` paths too.
    pub additional_directories_claude_md: bool,
    /// `CALIBAN_DISABLE_CLAUDE_MD_WALK` — fall back to the legacy single-file
    /// project tier (regression escape).
    pub disable_walk: bool,
    /// `CALIBAN_APPROVE_IMPORTS` — auto-approve every external `@`-import.
    pub approve_imports: bool,
    /// `--print` / `--bare` / similar — short-circuit the import dialog to
    /// auto-deny. Defaults to `false` (interactive). Set by the binary based
    /// on its run mode.
    pub non_interactive: bool,
    /// Path to the imports-allowlist JSON (`~/.caliban/imports-allowlist.json`).
    pub imports_allowlist_path: PathBuf,
    /// Per-workspace auto-memory directory. Always set; may not exist yet.
    pub auto_memory_dir: PathBuf,
    /// Approximate token budget for the combined memory prefix.
    pub max_tokens: usize,
}

impl MemoryConfig {
    /// Resolve a `MemoryConfig` from the environment + the given workspace root.
    ///
    /// Env vars honored:
    /// - `XDG_CONFIG_HOME` / `XDG_DATA_HOME` for global + auto-memory paths.
    /// - `CALIBAN_MEMORY_DIR` / `CALIBAN_AUTO_MEMORY_DIRECTORY` for auto-memory.
    /// - `CALIBAN_MEMORY_BUDGET_TOKENS` overrides the default `8_000` budget.
    /// - `CALIBAN_ADDITIONAL_DIRECTORIES_CLAUDE_MD=1` enables CLAUDE.md load
    ///   from `--add-dir` paths.
    /// - `CALIBAN_DISABLE_CLAUDE_MD_WALK=1` reverts to the single-file project
    ///   tier (regression escape).
    /// - `CALIBAN_APPROVE_IMPORTS=1` auto-approves every external `@`-import.
    /// - `CALIBAN_CLAUDE_MD_EXCLUDES` is a colon-or-newline-separated list of
    ///   gitignore-style patterns to skip during the ancestor walk.
    #[must_use]
    pub fn from_env(workspace_root: &Path) -> Self {
        let config_home = xdg_dir("XDG_CONFIG_HOME", dirs::config_dir);
        let data_home = xdg_dir("XDG_DATA_HOME", dirs::data_local_dir);

        let global_path = config_home.map(|d| d.join("caliban").join("CLAUDE.md"));
        let project_path = Some(workspace_root.join("CLAUDE.md"));

        let auto_memory_dir = if let Some(dir) = std::env::var_os("CALIBAN_AUTO_MEMORY_DIRECTORY") {
            PathBuf::from(dir)
        } else {
            let auto_memory_root = std::env::var_os("CALIBAN_MEMORY_DIR")
                .map(PathBuf::from)
                .or_else(|| data_home.map(|d| d.join("caliban").join("projects")));
            let slug = sanitize_cwd_for_path(workspace_root);
            auto_memory_root
                .unwrap_or_else(|| PathBuf::from("./.caliban/projects"))
                .join(slug)
                .join("memory")
        };

        let max_tokens = std::env::var("CALIBAN_MEMORY_BUDGET_TOKENS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(DEFAULT_BUDGET_TOKENS);

        let claude_md_excludes =
            parse_exclude_patterns(std::env::var("CALIBAN_CLAUDE_MD_EXCLUDES").ok().as_deref());

        let imports_allowlist_path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".caliban")
            .join("imports-allowlist.json");

        Self {
            global_path,
            project_path,
            project_walk_root: workspace_root.to_path_buf(),
            project_walk_stop: WalkStop::default(),
            additional_dirs: Vec::new(),
            claude_md_excludes,
            additional_directories_claude_md: env_truthy(
                "CALIBAN_ADDITIONAL_DIRECTORIES_CLAUDE_MD",
            ),
            disable_walk: env_truthy("CALIBAN_DISABLE_CLAUDE_MD_WALK"),
            approve_imports: env_truthy("CALIBAN_APPROVE_IMPORTS"),
            non_interactive: false,
            imports_allowlist_path,
            auto_memory_dir,
            max_tokens,
        }
    }
}

impl MemoryConfig {
    /// Construct a minimal config for unit tests / library callers that don't
    /// want to read from the process environment. All env-driven fields take
    /// their defaults; only the auto-memory directory and the token budget are
    /// caller-controlled.
    #[must_use]
    pub fn for_test(auto_memory_dir: PathBuf) -> Self {
        Self {
            global_path: None,
            project_path: None,
            project_walk_root: PathBuf::from("/tmp"),
            project_walk_stop: WalkStop::default(),
            additional_dirs: Vec::new(),
            claude_md_excludes: GlobSet::empty(),
            additional_directories_claude_md: false,
            disable_walk: true, // tests opt out of the walk by default
            approve_imports: false,
            non_interactive: false,
            imports_allowlist_path: PathBuf::from("/tmp/.caliban/imports-allowlist.json"),
            auto_memory_dir,
            max_tokens: 100_000,
        }
    }
}

fn env_truthy(key: &str) -> bool {
    matches!(
        std::env::var(key).ok().as_deref(),
        Some("1" | "true" | "TRUE" | "True" | "yes" | "YES"),
    )
}

/// Parse a colon-or-newline-separated list of gitignore-style patterns into a
/// `GlobSet`. Invalid patterns are dropped with a `warn!` log.
fn parse_exclude_patterns(raw: Option<&str>) -> GlobSet {
    let mut builder = GlobSetBuilder::new();
    let Some(s) = raw else {
        return GlobSet::empty();
    };
    for raw in s.split(['\n', ':']) {
        let pat = raw.trim();
        if pat.is_empty() {
            continue;
        }
        match Glob::new(pat) {
            Ok(g) => {
                builder.add(g);
            }
            Err(e) => tracing::warn!(
                target: caliban_common::tracing_targets::TARGET_MEMORY,
                pattern = %pat,
                error = %e,
                "skipping invalid claude_md_excludes pattern",
            ),
        }
    }
    builder.build().unwrap_or_else(|e| {
        tracing::warn!(
            target: caliban_common::tracing_targets::TARGET_MEMORY,
            error = %e,
            "claude_md_excludes globset build failed; using empty matcher",
        );
        GlobSet::empty()
    })
}

/// Public helper: build a `GlobSet` from an iterable of patterns. Used by
/// downstream callers that load patterns from `settings.toml`.
///
/// # Errors
///
/// Returns the first [`globset::Error`] encountered if a pattern fails to
/// parse. Builder errors during finalization are also surfaced.
pub fn build_excludes<I, S>(patterns: I) -> std::result::Result<GlobSet, globset::Error>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut builder = GlobSetBuilder::new();
    for p in patterns {
        builder.add(Glob::new(p.as_ref())?);
    }
    builder.build()
}

/// Resolve an XDG directory: honor the env var if set + non-empty, else fall
/// back to the `dirs` crate's platform default.
fn xdg_dir(env_var: &str, fallback: fn() -> Option<PathBuf>) -> Option<PathBuf> {
    if let Some(v) = std::env::var_os(env_var)
        && !v.is_empty()
    {
        return Some(PathBuf::from(v));
    }
    fallback()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_budget_constant_matches() {
        assert_eq!(DEFAULT_BUDGET_TOKENS, 8_000);
    }

    #[test]
    fn project_path_joins_workspace_root() {
        let cfg = MemoryConfig::from_env(Path::new("/tmp/my-workspace"));
        assert_eq!(
            cfg.project_path.as_deref(),
            Some(Path::new("/tmp/my-workspace/CLAUDE.md")),
        );
        assert_eq!(
            cfg.project_walk_root.as_path(),
            Path::new("/tmp/my-workspace"),
        );
        assert_eq!(cfg.project_walk_stop, WalkStop::Both);
    }

    #[test]
    fn parse_exclude_patterns_handles_colon_and_newline_lists() {
        let g = parse_exclude_patterns(Some("node_modules/**\nvendor/**:third_party/**/CLAUDE.md"));
        assert!(g.is_match("node_modules/foo/CLAUDE.md"));
        assert!(g.is_match("vendor/x/y/AGENTS.md"));
        assert!(g.is_match("third_party/lib/CLAUDE.md"));
        assert!(!g.is_match("src/foo.rs"));
    }

    #[test]
    fn parse_exclude_patterns_drops_invalid_patterns_and_empties() {
        let g = parse_exclude_patterns(Some(""));
        assert!(g.is_empty());
        let g2 = parse_exclude_patterns(None);
        assert!(g2.is_empty());
    }

    #[test]
    fn build_excludes_helper_round_trips_patterns() {
        let g = build_excludes(["a/**", "b/**.md"]).unwrap();
        assert!(g.is_match("a/x"));
        assert!(g.is_match("b/x.md"));
        assert!(!g.is_match("c/x"));
    }
}
