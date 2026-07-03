//! `@path/file` imports inside CLAUDE.md / AGENTS.md / `.caliban.md` /
//! rules files. Recursion depth ≤ 5, cycle detection by canonical path, and a
//! first-time approval dialog for files outside the workspace root.
//!
//! Part of ADR 0036. See
//! `docs/superpowers/specs/2026-05-24-claudemd-ancestry-design.md`.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::auto::strip_html_comments;

/// Recursion depth cap. Depth = number of imports above the current parse;
/// the top-level file is depth 0, its imports are depth 1, etc.
pub const MAX_IMPORT_DEPTH: u8 = 5;

/// Per-imported-file size cap (64 KB).
pub const IMPORT_MAX_BYTES: usize = 64 * 1024;

/// Total per-tier import budget (256 KB). Once breached, further imports are
/// skipped with a `tracing::warn!`.
pub const IMPORT_TOTAL_BUDGET: usize = 256 * 1024;

/// Approval verdict for a candidate import path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportApproval {
    /// Allow this import (and persist as "always approved").
    AlwaysAllow,
    /// Allow this import for the current session only (no persistence).
    AllowOnce,
    /// Deny this import (skip and continue).
    Deny,
}

/// Decision callback the loader uses to ask the user about external paths.
///
/// Implementations live in the binary (TUI prompt). Tests use `auto-deny`
/// or `auto-allow` closures.
pub type ApprovalCallback<'a> = dyn Fn(&Path, &Path) -> ImportApproval + Send + Sync + 'a;

/// Persistent allowlist on disk.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ImportAllowlist {
    /// Schema version.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Approved entries.
    #[serde(default)]
    pub approved: Vec<ApprovedEntry>,
}

fn default_version() -> u32 {
    1
}

/// One persisted approval row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovedEntry {
    /// Canonical path that was approved.
    pub path: PathBuf,
    /// RFC-3339 timestamp.
    pub approved_at: String,
    /// Session identifier under which the approval was granted (optional).
    #[serde(default)]
    pub approved_session: Option<String>,
}

impl ImportAllowlist {
    /// Load the allowlist from `path`, returning an empty default on
    /// not-found. Other IO errors propagate.
    ///
    /// # Errors
    ///
    /// Returns [`std::io::Error`] for non-NotFound IO failures or
    /// [`serde_json::Error`] wrapped as `Other` for malformed JSON.
    pub fn load(path: &Path) -> std::io::Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    /// Atomically persist via tmp + rename. Creates the parent directory if
    /// it doesn't exist.
    ///
    /// # Errors
    ///
    /// Returns [`std::io::Error`] on any disk failure.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        caliban_common::fs::write_atomic(path, &bytes)
    }

    /// True if `path` (canonicalized) is already approved.
    #[must_use]
    pub fn contains(&self, path: &Path) -> bool {
        let needle = canonical_or(path);
        self.approved
            .iter()
            .any(|e| canonical_or(&e.path) == needle)
    }

    /// Add `path` (canonicalized) to the approved list. Idempotent.
    pub fn add(&mut self, path: &Path, session_id: Option<&str>) {
        if self.contains(path) {
            return;
        }
        self.approved.push(ApprovedEntry {
            path: canonical_or(path),
            approved_at: chrono::Utc::now().to_rfc3339(),
            approved_session: session_id.map(String::from),
        });
    }
}

/// Approval / interactive mode for the import resolver.
pub enum ApprovalMode<'a> {
    /// Interactive — invoke the callback the first time an external path is
    /// seen. Decisions may be persisted to the allowlist when `AlwaysAllow`.
    Interactive(Box<ApprovalCallback<'a>>),
    /// Approve everything silently (e.g. `CALIBAN_APPROVE_IMPORTS=1`).
    AutoAllow,
    /// Deny everything external silently (e.g. `--print` / `--bare`).
    AutoDeny,
}

impl std::fmt::Debug for ApprovalMode<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Interactive(_) => write!(f, "Interactive(<fn>)"),
            Self::AutoAllow => write!(f, "AutoAllow"),
            Self::AutoDeny => write!(f, "AutoDeny"),
        }
    }
}

