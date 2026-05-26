//! Async tier loader + budget enforcement.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::auto::strip_html_comments;
use crate::config::MemoryConfig;
use crate::error::{MemoryError, Result};
use crate::prefix::{MemoryPrefix, ProjectTier, TierFile, TierKind};
use crate::project_imports::{
    ApprovalMode, ImportAllowlist, ImportState, canonical_or, resolve_imports,
};
use crate::project_walk::walk_ancestors;
use crate::rules::scan_caliban_rules;

/// Cap per-file disk read at 256 KB so a runaway memory file cannot wedge the
/// startup path.
const MAX_FILE_BYTES: usize = 256 * 1024;

/// Maximum number of `MEMORY.md` lines spliced into the prompt.
const AUTO_MAX_LINES: usize = 200;

/// Maximum bytes of `MEMORY.md` spliced into the prompt. Whichever cap is
/// reached first wins.
const AUTO_MAX_BYTES: usize = 25 * 1024;

/// Approximate-token estimator (chars / 4). Provider-agnostic, deterministic.
#[must_use]
pub fn estimate_tokens(body: &str) -> usize {
    body.chars().count() / 4
}

/// Seed file written into a freshly created auto-memory directory on first run.
const SEED_MEMORY_MD: &str = "# Memory index\n\n_No memories yet. Add entries below as `- [title](slug.md) — one-line summary`._\n";

/// Conventions block appended to MEMORY.md (in-memory only) on every load so
/// the agent always sees the writing rules without the operator maintaining them.
const CONVENTIONS_BLOCK: &str = concat!(
    "\n<!-- caliban: auto-memory conventions follow; do not delete -->\n",
    "Write to this index when you learn something durable about the user, project, or environment. ",
    "One topic per file, slug in kebab-case. Do not save transient task state, debug traces, or ",
    "facts already in the repo. Keep this file ≤ 200 lines.\n",
);

/// Environment kill-switch. When set to a truthy value, the auto tier is
/// dropped from the prefix entirely (and the auto-memory skill is hidden by
/// the binary, which checks the same flag).
const DISABLE_ENV: &str = "CALIBAN_DISABLE_AUTO_MEMORY";

fn auto_memory_disabled() -> bool {
    matches!(
        std::env::var(DISABLE_ENV).ok().as_deref(),
        Some("1" | "true" | "TRUE" | "True" | "yes")
    )
}

/// Load all three memory tiers from disk, enforce the token budget, and return
/// the assembled [`MemoryPrefix`].
///
/// Missing files are not errors — they contribute `None` tiers.
///
/// # Errors
///
/// Returns [`MemoryError::Io`] if a tier file exists but cannot be read
/// (permissions, etc.), or [`MemoryError::AutoMemorySeed`] if the auto-memory
/// directory exists check / seed write fails.
pub async fn load(config: &MemoryConfig) -> Result<MemoryPrefix> {
    let auto_disabled = auto_memory_disabled();

    // Seed the auto-memory dir if it doesn't exist yet (skip when disabled —
    // we don't want a CI run to create a project dir).
    let auto_md = if auto_disabled {
        None
    } else {
        Some(ensure_auto_memory(&config.auto_memory_dir).await?)
    };

    let global = read_optional(config.global_path.as_deref()).await?;
    let auto_raw = if let Some(p) = auto_md.as_deref() {
        read_optional_with_caps(Some(p), AUTO_MAX_LINES, AUTO_MAX_BYTES).await?
    } else {
        None
    };

    let global = global.map(post_process_static);
    // Inject conventions into the auto-memory body (in-memory only), then
    // strip HTML comments so the splice stays clean.
    let auto = auto_raw.map(|mut t| {
        if !t.body.contains("caliban: auto-memory conventions follow") {
            if !t.body.ends_with('\n') {
                t.body.push('\n');
            }
            t.body.push_str(CONVENTIONS_BLOCK);
        }
        t.body = strip_html_comments(&t.body);
        t.estimated_tokens = estimate_tokens(&t.body);
        t
    });

    // Build the project tier — either the legacy single-file load (regression
    // escape) or the new ancestor walk + imports + rules.
    let (project_legacy, project_tier) = if config.disable_walk {
        let legacy = read_optional(config.project_path.as_deref())
            .await?
            .map(post_process_static);
        (legacy, None)
    } else {
        let project_tier = build_project_tier(config).await?;
        let legacy = project_tier.to_legacy_tier();
        (legacy, Some(project_tier))
    };

    let mut prefix = MemoryPrefix {
        global,
        project: project_legacy,
        project_tier,
        auto,
        estimated_tokens: 0,
        truncated: false,
    };

    enforce_caps_and_budget(&mut prefix, config);
    prefix.estimated_tokens = prefix.global.as_ref().map_or(0, |t| t.estimated_tokens)
        + prefix.project.as_ref().map_or(0, |t| t.estimated_tokens)
        + prefix.auto.as_ref().map_or(0, |t| t.estimated_tokens);

    Ok(prefix)
}

