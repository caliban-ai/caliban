//! Async tier loader + budget enforcement.

use std::path::{Path, PathBuf};

use crate::config::MemoryConfig;
use crate::error::{MemoryError, Result};
use crate::prefix::{MemoryPrefix, TierFile, TierKind};

/// Cap per-file disk read at 256 KB so a runaway memory file cannot wedge the
/// startup path.
const MAX_FILE_BYTES: usize = 256 * 1024;

/// Approximate-token estimator (chars / 4). Provider-agnostic, deterministic.
#[must_use]
pub fn estimate_tokens(body: &str) -> usize {
    body.chars().count() / 4
}

/// Seed file written into a freshly created auto-memory directory on first run.
const SEED_MEMORY_MD: &str =
    "# Memory index\n\n_No memories yet. Add entries below as `- [title](slug.md) — one-line summary`._\n";

/// Conventions block appended to MEMORY.md (in-memory only) on every load so
/// the agent always sees the writing rules without the operator maintaining them.
const CONVENTIONS_BLOCK: &str = concat!(
    "\n<!-- caliban: auto-memory conventions follow; do not delete -->\n",
    "Write to this index when you learn something durable about the user, project, or environment. ",
    "One topic per file, slug in kebab-case. Do not save transient task state, debug traces, or ",
    "facts already in the repo. Keep this file ≤ 200 lines.\n",
);

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
    // Seed the auto-memory dir if it doesn't exist yet.
    let auto_md = ensure_auto_memory(&config.auto_memory_dir).await?;

    let global = read_optional(config.global_path.as_deref()).await?;
    let project = read_optional(config.project_path.as_deref()).await?;
    let auto_raw = read_optional(Some(&auto_md)).await?;

    // Inject conventions into the auto-memory body (in-memory only).
    let auto = auto_raw.map(|mut t| {
        if !t.body.contains("caliban: auto-memory conventions follow") {
            if !t.body.ends_with('\n') {
                t.body.push('\n');
            }
            t.body.push_str(CONVENTIONS_BLOCK);
        }
        t.estimated_tokens = estimate_tokens(&t.body);
        t
    });

    let mut prefix = MemoryPrefix {
        global,
        project,
        auto,
        estimated_tokens: 0,
        truncated: false,
    };

    enforce_budget(&mut prefix, config.max_tokens);
    prefix.estimated_tokens = prefix
        .global
        .as_ref()
        .map_or(0, |t| t.estimated_tokens)
        + prefix.project.as_ref().map_or(0, |t| t.estimated_tokens)
        + prefix.auto.as_ref().map_or(0, |t| t.estimated_tokens);

    Ok(prefix)
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
            target: "caliban::memory",
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
    new_body.push_str(&format!(
        "\n[truncated: {shed} bytes over budget; raise CALIBAN_MEMORY_BUDGET_TOKENS or trim]\n",
    ));
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
            estimated_tokens: 0,
            truncated: false,
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
            estimated_tokens: 0,
            truncated: false,
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
        let body = (0..100)
            .map(|i| format!("line {i:03}\n"))
            .collect::<String>();
        let mut p = MemoryPrefix {
            global: None,
            project: None,
            auto: Some(tier(&body)),
            estimated_tokens: 0,
            truncated: false,
        };
        enforce_budget(&mut p, 20);
        let cut_body = &p.auto.as_ref().unwrap().body;
        // Snipped result must end on a newline (or the marker we appended).
        // Walk lines and ensure every kept body line is intact.
        for line in cut_body.lines().take_while(|l| l.starts_with("line ")) {
            assert!(
                line.len() == "line NNN".len(),
                "non-boundary cut: {line:?}"
            );
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
            estimated_tokens: 0,
            truncated: false,
        };
        enforce_budget(&mut p, 500);
        assert!(p.truncated);
        assert!(p.global.unwrap().truncated_bytes > 0);
    }

    #[test]
    fn other_tokens_excludes_correct_tier() {
        let p = MemoryPrefix {
            global: Some(tier(&"a".repeat(40))),   // 10 tokens
            project: Some(tier(&"b".repeat(80))),  // 20 tokens
            auto: Some(tier(&"c".repeat(120))),    // 30 tokens
            estimated_tokens: 0,
            truncated: false,
        };
        assert_eq!(other_tokens(&p, TierKind::Auto), 30);
        assert_eq!(other_tokens(&p, TierKind::Project), 40);
        assert_eq!(other_tokens(&p, TierKind::Global), 50);
    }
}