/// Mutable state shared across all `@`-imports during a single tier load.
///
/// Holds the depth counter, the import stack (cycle detection), the
/// already-loaded set (dedupe), the running byte budget, and the approval
/// mode.
pub struct ImportState<'a> {
    /// Workspace root — any resolved path **not** under this directory
    /// requires approval (also tolerated under `~/.config/caliban`).
    pub workspace_root: PathBuf,
    /// Allowlist of pre-approved external paths.
    pub allowlist: ImportAllowlist,
    /// Path to the allowlist file on disk (used when persisting "always").
    pub allowlist_path: Option<PathBuf>,
    /// Approval mode (interactive / auto-allow / auto-deny).
    pub approval: ApprovalMode<'a>,
    /// Files already loaded — second `@`-import of the same path skips.
    pub loaded: BTreeSet<PathBuf>,
    /// Current recursion depth (top-level body is depth 0).
    pub depth: u8,
    /// Cycle-detection stack of canonical paths currently being resolved.
    pub import_stack: Vec<PathBuf>,
    /// Bytes of imported content emitted so far.
    pub bytes_emitted: usize,
    /// Bytes shed by the per-tier budget cap.
    pub bytes_shed: usize,
    /// Allow-once tracking (in-memory, session-scoped).
    pub session_allow_once: BTreeSet<PathBuf>,
}

impl<'a> ImportState<'a> {
    /// Build a fresh state with the given approval mode and workspace root.
    #[must_use]
    pub fn new(workspace_root: PathBuf, approval: ApprovalMode<'a>) -> Self {
        Self {
            workspace_root,
            allowlist: ImportAllowlist::default(),
            allowlist_path: None,
            approval,
            loaded: BTreeSet::new(),
            depth: 0,
            import_stack: Vec::new(),
            bytes_emitted: 0,
            bytes_shed: 0,
            session_allow_once: BTreeSet::new(),
        }
    }

    /// Attach an allowlist (loaded from disk) and remember the persistence path.
    #[must_use]
    pub fn with_allowlist(mut self, allowlist: ImportAllowlist, path: Option<PathBuf>) -> Self {
        self.allowlist = allowlist;
        self.allowlist_path = path;
        self
    }
}

/// Parsed import directive — either a clean import line we should process, or
/// `None` if the line should be left untouched.
///
/// Rules (mirroring Claude Code's parser):
/// - The first non-whitespace token starts with `@`.
/// - The token must contain `/`, start with `~`, or contain `.` somewhere
///   (otherwise it's a `@mention` / `@interface_name`, not a path).
/// - HTTP/S schemes are rejected at resolve time (here we still parse them so
///   the resolver can warn).
#[must_use]
pub fn parse_import_directive(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix('@')?;
    let token = rest.split_whitespace().next()?;
    // Empty token (`@ rest`) — not an import.
    if token.is_empty() {
        return None;
    }
    // Bare mention (no `/`, no `~`, no `.`): not a path.
    if !(token.contains('/') || token.starts_with('~') || token.contains('.')) {
        return None;
    }
    Some(token)
}

/// Error reasons for [`resolve_imports`]. These never propagate to the
/// caller — they're surfaced as inline `<!-- … -->` markers in the resolved
/// body so the model can see the failure mode.
#[derive(Debug)]
enum ImportFailure {
    UnsupportedScheme,
    NotFound,
    TooLarge { bytes: usize },
    BudgetExceeded,
    Denied,
    Cycle,
    DepthCap,
    InvalidPath,
}

impl std::fmt::Display for ImportFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedScheme => write!(f, "unsupported-scheme"),
            Self::NotFound => write!(f, "not-found"),
            Self::TooLarge { bytes } => write!(f, "too-large ({bytes} bytes)"),
            Self::BudgetExceeded => write!(f, "tier-budget-exceeded"),
            Self::Denied => write!(f, "denied"),
            Self::Cycle => write!(f, "cycle"),
            Self::DepthCap => write!(f, "depth-cap"),
            Self::InvalidPath => write!(f, "invalid-path"),
        }
    }
}

/// Recursively resolve every `@`-import in `body`. Lines that aren't pure
/// imports are passed through verbatim. Imported content is wrapped in
/// `<!-- imported from … -->` … `<!-- end … -->` markers and stripped of HTML
/// comments before splicing (so import provenance survives but content
/// comments don't bloat the prompt).
pub fn resolve_imports(body: &str, importer: &Path, state: &mut ImportState<'_>) -> String {
    // Track the importer on the cycle stack so a nested `@./importer` is
    // detected even at depth 0.
    let importer_canonical = canonical_or(importer);
    let pushed_importer = if state.import_stack.iter().any(|p| p == &importer_canonical) {
        false
    } else {
        state.import_stack.push(importer_canonical.clone());
        state.loaded.insert(importer_canonical.clone());
        true
    };

    let out = resolve_imports_inner(body, importer, state);

    if pushed_importer {
        state.import_stack.pop();
    }
    out
}

