//! `MemoryPrefix` — assembled tier blocks + splice rendering.

use std::fmt::Write as _;
use std::path::PathBuf;

/// Where a [`TierFile`] originated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TierFileSource {
    /// Operator-global CLAUDE.md.
    Global,
    /// Discovered via the project-tier ancestor walk.
    Walk,
    /// Inlined via an `@`-import inside another tier file.
    Import,
    /// Added on-demand after the model touched a file in this subtree.
    Nested,
    /// Path-glob-matched rule from `.caliban/rules/`.
    Rule,
    /// Per-workspace auto-memory `MEMORY.md`.
    Auto,
    /// Legacy single-file project tier (regression escape).
    LegacyProject,
}

impl TierFileSource {
    /// Splice attribute value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Walk => "walk",
            Self::Import => "import",
            Self::Nested => "nested",
            Self::Rule => "rule",
            Self::Auto => "auto",
            Self::LegacyProject => "legacy",
        }
    }
}

/// One loaded tier file with provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TierFile {
    /// Absolute path of the file on disk.
    pub path: PathBuf,
    /// File contents (UTF-8 lossy; may have a truncation suffix when over-budget).
    pub body: String,
    /// Estimated tokens (`body.len() / 4`).
    pub estimated_tokens: usize,
    /// Bytes shed by budget truncation; `0` when the file fit.
    pub truncated_bytes: usize,
}

/// Tier identifiers used by the splice output and the `/memory` summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TierKind {
    /// Operator-global `CLAUDE.md` (XDG config).
    Global,
    /// Project `CLAUDE.md` at the workspace root.
    Project,
    /// Per-workspace auto-memory `MEMORY.md`.
    Auto,
}

impl TierKind {
    /// XML tag name written into the system-prompt prefix.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Global => "global-claude-md",
            Self::Project => "project-claude-md",
            Self::Auto => "auto-memory-index",
        }
    }

    /// Short label used by `/memory` summary lines.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project => "project",
            Self::Auto => "auto",
        }
    }
}

/// Rich project tier with all sub-collections preserved. Useful for the
/// `/memory` overlay and the ancestry-addendum subsystem.
#[derive(Debug, Clone, Default)]
pub struct ProjectTier {
    /// Files discovered by the ancestor walk (broad → narrow order).
    pub base_files: Vec<TierFile>,
    /// Imports resolved from any walk / rule / nested file — surfaced for
    /// provenance display; the bodies are already inlined in their owning
    /// tier file via `<!-- imported from … -->` markers.
    pub imports: Vec<TierFile>,
    /// Path-glob-matched rules (loaded lazily on first matching path touch).
    pub active_rules: Vec<TierFile>,
    /// Files added on-demand mid-session by `Read`/`Edit`/`Glob` hooks.
    pub nested: Vec<TierFile>,
}

impl ProjectTier {
    /// Total estimated tokens across every collection.
    #[must_use]
    pub fn estimated_tokens(&self) -> usize {
        self.base_files
            .iter()
            .map(|t| t.estimated_tokens)
            .sum::<usize>()
            + self
                .imports
                .iter()
                .map(|t| t.estimated_tokens)
                .sum::<usize>()
            + self
                .active_rules
                .iter()
                .map(|t| t.estimated_tokens)
                .sum::<usize>()
            + self
                .nested
                .iter()
                .map(|t| t.estimated_tokens)
                .sum::<usize>()
    }

