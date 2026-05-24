//! `/plugins` slash-overlay text renderer.
//!
//! Full interactive UI lands with ADR 0040 (slash-command registry).
//! v1 returns a flat list of lines that the TUI overlay renders verbatim.

use crate::cli::ListedPlugin;

/// Render the `/plugins` overlay body. Returns one display line per
/// installed plugin plus a header.
#[must_use]
pub fn render_overlay(rows: &[ListedPlugin]) -> Vec<String> {
    if rows.is_empty() {
        return vec![
            "No plugins installed.".to_string(),
            "Install one with `caliban plugin install <name>@<marketplace>` or".to_string(),
            "drop a directory under `.caliban/plugins/<name>/`.".to_string(),
        ];
    }
    let mut out = Vec::with_capacity(rows.len() + 1);
    out.push(format!("{} plugin(s) installed:", rows.len()));
    for r in rows {
        let glyph = if r.enabled { '\u{25cf}' } else { '\u{25cb}' };
        let status = if r.enabled { "" } else { "  DISABLED" };
        let summary = if r.summary.is_empty() {
            String::new()
        } else {
            format!("  {}", r.summary)
        };
        out.push(format!(
            "  {glyph} {name}  v{version}  ({source}){status}{summary}",
            name = r.name,
            version = r.version,
            source = r.source,
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_empty_overlay() {
        let lines = render_overlay(&[]);
        assert!(lines[0].contains("No plugins installed"));
    }

    #[test]
    fn renders_plugin_list() {
        let rows = vec![
            ListedPlugin {
                name: "demo".into(),
                version: "1.0.0".into(),
                source: "user".into(),
                enabled: true,
                summary: "2 skills".into(),
            },
            ListedPlugin {
                name: "off".into(),
                version: "0.1.0".into(),
                source: "user".into(),
                enabled: false,
                summary: String::new(),
            },
        ];
        let lines = render_overlay(&rows);
        assert!(lines[0].contains("2 plugin(s) installed"));
        let demo = lines.iter().find(|l| l.contains("demo")).unwrap();
        assert!(demo.contains("\u{25cf}"));
        assert!(demo.contains("v1.0.0"));
        assert!(demo.contains("2 skills"));
        let off = lines.iter().find(|l| l.contains("off")).unwrap();
        assert!(off.contains("\u{25cb}"));
        assert!(off.contains("DISABLED"));
    }

    #[test]
    fn renders_invalid_plugin_with_error() {
        let rows = vec![ListedPlugin {
            name: "broken".into(),
            version: "?".into(),
            source: "user".into(),
            enabled: false,
            summary: "invalid: missing name field".into(),
        }];
        let lines = render_overlay(&rows);
        let row = lines.iter().find(|l| l.contains("broken")).unwrap();
        assert!(row.contains("invalid"));
    }
}