/// Build the rich project tier: ancestor walk + `@`-imports per file + rules.
async fn build_project_tier(config: &MemoryConfig) -> Result<ProjectTier> {
    let mut tier = ProjectTier::default();

    let mut walked = walk_ancestors(
        &config.project_walk_root,
        config.project_walk_stop,
        &config.claude_md_excludes,
    );
    if config.additional_directories_claude_md {
        for dir in &config.additional_dirs {
            let extra = walk_ancestors(dir, config.project_walk_stop, &config.claude_md_excludes);
            walked.extend(extra);
        }
    }

    // Effective workspace root for approval is the highest dir reached during
    // the walk (typically the git root). This means any `@`-import that
    // resolves *inside* the walked tree never needs approval, even when the
    // walk started in a deeply-nested subdirectory.
    let approval_root = walked
        .first()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| config.project_walk_root.clone());

    // Load the import allowlist once.
    let allowlist = ImportAllowlist::load(&config.imports_allowlist_path).unwrap_or_default();
    let approval = approval_mode_for(config);
    let mut state = ImportState::new(approval_root, approval)
        .with_allowlist(allowlist, Some(config.imports_allowlist_path.clone()));

    for path in walked {
        let Some(body) = read_capped(&path).await? else {
            continue;
        };
        let resolved = resolve_imports(&body, &path, &mut state);
        let stripped = strip_html_comments(&resolved);
        let estimated_tokens = estimate_tokens(&stripped);
        // Record which canonical paths the import resolver had pulled in for
        // this file — they're shown in `/memory` for provenance.
        for imp in &state.loaded {
            if tier.imports.iter().any(|f| canonical_or(&f.path) == *imp) {
                continue;
            }
            if imp == &canonical_or(&path) {
                continue;
            }
            // Imports are inlined into `resolved` already; we record their
            // paths via a small stub TierFile (the body is empty since the
            // real content is in the parent tier file).
            tier.imports.push(TierFile {
                path: imp.clone(),
                body: String::new(),
                estimated_tokens: 0,
                truncated_bytes: 0,
            });
        }
        tier.base_files.push(TierFile {
            path,
            body: stripped,
            estimated_tokens,
            truncated_bytes: 0,
        });
    }

    // Rules: always-active ones load into the prompt now; path-scoped rules
    // wait for a path-touch via AncestryAddendum / RulesActivator.
    let rule_set = scan_caliban_rules(&config.project_walk_root);
    for rule in rule_set.always_active() {
        let resolved = resolve_imports(&rule.body, &rule.path, &mut state);
        let stripped = strip_html_comments(&resolved);
        let estimated_tokens = estimate_tokens(&stripped);
        tier.active_rules.push(TierFile {
            path: rule.path.clone(),
            body: stripped,
            estimated_tokens,
            truncated_bytes: 0,
        });
    }

    Ok(tier)
}

fn approval_mode_for(config: &MemoryConfig) -> ApprovalMode<'static> {
    if config.approve_imports {
        ApprovalMode::AutoAllow
    } else if config.non_interactive {
        ApprovalMode::AutoDeny
    } else {
        // Default for the library: no interactive prompt available — auto-deny.
        // Binaries hook a real TUI prompt by replacing this mode at config time
        // (planned wiring; for v1 we deny silently and log).
        ApprovalMode::AutoDeny
    }
}