    /// Concatenate all base files + always-active rules into a single body for
    /// the legacy `project: Option<TierFile>` slot used by `splice_into`.
    /// The first file's path is used as the slot's `path` for provenance.
    #[must_use]
    pub fn to_legacy_tier(&self) -> Option<TierFile> {
        if self.base_files.is_empty() && self.active_rules.is_empty() {
            return None;
        }
        let mut body = String::new();
        let mut tokens = 0usize;
        for f in &self.base_files {
            let _ = writeln!(
                body,
                "<project-claude-md path=\"{}\" source=\"walk\">",
                f.path.display(),
            );
            body.push_str(&f.body);
            if !body.ends_with('\n') {
                body.push('\n');
            }
            body.push_str("</project-claude-md>\n\n");
            tokens = tokens.saturating_add(f.estimated_tokens);
        }
        for r in &self.active_rules {
            let _ = writeln!(
                body,
                "<project-rule path=\"{}\" source=\"rule\">",
                r.path.display(),
            );
            body.push_str(&r.body);
            if !body.ends_with('\n') {
                body.push('\n');
            }
            body.push_str("</project-rule>\n\n");
            tokens = tokens.saturating_add(r.estimated_tokens);
        }
        let path = self
            .base_files
            .first()
            .or_else(|| self.active_rules.first())
            .map(|t| t.path.clone())
            .unwrap_or_default();
        Some(TierFile {
            path,
            body,
            estimated_tokens: tokens,
            truncated_bytes: 0,
        })
    }
}

/// Assembled memory prefix.
///
/// Tiers are present when the corresponding file existed and read successfully.
/// `estimated_tokens` is the *combined* token estimate across all present tiers.
#[derive(Debug, Clone, Default)]
pub struct MemoryPrefix {
    /// Operator-global `CLAUDE.md`, if any.
    pub global: Option<TierFile>,
    /// Workspace `CLAUDE.md`, if any. This is the **flattened** view of
    /// [`Self::project_tier`] used by `splice_into` for backward compat; the
    /// rich view is in `project_tier`.
    pub project: Option<TierFile>,
    /// Rich project-tier collections (walk + imports + rules + nested).
    pub project_tier: Option<ProjectTier>,
    /// Per-workspace auto-memory `MEMORY.md`, if any.
    pub auto: Option<TierFile>,
    /// Sum of `estimated_tokens` across present tiers.
    pub estimated_tokens: usize,
    /// `true` if any tier was truncated by budget enforcement.
    pub truncated: bool,
}

impl MemoryPrefix {
    /// Render the memory prefix and append the operator's default-body system
    /// prompt. Tier order is global → project → auto; missing tiers contribute
    /// zero bytes. When all tiers are missing, returns `default_body` as-is.
    #[must_use]
    pub fn splice_into(&self, default_body: &str) -> String {
        let mut out = String::new();
        for (kind, tier) in [
            (TierKind::Global, self.global.as_ref()),
            (TierKind::Project, self.project.as_ref()),
            (TierKind::Auto, self.auto.as_ref()),
        ] {
            let Some(tier) = tier else { continue };
            out.push('<');
            out.push_str(kind.tag());
            out.push_str(" path=\"");
            out.push_str(&tier.path.display().to_string());
            out.push_str("\">\n");
            out.push_str(&tier.body);
            if !tier.body.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("</");
            out.push_str(kind.tag());
            out.push_str(">\n\n");
        }
        out.push_str(default_body);
        out
    }

