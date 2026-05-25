//! Narrow shell-style glob matching and the `first_arg` accessor for the
//! permissions engine.
//!
//! Lifted from `caliban-agent-core/src/permissions.rs` — re-exported there
//! for back-compat. Other consumers (sandbox glob bypass lists, MCP per-
//! server permission scoping) can call into this module directly.

/// Match `pattern` against `value`. Supports `*` (zero or more chars) and
/// `?` (exactly one char). Intentionally narrow; if requirements grow,
/// switch to the `globset` crate.
#[must_use]
pub fn matches_glob(pattern: &str, value: &str) -> bool {
    let pattern_bytes = pattern.as_bytes();
    let value_bytes = value.as_bytes();
    let mut p = 0_usize;
    let mut v = 0_usize;
    let mut star: Option<usize> = None;
    let mut star_v: usize = 0;

    while v < value_bytes.len() {
        if p < pattern_bytes.len()
            && (pattern_bytes[p] == b'?' || pattern_bytes[p] == value_bytes[v])
        {
            p += 1;
            v += 1;
        } else if p < pattern_bytes.len() && pattern_bytes[p] == b'*' {
            star = Some(p);
            star_v = v;
            p += 1;
        } else if let Some(s) = star {
            p = s + 1;
            star_v += 1;
            v = star_v;
        } else {
            return false;
        }
    }
    while p < pattern_bytes.len() && pattern_bytes[p] == b'*' {
        p += 1;
    }
    p == pattern_bytes.len()
}

/// Extract the "first arg" string for a tool input, per the permissions
/// design spec. Returns `None` when the tool has no first-arg accessor or
/// the JSON shape doesn't match.
#[must_use]
pub fn first_arg(tool_name: &str, input: &serde_json::Value) -> Option<String> {
    let key = match tool_name {
        "Bash" => "command",
        "WebFetch" => "url",
        "Read" | "Write" | "Edit" => "path",
        _ => return None,
    };
    input.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- matches_glob ---

    #[test]
    fn glob_star_matches_anything() {
        assert!(matches_glob("*", ""));
        assert!(matches_glob("*", "anything"));
    }

    #[test]
    fn glob_q_matches_one_char() {
        assert!(matches_glob("a?c", "abc"));
        assert!(!matches_glob("a?c", "abbc"));
    }

    #[test]
    fn glob_no_special_chars_is_literal() {
        assert!(matches_glob("hello", "hello"));
        assert!(!matches_glob("hello", "hella"));
    }

    #[test]
    fn glob_star_prefix() {
        assert!(matches_glob("git *", "git status"));
        assert!(!matches_glob("git *", "gitk"));
    }

    #[test]
    fn glob_rm_prefix_does_not_match_sudo_rm() {
        assert!(!matches_glob("rm *", "sudo rm -rf /"));
    }

    #[test]
    fn glob_double_star_collapses() {
        assert!(matches_glob("a**b", "axxxb"));
        assert!(matches_glob("a**b", "ab"));
    }

    // --- first_arg ---

    #[test]
    fn first_arg_bash_returns_command() {
        let v = serde_json::json!({"command": "ls -la"});
        assert_eq!(first_arg("Bash", &v).as_deref(), Some("ls -la"));
    }

    #[test]
    fn first_arg_webfetch_returns_url() {
        let v = serde_json::json!({"url": "https://x"});
        assert_eq!(first_arg("WebFetch", &v).as_deref(), Some("https://x"));
    }

    #[test]
    fn first_arg_read_returns_path() {
        let v = serde_json::json!({"path": "/a/b"});
        assert_eq!(first_arg("Read", &v).as_deref(), Some("/a/b"));
    }

    #[test]
    fn first_arg_unknown_tool_returns_none() {
        let v = serde_json::json!({"command": "x"});
        assert_eq!(first_arg("UnknownMcpTool", &v), None);
    }

    #[test]
    fn first_arg_missing_key_returns_none() {
        let v = serde_json::json!({});
        assert_eq!(first_arg("Bash", &v), None);
    }
}