async fn read_capped(path: &Path) -> Result<Option<String>> {
    match tokio::fs::metadata(path).await {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(MemoryError::Io {
                path: path.to_path_buf(),
                source: e,
            });
        }
        Ok(md) if !md.is_file() => return Ok(None),
        Ok(_) => {}
    }
    let raw = tokio::fs::read(path).await.map_err(|e| MemoryError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let clamped = if raw.len() > MAX_FILE_BYTES {
        &raw[..MAX_FILE_BYTES]
    } else {
        &raw[..]
    };
    Ok(Some(String::from_utf8_lossy(clamped).into_owned()))
}

/// Post-process a tier file that's not the auto tier: strip HTML comments and
/// re-estimate tokens. Applied to global + project so that conventions /
/// internal notes the operator hides in comments don't bloat the splice.
fn post_process_static(mut t: TierFile) -> TierFile {
    t.body = strip_html_comments(&t.body);
    t.estimated_tokens = estimate_tokens(&t.body);
    t
}

async fn read_optional(path: Option<&Path>) -> Result<Option<TierFile>> {
    let Some(path) = path else { return Ok(None) };
    match tokio::fs::metadata(path).await {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(MemoryError::Io {
                path: path.to_path_buf(),
                source: e,
            });
        }
        Ok(_) => {}
    }
    let raw = tokio::fs::read(path).await.map_err(|e| MemoryError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let truncated_bytes = raw.len().saturating_sub(MAX_FILE_BYTES);
    let clamped = if truncated_bytes > 0 {
        &raw[..MAX_FILE_BYTES]
    } else {
        &raw[..]
    };
    let body = String::from_utf8_lossy(clamped).into_owned();
    Ok(Some(TierFile {
        path: path.to_path_buf(),
        estimated_tokens: estimate_tokens(&body),
        body,
        truncated_bytes,
    }))
}

/// Read with cap-by-lines-or-bytes (whichever wins first). Used for the auto
/// tier where the spec mandates a strict 200-line / 25 KB ceiling on the
/// spliced body.
async fn read_optional_with_caps(
    path: Option<&Path>,
    max_lines: usize,
    max_bytes: usize,
) -> Result<Option<TierFile>> {
    let Some(path) = path else { return Ok(None) };
    match tokio::fs::metadata(path).await {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(MemoryError::Io {
                path: path.to_path_buf(),
                source: e,
            });
        }
        Ok(_) => {}
    }
    let raw_bytes = tokio::fs::read(path).await.map_err(|e| MemoryError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let raw = String::from_utf8_lossy(&raw_bytes).into_owned();
    let total_bytes = raw.len();

    // Apply caps. We walk lines and accumulate, stopping when either ceiling
    // would be exceeded.
    let mut kept = String::new();
    for (lines_used, line) in raw.split_inclusive('\n').enumerate() {
        if lines_used >= max_lines {
            break;
        }
        if kept.len() + line.len() > max_bytes {
            break;
        }
        kept.push_str(line);
    }
    let truncated_bytes = total_bytes.saturating_sub(kept.len());

    Ok(Some(TierFile {
        path: path.to_path_buf(),
        estimated_tokens: estimate_tokens(&kept),
        body: kept,
        truncated_bytes,
    }))
}

async fn ensure_auto_memory(dir: &Path) -> Result<PathBuf> {
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| MemoryError::AutoMemorySeed {
            path: dir.to_path_buf(),
            source: e,
        })?;
    let memory_md = dir.join("MEMORY.md");
    if tokio::fs::try_exists(&memory_md).await.unwrap_or(false) {
        return Ok(memory_md);
    }
    tokio::fs::write(&memory_md, SEED_MEMORY_MD)
        .await
        .map_err(|e| MemoryError::AutoMemorySeed {
            path: memory_md.clone(),
            source: e,
        })?;
    Ok(memory_md)
}