    /// Human-readable summary lines for the `/memory` slash command.
    #[must_use]
    pub fn summary_lines(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(6);
        for (kind, tier) in [
            (TierKind::Global, self.global.as_ref()),
            (TierKind::Project, self.project.as_ref()),
            (TierKind::Auto, self.auto.as_ref()),
        ] {
            match tier {
                Some(t) => out.push(format!(
                    "  {:<8} {} ({} tokens{})",
                    kind.label(),
                    t.path.display(),
                    t.estimated_tokens,
                    if t.truncated_bytes > 0 {
                        format!(", truncated {} bytes", t.truncated_bytes)
                    } else {
                        String::new()
                    },
                )),
                None => out.push(format!("  {:<8} (missing)", kind.label())),
            }
        }
        if let Some(pt) = self.project_tier.as_ref() {
            for f in &pt.base_files {
                out.push(format!(
                    "    walk     {} ({} tokens)",
                    f.path.display(),
                    f.estimated_tokens,
                ));
            }
            for f in &pt.imports {
                out.push(format!(
                    "    import   {} ({} tokens)",
                    f.path.display(),
                    f.estimated_tokens,
                ));
            }
            for f in &pt.active_rules {
                out.push(format!(
                    "    rule     {} ({} tokens)",
                    f.path.display(),
                    f.estimated_tokens,
                ));
            }
            for f in &pt.nested {
                out.push(format!(
                    "    nested   {} ({} tokens)",
                    f.path.display(),
                    f.estimated_tokens,
                ));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tier(label: TierKind, body: &str) -> TierFile {
        TierFile {
            path: PathBuf::from(format!("/tmp/{}.md", label.label())),
            estimated_tokens: body.len() / 4,
            body: body.to_string(),
            truncated_bytes: 0,
        }
    }

    fn raw_tier(path: &str, body: &str) -> TierFile {
        TierFile {
            path: PathBuf::from(path),
            estimated_tokens: body.len() / 4,
            body: body.to_string(),
            truncated_bytes: 0,
        }
    }

    #[test]
    fn splice_into_orders_tiers_correctly() {
        let p = MemoryPrefix {
            global: Some(tier(TierKind::Global, "GLOBAL")),
            project: Some(tier(TierKind::Project, "PROJECT")),
            auto: Some(tier(TierKind::Auto, "AUTO")),
            ..MemoryPrefix::default()
        };
        let out = p.splice_into("BODY");
        let g = out.find("GLOBAL").expect("global present");
        let pj = out.find("PROJECT").expect("project present");
        let a = out.find("AUTO").expect("auto present");
        let b = out.find("BODY").expect("body present");
        assert!(g < pj && pj < a && a < b, "wrong order: {out}");
    }

    #[test]
    fn splice_into_omits_missing_tiers() {
        let p = MemoryPrefix {
            global: None,
            project: Some(tier(TierKind::Project, "PROJECT")),
            auto: None,
            ..MemoryPrefix::default()
        };
        let out = p.splice_into("BODY");
        assert!(!out.contains("global-claude-md"));
        assert!(!out.contains("auto-memory-index"));
        assert!(out.contains("project-claude-md"));
        assert!(out.contains("BODY"));
    }

    #[test]
    fn splice_into_preserves_default_body() {
        let p = MemoryPrefix::default();
        let out = p.splice_into("the default body verbatim");
        assert_eq!(out, "the default body verbatim");
    }

    #[test]
    fn summary_lines_show_missing_tiers() {
        let p = MemoryPrefix {
            global: None,
            project: Some(tier(TierKind::Project, "x")),
            auto: None,
            ..MemoryPrefix::default()
        };
        let lines = p.summary_lines();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("(missing)"));
        assert!(lines[1].contains("project"));
        assert!(lines[2].contains("(missing)"));
    }

    #[test]
    fn project_tier_flattens_walk_and_rules_into_legacy_tier() {
        let pt = ProjectTier {
            base_files: vec![
                raw_tier("/tmp/root/CLAUDE.md", "ROOT-BODY"),
                raw_tier("/tmp/root/sub/CLAUDE.md", "SUB-BODY"),
            ],
            active_rules: vec![raw_tier("/tmp/root/.caliban/rules/x.md", "RULE-BODY")],
            ..ProjectTier::default()
        };
        let flat = pt.to_legacy_tier().expect("flat tier built");
        assert!(flat.body.contains("ROOT-BODY"));
        assert!(flat.body.contains("SUB-BODY"));
        assert!(flat.body.contains("RULE-BODY"));
        assert!(flat.body.contains("project-claude-md"));
        assert!(flat.body.contains("project-rule"));
        // ROOT should come before SUB (broad → narrow).
        assert!(flat.body.find("ROOT-BODY").unwrap() < flat.body.find("SUB-BODY").unwrap(),);
    }

    #[test]
    fn project_tier_empty_returns_none_legacy_tier() {
        let pt = ProjectTier::default();
        assert!(pt.to_legacy_tier().is_none());
    }
}