fn resolve_imports_inner(body: &str, importer: &Path, state: &mut ImportState<'_>) -> String {
    let mut out = String::with_capacity(body.len());
    for line in body.lines() {
        let Some(token) = parse_import_directive(line) else {
            out.push_str(line);
            out.push('\n');
            continue;
        };

        // Reject HTTP/S schemes outright (we don't fetch over the network).
        if token.starts_with("http://") || token.starts_with("https://") {
            push_failure(&mut out, line, token, &ImportFailure::UnsupportedScheme);
            continue;
        }

        let Some(resolved) = resolve_relative(token, importer) else {
            push_failure(&mut out, line, token, &ImportFailure::InvalidPath);
            continue;
        };
        let canonical = canonical_or(&resolved);

        // Depth cap.
        if state.depth >= MAX_IMPORT_DEPTH {
            tracing::warn!(
                target: caliban_common::tracing_targets::TARGET_MEMORY,
                importer = %importer.display(),
                token,
                "@-import depth cap reached",
            );
            push_failure(&mut out, line, token, &ImportFailure::DepthCap);
            continue;
        }

        // Cycle detection.
        if state.import_stack.iter().any(|p| p == &canonical) {
            push_failure(&mut out, line, token, &ImportFailure::Cycle);
            continue;
        }

        // De-duplicate: already loaded earlier in this tier.
        if state.loaded.contains(&canonical) {
            let _ = writeln!(out, "[@-import already loaded: {token}]");
            continue;
        }

        // Approval gate for external paths.
        if needs_approval(&canonical, &state.workspace_root)
            && !approval_grants(&canonical, importer, state)
        {
            push_failure(&mut out, line, token, &ImportFailure::Denied);
            continue;
        }

        // Read with per-file size cap.
        let raw = match std::fs::metadata(&canonical) {
            Ok(md) if md.is_file() => {
                let len_usize = usize::try_from(md.len()).unwrap_or(usize::MAX);
                if len_usize > IMPORT_MAX_BYTES {
                    push_failure(
                        &mut out,
                        line,
                        token,
                        &ImportFailure::TooLarge { bytes: len_usize },
                    );
                    continue;
                }
                if let Ok(bytes) = std::fs::read(&canonical) {
                    String::from_utf8_lossy(&bytes).into_owned()
                } else {
                    push_failure(&mut out, line, token, &ImportFailure::NotFound);
                    continue;
                }
            }
            _ => {
                push_failure(&mut out, line, token, &ImportFailure::NotFound);
                continue;
            }
        };

        // Per-tier budget check (account before recursing).
        let projected = state.bytes_emitted.saturating_add(raw.len());
        if projected > IMPORT_TOTAL_BUDGET {
            state.bytes_shed = state.bytes_shed.saturating_add(raw.len());
            push_failure(&mut out, line, token, &ImportFailure::BudgetExceeded);
            continue;
        }
        state.bytes_emitted = projected;
        state.loaded.insert(canonical.clone());
        state.depth += 1;
        state.import_stack.push(canonical.clone());

        // Recurse into the imported file. Use the inner helper so we don't
        // re-push the canonical onto the cycle stack (we already did just
        // above). Strip HTML comments from the resolved sub-body so importer
        // comments don't bloat the prompt.
        let sub = resolve_imports_inner(&raw, &canonical, state);
        let sub_stripped = strip_html_comments(&sub);

        state.import_stack.pop();
        state.depth -= 1;

        let _ = writeln!(
            out,
            "<!-- imported from {} (depth={}) -->",
            canonical.display(),
            state.depth + 1,
        );
        out.push_str(&sub_stripped);
        if !sub_stripped.ends_with('\n') {
            out.push('\n');
        }
        let _ = writeln!(out, "<!-- end {} -->", canonical.display());
    }
    out
}

fn push_failure(out: &mut String, line: &str, token: &str, why: &ImportFailure) {
    // Use brackets (not HTML comments) so the marker survives any subsequent
    // `strip_html_comments` pass — operators + the model both need to see why
    // an `@`-import was skipped.
    let _ = writeln!(out, "[@-import skipped ({why}): {token}]");
    // For UnsupportedScheme + InvalidPath, leave the original line so the
    // operator notices it visually too. For NotFound / Denied / etc. we
    // suppress the directive (it would mislead the model otherwise).
    if matches!(
        why,
        ImportFailure::UnsupportedScheme | ImportFailure::InvalidPath
    ) {
        out.push_str(line);
        out.push('\n');
    }
}

