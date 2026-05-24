//! `MemoryPrefix` — assembled tier blocks + splice rendering.

use std::path::PathBuf;

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

/// Assembled memory prefix.
///
/// Tiers are present when the corresponding file existed and read successfully.
/// `estimated_tokens` is the *combined* token estimate across all present tiers.
#[derive(Debug, Clone, Default)]
pub struct MemoryPrefix {
    /// Operator-global `CLAUDE.md`, if any.
    pub global: Option<TierFile>,
    /// Workspace `CLAUDE.md`, if any.
    pub project: Option<TierFile>,
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
        let mut out = Vec::with_capacity(4);
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

    #[test]
    fn splice_into_orders_tiers_correctly() {
        let p = MemoryPrefix {
            global: Some(tier(TierKind::Global, "GLOBAL")),
            project: Some(tier(TierKind::Project, "PROJECT")),
            auto: Some(tier(TierKind::Auto, "AUTO")),
            estimated_tokens: 0,
            truncated: false,
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
            estimated_tokens: 0,
            truncated: false,
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
            estimated_tokens: 0,
            truncated: false,
        };
        let lines = p.summary_lines();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("(missing)"));
        assert!(lines[1].contains("project"));
        assert!(lines[2].contains("(missing)"));
    }
}