/// Apply per-scope caps from `config`, then the combined `max_tokens` ceiling.
///
/// Per-scope ordering:
/// 1. `cap_tokens_auto` truncates the auto tier in isolation.
/// 2. `cap_tokens_claude_md` applies to the combined global + project tiers;
///    truncates project first (less important within the CLAUDE.md group), then
///    global if still over.
/// 3. The combined `max_tokens` ceiling is enforced last via [`enforce_budget`],
///    which walks tiers in priority order (auto → project → global).
fn enforce_caps_and_budget(prefix: &mut MemoryPrefix, config: &MemoryConfig) {
    if let Some(cap) = config.cap_tokens_auto {
        let effective = config.effective_cap(cap, config.cap_tokens_claude_md);
        if prefix
            .auto
            .as_ref()
            .is_some_and(|t| t.estimated_tokens > effective)
        {
            truncate_tier(prefix, TierKind::Auto, effective);
            prefix.truncated = true;
        }
    }
    if let Some(cap) = config.cap_tokens_claude_md {
        let effective = config.effective_cap(cap, config.cap_tokens_auto);
        let global_t = prefix.global.as_ref().map_or(0, |t| t.estimated_tokens);
        let project_t = prefix.project.as_ref().map_or(0, |t| t.estimated_tokens);
        if global_t + project_t > effective {
            // Truncate project first, then global.
            let project_allowance = effective.saturating_sub(global_t);
            if project_allowance < project_t {
                truncate_tier(prefix, TierKind::Project, project_allowance);
                prefix.truncated = true;
            }
            let project_after = prefix.project.as_ref().map_or(0, |t| t.estimated_tokens);
            let global_allowance = effective.saturating_sub(project_after);
            if global_allowance < global_t {
                truncate_tier(prefix, TierKind::Global, global_allowance);
                prefix.truncated = true;
            }
        }
    }
    enforce_budget(prefix, config.max_tokens);
}

/// Truncate tiers in priority order (auto → project → global) until the total
/// token estimate fits the budget. Each truncation snips at a line boundary
/// and appends a marker. Sets `prefix.truncated` accordingly.
fn enforce_budget(prefix: &mut MemoryPrefix, max_tokens: usize) {
    let total = total_tokens(prefix);
    if total <= max_tokens {
        return;
    }

    // Reverse priority order: shed auto first, then project, then global.
    for kind in [TierKind::Auto, TierKind::Project, TierKind::Global] {
        if total_tokens(prefix) <= max_tokens {
            return;
        }
        let allowance = max_tokens.saturating_sub(other_tokens(prefix, kind));
        truncate_tier(prefix, kind, allowance);
        prefix.truncated = true;
    }

    // If we got here and we're still over budget, the global tier alone is
    // bigger than the cap. We already truncated it; emit a warning so the
    // operator's debug log captures this case.
    if total_tokens(prefix) > max_tokens
        && let Some(g) = prefix.global.as_ref()
    {
        tracing::warn!(
            target: caliban_common::tracing_targets::TARGET_MEMORY,
            path = %g.path.display(),
            estimated_tokens = g.estimated_tokens,
            cap = max_tokens,
            "global memory file exceeds budget even after truncation",
        );
    }
}

fn total_tokens(p: &MemoryPrefix) -> usize {
    p.global.as_ref().map_or(0, |t| t.estimated_tokens)
        + p.project.as_ref().map_or(0, |t| t.estimated_tokens)
        + p.auto.as_ref().map_or(0, |t| t.estimated_tokens)
}

fn other_tokens(p: &MemoryPrefix, exclude: TierKind) -> usize {
    let mut sum = 0;
    if !matches!(exclude, TierKind::Global)
        && let Some(t) = p.global.as_ref()
    {
        sum += t.estimated_tokens;
    }
    if !matches!(exclude, TierKind::Project)
        && let Some(t) = p.project.as_ref()
    {
        sum += t.estimated_tokens;
    }
    if !matches!(exclude, TierKind::Auto)
        && let Some(t) = p.auto.as_ref()
    {
        sum += t.estimated_tokens;
    }
    sum
}

/// Bytes reserved at the end of a truncated tier for the `[truncated: ...]`
/// marker. Conservative — actual marker is ~80 bytes.
const MARKER_RESERVE_BYTES: usize = 128;