/// True when `resolved` falls outside the workspace and outside the user's
/// caliban config dir (`~/.config/caliban`). External paths require approval.
fn needs_approval(resolved: &Path, workspace_root: &Path) -> bool {
    let resolved_c = canonical_or(resolved);
    let workspace_c = canonical_or(workspace_root);
    if resolved_c.starts_with(&workspace_c) || resolved.starts_with(workspace_root) {
        return false;
    }
    if let Some(config_dir) =
        caliban_common::paths::platform_config_dir().map(|d| d.join("caliban"))
    {
        let cfg_c = canonical_or(&config_dir);
        if resolved_c.starts_with(&cfg_c) || resolved.starts_with(&config_dir) {
            return false;
        }
    }
    true
}

/// True iff approval is granted (and any persistent decision has been recorded).
fn approval_grants(resolved: &Path, importer: &Path, state: &mut ImportState<'_>) -> bool {
    let canon = canonical_or(resolved);
    if state.allowlist.contains(&canon) || state.session_allow_once.contains(&canon) {
        return true;
    }
    match &state.approval {
        ApprovalMode::AutoAllow => {
            // Treat env-flag auto-approval as "always" — persist if we have a path.
            state.allowlist.add(&canon, None);
            if let Some(p) = state.allowlist_path.as_deref() {
                let _ = state.allowlist.save(p);
            }
            true
        }
        ApprovalMode::AutoDeny => {
            tracing::warn!(
                target: caliban_common::tracing_targets::TARGET_MEMORY,
                path = %canon.display(),
                "external @-import auto-denied (non-interactive mode)",
            );
            false
        }
        ApprovalMode::Interactive(cb) => match cb(&canon, importer) {
            ImportApproval::AlwaysAllow => {
                state.allowlist.add(&canon, None);
                if let Some(p) = state.allowlist_path.as_deref() {
                    let _ = state.allowlist.save(p);
                }
                true
            }
            ImportApproval::AllowOnce => {
                state.session_allow_once.insert(canon);
                true
            }
            ImportApproval::Deny => false,
        },
    }
}

/// Resolve `token` (the bit after `@`) into a filesystem path. `~` expands
/// against the home directory; relative paths join the importer's directory.
/// Returns `None` for empty or malformed tokens.
#[must_use]
fn resolve_relative(token: &str, importer: &Path) -> Option<PathBuf> {
    if token.is_empty() {
        return None;
    }
    if let Some(rest) = token.strip_prefix("~/") {
        let home = dirs::home_dir()?;
        return Some(home.join(rest));
    }
    if token == "~" {
        return dirs::home_dir();
    }
    let p = Path::new(token);
    if p.is_absolute() {
        return Some(p.to_path_buf());
    }
    let base = importer.parent().unwrap_or_else(|| Path::new("."));
    Some(normalize(&base.join(p)))
}

