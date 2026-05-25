//! `${CALIBAN_PLUGIN_ROOT}` (+ `${CLAUDE_PLUGIN_ROOT}` alias) expansion.
//!
//! Other `${VAR}` references are passed through unchanged — downstream
//! loaders (MCP client, hooks) own their own env-var expansion rules.

use std::path::Path;

/// Recognized aliases for the plugin root variable.
pub const PLUGIN_ROOT_VARS: &[&str] = &["CALIBAN_PLUGIN_ROOT", "CLAUDE_PLUGIN_ROOT"];

/// Replace every occurrence of `${CALIBAN_PLUGIN_ROOT}` and the
/// `${CLAUDE_PLUGIN_ROOT}` alias in `s` with the plugin's absolute path.
/// Other `${VAR}` references are passed through untouched.
#[must_use]
pub fn expand(s: &str, plugin_root: &Path) -> String {
    let root_str = plugin_root.to_string_lossy();
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        if let Some(end) = after.find('}') {
            let var = &after[..end];
            if PLUGIN_ROOT_VARS.contains(&var) {
                out.push_str(&root_str);
            } else {
                // Pass through the literal `${VAR}` for downstream expansion.
                out.push_str("${");
                out.push_str(var);
                out.push('}');
            }
            rest = &after[end + 1..];
        } else {
            // No closing brace — copy the rest literally and stop.
            out.push_str("${");
            out.push_str(after);
            rest = "";
            break;
        }
    }
    out.push_str(rest);
    out
}

/// In-place expand every string in a `serde_json::Value` tree (objects,
/// arrays, and string scalars). Numbers and booleans are left alone.
/// Useful for stamping hook config / mcp config snippets.
pub fn expand_json_in_place(v: &mut serde_json::Value, plugin_root: &Path) {
    match v {
        serde_json::Value::String(s) => {
            let new = expand(s, plugin_root);
            *s = new;
        }
        serde_json::Value::Array(arr) => {
            for child in arr.iter_mut() {
                expand_json_in_place(child, plugin_root);
            }
        }
        serde_json::Value::Object(map) => {
            for (_, val) in map.iter_mut() {
                expand_json_in_place(val, plugin_root);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn expands_caliban_plugin_root() {
        let s = "${CALIBAN_PLUGIN_ROOT}/bin/x";
        let out = expand(s, Path::new("/p/demo"));
        assert_eq!(out, "/p/demo/bin/x");
    }

    #[test]
    fn expands_claude_plugin_root_alias() {
        let s = "${CLAUDE_PLUGIN_ROOT}/bin/x";
        let out = expand(s, Path::new("/p/demo"));
        assert_eq!(out, "/p/demo/bin/x");
    }

    #[test]
    fn passes_through_unrelated_vars() {
        let s = "${HOME}/keys/${CALIBAN_PLUGIN_ROOT}/bin";
        let out = expand(s, Path::new("/p/demo"));
        assert_eq!(out, "${HOME}/keys//p/demo/bin");
    }

    #[test]
    fn no_braces_returns_input() {
        let s = "no vars here";
        assert_eq!(expand(s, Path::new("/p/demo")), s);
    }

    #[test]
    fn unclosed_brace_passes_through() {
        let s = "broken ${UNCLOSED";
        let out = expand(s, Path::new("/p/demo"));
        assert_eq!(out, "broken ${UNCLOSED");
    }

    #[test]
    fn expands_nested_json_strings() {
        let mut v: serde_json::Value = serde_json::json!({
            "command": "${CALIBAN_PLUGIN_ROOT}/bin/srv",
            "args": ["--root", "${CLAUDE_PLUGIN_ROOT}"],
            "nested": { "path": "${CALIBAN_PLUGIN_ROOT}/sub" }
        });
        expand_json_in_place(&mut v, Path::new("/p/demo"));
        assert_eq!(v["command"], "/p/demo/bin/srv");
        assert_eq!(v["args"][1], "/p/demo");
        assert_eq!(v["nested"]["path"], "/p/demo/sub");
    }
}