fn truncate_tier(prefix: &mut MemoryPrefix, kind: TierKind, max_tokens: usize) {
    let slot: &mut Option<TierFile> = match kind {
        TierKind::Global => &mut prefix.global,
        TierKind::Project => &mut prefix.project,
        TierKind::Auto => &mut prefix.auto,
    };
    let Some(tier) = slot.as_mut() else { return };
    if tier.estimated_tokens <= max_tokens {
        return;
    }
    // Reserve headroom for the marker so the resulting body still fits.
    let target_bytes = max_tokens
        .saturating_mul(4)
        .saturating_sub(MARKER_RESERVE_BYTES);
    let original_len = tier.body.len();
    if target_bytes >= original_len {
        return;
    }
    // Snip at the last newline before target_bytes.
    let cut = tier.body[..target_bytes]
        .rfind('\n')
        .map_or(target_bytes, |i| i + 1);
    let mut new_body = tier.body[..cut].to_string();
    let shed = original_len - cut;
    let _ = writeln!(
        new_body,
        "\n[truncated: {shed} bytes over budget; raise CALIBAN_MEMORY_BUDGET_TOKENS or trim]",
    );
    tier.truncated_bytes = shed;
    tier.body = new_body;
    tier.estimated_tokens = estimate_tokens(&tier.body);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prefix::{MemoryPrefix, TierFile, TierKind};

    fn tier(body: &str) -> TierFile {
        TierFile {
            path: std::path::PathBuf::from("/tmp/x.md"),
            estimated_tokens: estimate_tokens(body),
            body: body.to_string(),
            truncated_bytes: 0,
        }
    }

    #[test]
    fn estimate_tokens_uses_chars_div_4() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abc"), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens(&"a".repeat(40)), 10);
    }

    #[test]
    fn budget_under_cap_no_truncation() {
        let mut p = MemoryPrefix {
            global: Some(tier("hi")),
            project: Some(tier("there")),
            auto: Some(tier("again")),
            ..MemoryPrefix::default()
        };
        enforce_budget(&mut p, 8_000);
        assert!(!p.truncated);
        assert_eq!(p.global.unwrap().truncated_bytes, 0);
        assert_eq!(p.project.unwrap().truncated_bytes, 0);
        assert_eq!(p.auto.unwrap().truncated_bytes, 0);
    }

    #[test]
    fn budget_truncates_auto_first() {
        let small = "x".repeat(100); // ~25 tokens
        let big_auto = "line\n".repeat(2_000); // ~2500 tokens
        let mut p = MemoryPrefix {
            global: Some(tier(&small)),
            project: Some(tier(&small)),
            auto: Some(tier(&big_auto)),
            ..MemoryPrefix::default()
        };
        enforce_budget(&mut p, 200);
        assert!(p.truncated);
        assert!(p.auto.as_ref().unwrap().truncated_bytes > 0);
        // Global + project should be untouched since auto shedding was enough.
        assert_eq!(p.global.as_ref().unwrap().truncated_bytes, 0);
        assert_eq!(p.project.as_ref().unwrap().truncated_bytes, 0);
    }

    #[test]
    fn truncate_cuts_on_line_boundary() {
        let mut body = String::new();
        for i in 0..100 {
            writeln!(body, "line {i:03}").unwrap();
        }
        let mut p = MemoryPrefix {
            global: None,
            project: None,
            auto: Some(tier(&body)),
            ..MemoryPrefix::default()
        };
        enforce_budget(&mut p, 20);
        let cut_body = &p.auto.as_ref().unwrap().body;
        // Snipped result must end on a newline (or the marker we appended).
        // Walk lines and ensure every kept body line is intact.
        for line in cut_body.lines().take_while(|l| l.starts_with("line ")) {
            assert!(line.len() == "line NNN".len(), "non-boundary cut: {line:?}");
        }
        assert!(cut_body.contains("[truncated:"));
    }

    #[test]
    fn budget_truncates_global_when_only_one_tier_present() {
        let big = "g".repeat(10_000);
        let mut p = MemoryPrefix {
            global: Some(tier(&big)),
            project: None,
            auto: None,
            ..MemoryPrefix::default()
        };
        enforce_budget(&mut p, 500);
        assert!(p.truncated);
        assert!(p.global.unwrap().truncated_bytes > 0);
    }

    #[test]
    fn other_tokens_excludes_correct_tier() {
        let p = MemoryPrefix {
            global: Some(tier(&"a".repeat(40))),  // 10 tokens
            project: Some(tier(&"b".repeat(80))), // 20 tokens
            auto: Some(tier(&"c".repeat(120))),   // 30 tokens
            ..MemoryPrefix::default()
        };
        assert_eq!(other_tokens(&p, TierKind::Auto), 30);
        assert_eq!(other_tokens(&p, TierKind::Project), 40);
        assert_eq!(other_tokens(&p, TierKind::Global), 50);
    }

    #[test]
    fn enforce_caps_truncates_auto_to_per_scope_cap() {
        let big_auto = "x".repeat(4_000); // ~1000 tokens
        let mut p = MemoryPrefix {
            global: Some(tier("hi")),
            project: Some(tier("there")),
            auto: Some(tier(&big_auto)),
            ..MemoryPrefix::default()
        };
        let cfg = MemoryConfig {
            cap_tokens_auto: Some(100),
            ..MemoryConfig::for_test(std::path::PathBuf::from("/tmp/m"))
        };
        enforce_caps_and_budget(&mut p, &cfg);
        assert!(p.truncated);
        let auto = p.auto.as_ref().unwrap();
        let auto_tokens = auto.estimated_tokens;
        assert!(
            auto_tokens <= 100,
            "auto cap not honored: {auto_tokens} > 100"
        );
    }

    #[test]
    fn enforce_caps_truncates_project_first_then_global_under_claude_md_cap() {
        let small = "x".repeat(40); // ~10 tokens
        let big_project = "p".repeat(4_000); // ~1000 tokens
        let big_global = "g".repeat(4_000); // ~1000 tokens
        let mut p = MemoryPrefix {
            global: Some(tier(&big_global)),
            project: Some(tier(&big_project)),
            auto: Some(tier(&small)),
            ..MemoryPrefix::default()
        };
        let cfg = MemoryConfig {
            cap_tokens_claude_md: Some(500),
            ..MemoryConfig::for_test(std::path::PathBuf::from("/tmp/m"))
        };
        enforce_caps_and_budget(&mut p, &cfg);
        assert!(p.truncated);
        let global_t = p.global.as_ref().map_or(0, |t| t.estimated_tokens);
        let project_t = p.project.as_ref().map_or(0, |t| t.estimated_tokens);
        assert!(
            global_t + project_t <= 500,
            "claude_md cap not honored: {global_t} + {project_t} > 500",
        );
        // Project should bear the brunt — global stays full (~1000 tokens
        // exceeds 500 alone, so global is also truncated, but project should
        // be MORE truncated than global).
        // Actually since global alone (1000) > cap (500), both will be hit.
        // Just check project is smaller than global, since project goes first.
        assert!(
            project_t == 0 || project_t < global_t,
            "project should be truncated first: project={project_t} global={global_t}",
        );
    }

    #[test]
    fn enforce_caps_proportional_scaling_when_per_scope_sum_exceeds_combined() {
        // Both per-scope caps are 20K, combined is 20K → effective caps scale to 10K each.
        let mut big_auto = String::new();
        for _ in 0..20_000 {
            big_auto.push_str("aaaa\n");
        } // ~25K tokens
        let mut big_md = String::new();
        for _ in 0..20_000 {
            big_md.push_str("bbbb\n");
        } // ~25K tokens
        let mut p = MemoryPrefix {
            global: Some(tier(&big_md)),
            project: None,
            auto: Some(tier(&big_auto)),
            ..MemoryPrefix::default()
        };
        let cfg = MemoryConfig {
            max_tokens: 20_000,
            cap_tokens_auto: Some(20_000),
            cap_tokens_claude_md: Some(20_000),
            ..MemoryConfig::for_test(std::path::PathBuf::from("/tmp/m"))
        };
        enforce_caps_and_budget(&mut p, &cfg);
        // After per-scope scaling: each effective cap = 10K.
        // Then combined enforce_budget(20K) is a no-op since total = 10K + 10K = 20K.
        let auto_t = p.auto.as_ref().unwrap().estimated_tokens;
        let global_t = p.global.as_ref().unwrap().estimated_tokens;
        assert!(auto_t <= 10_000, "auto effective cap: {auto_t}");
        assert!(global_t <= 10_000, "global effective cap: {global_t}");
        assert!(auto_t + global_t <= 20_000);
    }

    #[tokio::test]
    async fn auto_load_caps_at_two_hundred_lines() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("memory");
        std::fs::create_dir_all(&dir).unwrap();
        let mut body = String::new();
        for i in 0..400 {
            writeln!(body, "line-{i:04}").unwrap();
        }
        std::fs::write(dir.join("MEMORY.md"), &body).unwrap();

        let cfg = MemoryConfig::for_test(dir.clone());
        let p = load(&cfg).await.unwrap();
        let auto = p.auto.as_ref().expect("auto loaded");
        // Strip the conventions block we add before counting.
        let kept_lines = auto.body.lines().filter(|l| l.starts_with("line-")).count();
        assert_eq!(kept_lines, AUTO_MAX_LINES);
        assert!(auto.truncated_bytes > 0);
    }

    #[tokio::test]
    async fn auto_load_caps_at_byte_ceiling() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("memory");
        std::fs::create_dir_all(&dir).unwrap();
        // 10 lines but each line is ~5 KB → cap on bytes hits before the line cap.
        let mut body = String::new();
        for i in 0..10 {
            let chunk = "x".repeat(5_000);
            writeln!(body, "{i}-{chunk}").unwrap();
        }
        std::fs::write(dir.join("MEMORY.md"), &body).unwrap();

        let cfg = MemoryConfig::for_test(dir.clone());
        let p = load(&cfg).await.unwrap();
        let auto = p.auto.as_ref().expect("auto loaded");
        // Body kept ≤ 25 KB before we appended conventions.
        // We can't assert exact byte length post-conventions, but truncated_bytes
        // must be set and the kept body shouldn't contain every line.
        assert!(auto.truncated_bytes > 0);
        let lines_with_x = auto.body.lines().filter(|l| l.contains("xxxx")).count();
        assert!(
            lines_with_x < 10,
            "expected truncation, got {lines_with_x} lines"
        );
    }

    #[tokio::test]
    async fn html_comments_stripped_from_auto_splice() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("memory");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("MEMORY.md"),
            "# Memory index\n<!-- secret comment -->\n- [foo](foo.md) — user: visible\n",
        )
        .unwrap();
        let cfg = MemoryConfig::for_test(dir.clone());
        let p = load(&cfg).await.unwrap();
        let auto = p.auto.as_ref().unwrap();
        assert!(!auto.body.contains("secret comment"));
        assert!(auto.body.contains("[foo](foo.md)"));
    }

    // ----- env-var driven tests -----
    //
    // `std::env::set_var` / `remove_var` were marked `unsafe` in Rust 2024
    // because mutating the process environment is racy with other threads
    // (especially `getenv` in libc). The workspace lint denies `unsafe_code`
    // — we localize the `#[allow]` to the env-guard helper which is only
    // reachable from `#[cfg(test)]` and runs single-threaded under
    // `cargo test -p caliban-memory` (no other crate in the workspace mutates
    // these vars). The guard restores the previous value on drop so leakage
    // across tests is contained.
    //
    // SAFETY: see comment above. We accept the documented race in test-only
    // code in exchange for being able to assert the env-driven branches.
    #[allow(unsafe_code)]
    fn set_env(key: &str, value: Option<&str>) {
        match value {
            // SAFETY: see module-level comment above the env tests.
            Some(v) => unsafe { std::env::set_var(key, v) },
            // SAFETY: see module-level comment above the env tests.
            None => unsafe { std::env::remove_var(key) },
        }
    }

    struct EnvGuard {
        key: &'static str,
        prior: Option<std::ffi::OsString>,
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            set_env(self.key, self.prior.as_ref().and_then(|s| s.to_str()));
        }
    }

    fn env_guard(key: &'static str) -> EnvGuard {
        EnvGuard {
            key,
            prior: std::env::var_os(key),
        }
    }

    #[tokio::test]
    async fn disable_env_skips_auto_tier() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("memory");
        // Pre-populate so we *would* load if the env weren't set.
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("MEMORY.md"), "# Memory index\n").unwrap();

        let _guard = env_guard("CALIBAN_DISABLE_AUTO_MEMORY");
        set_env("CALIBAN_DISABLE_AUTO_MEMORY", Some("1"));

        let cfg = MemoryConfig::for_test(dir.clone());
        let p = load(&cfg).await.unwrap();
        assert!(p.auto.is_none(), "auto tier should be dropped");
    }

    #[test]
    fn config_honors_auto_memory_directory_override() {
        let _g1 = env_guard("CALIBAN_AUTO_MEMORY_DIRECTORY");
        set_env(
            "CALIBAN_AUTO_MEMORY_DIRECTORY",
            Some("/tmp/custom-auto-mem-xyz"),
        );
        let cfg = MemoryConfig::from_env(std::path::Path::new("/tmp/whatever"));
        assert_eq!(
            cfg.auto_memory_dir,
            std::path::PathBuf::from("/tmp/custom-auto-mem-xyz")
        );
    }
}
