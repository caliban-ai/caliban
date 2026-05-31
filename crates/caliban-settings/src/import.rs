//! One-shot import of foreign or legacy permissions/settings config into
//! canonical caliban v2 TOML form.
//!
//! Supported source shapes:
//! - Claude Code `settings.json`: `{"permissions": {"allow": [...], "deny": [...], "ask": [...]}}`
//! - Codex `config.json`: same JSON shape (same top-level `permissions` key).
//! - Legacy caliban `permissions.toml`: `[[rule]]\ntool = "…"\naction = "…"` (old form).

use std::path::Path;

use crate::writer::toml_str;
use crate::{RuleSpec, write_toml_atomic};

/// Errors that can occur during import.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// IO failure reading source or writing destination.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// Parse failure (JSON or TOML decode error).
    #[error("parse error: {0}")]
    Parse(String),
    /// Source file shape was not recognised as any supported format.
    #[error("unrecognised shape — not a known JSON or TOML permissions/settings format")]
    Unrecognised,
}

/// Import permissions rules from a foreign config file at `src` and write the
/// result as canonical `[[permissions.rules]]` TOML to `dst`.
///
/// Recognised source shapes:
/// - Claude Code / Codex JSON with `permissions.allow / ask / deny` arrays.
/// - Legacy caliban TOML with `[[rule]]` table entries (old v1 shape).
///
/// Returns the number of rules imported.
///
/// # Errors
/// Returns [`ImportError`] on IO failure, unrecognised shape, or parse error.
pub fn import_permissions_to_toml(src: &Path, dst: &Path) -> Result<usize, ImportError> {
    use std::fmt::Write as _;
    let body = std::fs::read_to_string(src)?;
    let rules = parse_any_permissions(&body)?;
    let count = rules.len();
    let mut out = String::new();
    out.push_str("# Imported by `caliban perms import` on ");
    out.push_str(&chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string());
    out.push('\n');
    for r in &rules {
        out.push_str("\n[[permissions.rules]]\n");
        let _ = writeln!(out, "pattern = {}", toml_str(&r.pattern));
        let _ = writeln!(out, "action  = {}", toml_str(&r.action));
        if let Some(c) = &r.comment {
            let _ = writeln!(out, "comment = {}", toml_str(c));
        }
        if let Some(reason) = &r.reason {
            let _ = writeln!(out, "reason  = {}", toml_str(reason));
        }
    }
    write_toml_atomic(dst, &out)?;
    Ok(count)
}

/// Import a full settings JSON (Claude Code `settings.json`, Codex `config.json`,
/// or legacy caliban `settings.json`) into a canonical caliban `settings.toml` at
/// `dst`.
///
/// The source must be valid JSON that can deserialize into [`crate::Settings`].
/// Unknown top-level keys are captured in the `extra` flatten field and
/// round-trip through the TOML serializer.
///
/// # Errors
/// Returns [`ImportError`] on IO failure or JSON/TOML parse error.
pub fn import_settings_to_toml(src: &Path, dst: &Path) -> Result<(), ImportError> {
    let body = std::fs::read_to_string(src)?;
    let json: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| ImportError::Parse(e.to_string()))?;
    let settings: crate::Settings =
        serde_json::from_value(json).map_err(|e| ImportError::Parse(e.to_string()))?;
    let toml_body =
        toml::to_string_pretty(&settings).map_err(|e| ImportError::Parse(e.to_string()))?;
    write_toml_atomic(dst, &toml_body)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Internal parser
// ---------------------------------------------------------------------------

/// Attempt to parse `body` as one of the recognised permissions-source shapes.
/// Returns an ordered `Vec<RuleSpec>` on success.
fn parse_any_permissions(body: &str) -> Result<Vec<RuleSpec>, ImportError> {
    // --- Try TOML first: legacy caliban `[[rule]] tool=… action=…` shape ---
    if let Ok(v) = toml::from_str::<toml::Value>(body)
        && let Some(arr) = v.get("rule").and_then(|x| x.as_array())
    {
        let mut out = Vec::new();
        for entry in arr {
            let tool = entry.get("tool").and_then(|x| x.as_str()).unwrap_or("");
            let action = entry
                .get("action")
                .and_then(|x| x.as_str())
                .unwrap_or("ask");
            let comment = entry
                .get("comment")
                .and_then(|x| x.as_str())
                .map(str::to_owned);
            if !tool.is_empty() {
                out.push(RuleSpec {
                    pattern: tool.to_owned(),
                    action: action.to_owned(),
                    comment,
                    reason: None,
                    expires_at: None,
                    tool: None,
                });
            }
        }
        if !out.is_empty() {
            return Ok(out);
        }
    }

    // --- Try JSON: Claude Code / Codex `permissions.{allow,ask,deny}` shape ---
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body)
        && let Some(perms) = v.get("permissions")
    {
        let mut out = Vec::new();
        // Process deny first (highest priority semantics preserved in order).
        for (key, action_str) in [("deny", "deny"), ("ask", "ask"), ("allow", "allow")] {
            if let Some(arr) = perms.get(key).and_then(|x| x.as_array()) {
                for p in arr {
                    if let Some(s) = p.as_str() {
                        out.push(RuleSpec {
                            pattern: s.to_owned(),
                            action: action_str.to_owned(),
                            comment: None,
                            reason: None,
                            expires_at: None,
                            tool: None,
                        });
                    }
                }
            }
        }
        // If the `permissions` key exists but is empty, return empty (not Unrecognised).
        return Ok(out);
    }

    Err(ImportError::Unrecognised)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_claude_code_json_produces_v2_toml() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("settings.json");
        let dst = dir.path().join("permissions.toml");
        std::fs::write(
            &src,
            r#"{"permissions":{"allow":["Read","Bash:git *"],"deny":["Bash:rm *"]}}"#,
        )
        .unwrap();
        let n = import_permissions_to_toml(&src, &dst).unwrap();
        // deny first, then allow: 3 total
        assert_eq!(n, 3);
        let body = std::fs::read_to_string(&dst).unwrap();
        assert!(body.contains("[[permissions.rules]]"));
        assert!(body.contains(r#"pattern = "Read""#));
        assert!(body.contains(r#"action  = "deny""#));
    }

    #[test]
    fn import_legacy_caliban_toml_produces_v2_toml() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("permissions.toml");
        let dst = dir.path().join("permissions.v2.toml");
        std::fs::write(
            &src,
            r#"
[[rule]]
tool = "Bash:git *"
action = "allow"
"#,
        )
        .unwrap();
        let n = import_permissions_to_toml(&src, &dst).unwrap();
        assert_eq!(n, 1);
        let body = std::fs::read_to_string(&dst).unwrap();
        assert!(body.contains("[[permissions.rules]]"));
        assert!(body.contains(r#"pattern = "Bash:git *""#));
    }

    #[test]
    fn import_settings_from_claude_code_json_emits_toml() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("settings.json");
        let dst = dir.path().join("settings.toml");
        std::fs::write(
            &src,
            r#"{"model": "claude-opus-4-7", "permissions": {"allow": ["Read"]}}"#,
        )
        .unwrap();
        import_settings_to_toml(&src, &dst).unwrap();
        let content = std::fs::read_to_string(&dst).unwrap();
        let s: crate::Settings = toml::from_str(&content).unwrap();
        assert!(s.model.is_some(), "model should round-trip through TOML");
        assert!(
            s.permissions.allow.iter().any(|x| x == "Read"),
            "allow list should contain 'Read'"
        );
    }
}