/// Normalize `..` / `.` segments without touching the filesystem.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Best-effort canonicalize; falls back to a normalized form when the path
/// doesn't yet exist (we still want a stable key for cycle detection).
#[must_use]
pub fn canonical_or(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| normalize(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn deny_cb<'a>() -> ApprovalMode<'a> {
        ApprovalMode::AutoDeny
    }

    #[test]
    fn parse_import_directive_recognizes_path_like_tokens() {
        assert_eq!(parse_import_directive("@./foo.md"), Some("./foo.md"));
        assert_eq!(
            parse_import_directive("@~/notes/api.md"),
            Some("~/notes/api.md"),
        );
        assert_eq!(
            parse_import_directive("@/abs/path.md"),
            Some("/abs/path.md")
        );
        assert_eq!(parse_import_directive("@foo.md"), Some("foo.md"));
        // Indented import is still recognized.
        assert_eq!(parse_import_directive("    @./foo.md"), Some("./foo.md"));
    }

    #[test]
    fn parse_import_directive_rejects_user_mentions_and_interface_names() {
        assert_eq!(parse_import_directive("@someone"), None);
        assert_eq!(parse_import_directive("@MyInterface"), None);
        assert_eq!(parse_import_directive("ping @someone here"), None);
        assert_eq!(parse_import_directive("@_underscore"), None);
        assert_eq!(parse_import_directive("@"), None);
    }

    #[test]
    fn resolve_imports_inlines_referenced_file() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let importer = root.join("CLAUDE.md");
        fs::write(root.join("part.md"), "PART-BODY\n").unwrap();
        fs::write(&importer, "header\n@./part.md\nfooter\n").unwrap();
        let body = fs::read_to_string(&importer).unwrap();

        let mut state = ImportState::new(root.to_path_buf(), deny_cb());
        let out = resolve_imports(&body, &importer, &mut state);
        assert!(out.contains("header"));
        assert!(out.contains("PART-BODY"));
        assert!(out.contains("footer"));
        assert!(out.contains("imported from"));
        assert!(out.contains("end"));
    }

    #[test]
    fn resolve_imports_enforces_depth_cap_at_five() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        // Build a chain: top → a → b → c → d → e → f (7 levels). e (depth 5
        // from top) imports f (depth 6) — the depth-6 read must be rejected
        // and the directive elided.
        fs::write(root.join("top.md"), "@./a.md\n").unwrap();
        fs::write(root.join("a.md"), "A-LEVEL\n@./b.md\n").unwrap();
        fs::write(root.join("b.md"), "B-LEVEL\n@./c.md\n").unwrap();
        fs::write(root.join("c.md"), "C-LEVEL\n@./d.md\n").unwrap();
        fs::write(root.join("d.md"), "D-LEVEL\n@./e.md\n").unwrap();
        fs::write(root.join("e.md"), "E-LEVEL\n@./f.md\n").unwrap();
        fs::write(root.join("f.md"), "F-SHOULD-NOT-APPEAR\n").unwrap();

        let body = fs::read_to_string(root.join("top.md")).unwrap();
        let mut state = ImportState::new(root.to_path_buf(), deny_cb());
        let out = resolve_imports(&body, &root.join("top.md"), &mut state);
        assert!(out.contains("A-LEVEL"));
        assert!(out.contains("E-LEVEL"));
        assert!(
            !out.contains("F-SHOULD-NOT-APPEAR"),
            "depth-6 file should have been rejected: {out}",
        );
        assert!(out.contains("depth-cap"));
    }

    #[test]
    fn resolve_imports_allows_exactly_five_levels() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::write(root.join("top.md"), "@./a.md\n").unwrap();
        fs::write(root.join("a.md"), "A-LEVEL\n@./b.md\n").unwrap();
        fs::write(root.join("b.md"), "B-LEVEL\n@./c.md\n").unwrap();
        fs::write(root.join("c.md"), "C-LEVEL\n@./d.md\n").unwrap();
        fs::write(root.join("d.md"), "D-LEVEL\n@./e.md\n").unwrap();
        fs::write(root.join("e.md"), "E-LEAF\n").unwrap();

        let body = fs::read_to_string(root.join("top.md")).unwrap();
        let mut state = ImportState::new(root.to_path_buf(), deny_cb());
        let out = resolve_imports(&body, &root.join("top.md"), &mut state);
        assert!(out.contains("E-LEAF"));
    }

    #[test]
    fn resolve_imports_detects_cycles() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::write(root.join("a.md"), "A-BODY\n@./b.md\n").unwrap();
        fs::write(root.join("b.md"), "B-BODY\n@./a.md\n").unwrap();
        let mut state = ImportState::new(root.to_path_buf(), deny_cb());
        let body = fs::read_to_string(root.join("a.md")).unwrap();
        let out = resolve_imports(&body, &root.join("a.md"), &mut state);
        assert!(out.contains("A-BODY"));
        assert!(out.contains("B-BODY"));
        assert!(out.contains("cycle"), "no cycle marker: {out}");
    }

    #[test]
    fn resolve_imports_rejects_http_urls() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let importer = root.join("CLAUDE.md");
        fs::write(&importer, "header\n@https://example.com/x.md\nfooter\n").unwrap();
        let body = fs::read_to_string(&importer).unwrap();
        let mut state = ImportState::new(root.to_path_buf(), ApprovalMode::AutoAllow);
        let out = resolve_imports(&body, &importer, &mut state);
        assert!(out.contains("unsupported-scheme"));
    }

    #[test]
    fn first_time_external_import_prompts_then_denies() {
        let tmp = TempDir::new().unwrap();
        let external = tmp.path().join("outside");
        fs::create_dir_all(&external).unwrap();
        fs::write(external.join("rules.md"), "EXTERNAL").unwrap();
        let workspace = tmp.path().join("ws");
        fs::create_dir_all(&workspace).unwrap();
        let importer = workspace.join("CLAUDE.md");
        let import_token = format!("@{}", external.join("rules.md").display());
        fs::write(&importer, format!("{import_token}\n")).unwrap();
        let body = fs::read_to_string(&importer).unwrap();

        // Non-interactive: AutoDeny.
        let mut state = ImportState::new(workspace.clone(), ApprovalMode::AutoDeny);
        let out = resolve_imports(&body, &importer, &mut state);
        assert!(!out.contains("EXTERNAL"));
        assert!(out.contains("denied"));
    }

    #[test]
    fn first_time_external_import_can_be_approved() {
        let tmp = TempDir::new().unwrap();
        let external = tmp.path().join("outside");
        fs::create_dir_all(&external).unwrap();
        fs::write(external.join("rules.md"), "EXTERNAL").unwrap();
        let workspace = tmp.path().join("ws");
        fs::create_dir_all(&workspace).unwrap();
        let importer = workspace.join("CLAUDE.md");
        let import_token = format!("@{}", external.join("rules.md").display());
        fs::write(&importer, format!("{import_token}\n")).unwrap();
        let body = fs::read_to_string(&importer).unwrap();

        // Interactive: always-allow on first ask.
        let cb: Box<ApprovalCallback<'static>> =
            Box::new(|_p: &Path, _i: &Path| ImportApproval::AlwaysAllow);
        let mut state = ImportState::new(workspace.clone(), ApprovalMode::Interactive(cb));
        let out = resolve_imports(&body, &importer, &mut state);
        assert!(out.contains("EXTERNAL"), "expected EXTERNAL inlined: {out}");
        assert!(
            state.allowlist.contains(&external.join("rules.md")),
            "always-allow should add to allowlist",
        );
    }

    #[test]
    fn cached_approval_skips_dialog_on_second_load() {
        let tmp = TempDir::new().unwrap();
        let external = tmp.path().join("outside");
        fs::create_dir_all(&external).unwrap();
        fs::write(external.join("rules.md"), "EXTERNAL").unwrap();
        let workspace = tmp.path().join("ws");
        fs::create_dir_all(&workspace).unwrap();
        let importer = workspace.join("CLAUDE.md");
        let import_token = format!("@{}", external.join("rules.md").display());
        fs::write(&importer, format!("{import_token}\n")).unwrap();
        let body = fs::read_to_string(&importer).unwrap();

        // Pre-populate the allowlist with the external path.
        let mut allow = ImportAllowlist::default();
        allow.add(&external.join("rules.md"), None);

        // Callback that panics if invoked — proving the allowlist short-circuits.
        let cb: Box<ApprovalCallback<'static>> =
            Box::new(|_p: &Path, _i: &Path| panic!("dialog should not be invoked"));
        let mut state = ImportState::new(workspace.clone(), ApprovalMode::Interactive(cb))
            .with_allowlist(allow, None);
        let out = resolve_imports(&body, &importer, &mut state);
        assert!(out.contains("EXTERNAL"));
    }

    #[test]
    fn allowlist_round_trips_through_disk() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(".caliban").join("imports-allowlist.json");
        let mut allow = ImportAllowlist::default();
        allow.add(Path::new("/Users/x/notes/api.md"), Some("session-1"));
        allow.save(&path).unwrap();
        let loaded = ImportAllowlist::load(&path).unwrap();
        assert_eq!(loaded.approved.len(), 1);
        assert!(loaded.contains(Path::new("/Users/x/notes/api.md")));
    }

    #[test]
    fn html_comments_stripped_from_imported_content() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let importer = root.join("CLAUDE.md");
        fs::write(
            root.join("part.md"),
            "VISIBLE\n<!-- secret stuff -->\nMORE\n",
        )
        .unwrap();
        fs::write(&importer, "@./part.md\n").unwrap();
        let body = fs::read_to_string(&importer).unwrap();
        let mut state = ImportState::new(root.to_path_buf(), deny_cb());
        let out = resolve_imports(&body, &importer, &mut state);
        assert!(out.contains("VISIBLE"));
        assert!(out.contains("MORE"));
        assert!(
            !out.contains("secret stuff"),
            "html comment leaked into output: {out}",
        );
    }

    #[test]
    fn empty_body_after_stripping_does_not_panic() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let importer = root.join("CLAUDE.md");
        fs::write(root.join("part.md"), "<!-- nothing -->\n").unwrap();
        fs::write(&importer, "@./part.md\n").unwrap();
        let body = fs::read_to_string(&importer).unwrap();
        let mut state = ImportState::new(root.to_path_buf(), deny_cb());
        let _ = resolve_imports(&body, &importer, &mut state);
    }
}
