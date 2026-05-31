//! v2 pattern matcher: `*`, `?`, `**`, `~glob` anywhere-match for Bash,
//! dotted-key MCP arg accessors, and workspace-normalized paths for
//! file-edit tools.

use crate::hooks::ToolCtx;

/// Match `pattern` against `ctx` using the workspace root inferred from `git`.
/// See [`matches_with_workspace`] for the full pattern grammar.
pub fn matches(pattern: &str, ctx: &ToolCtx<'_>) -> bool {
    matches_with_workspace(pattern, ctx, &workspace_root())
}

/// Return the current workspace root by asking `git rev-parse --show-toplevel`.
/// Falls back to the current working directory if git is unavailable or fails.
pub fn workspace_root() -> std::path::PathBuf {
    // Best-effort: ask git for the toplevel; fall back to cwd.
    if let Ok(out) = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        && out.status.success()
    {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !s.is_empty() {
            return std::path::PathBuf::from(s);
        }
    }
    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
}

fn split_pattern(pattern: &str) -> (&str, Option<&str>) {
    pattern
        .split_once(':')
        .map_or((pattern, None), |(name, spec)| (name, Some(spec)))
}

fn is_file_edit_tool(name: &str) -> bool {
    matches!(
        name,
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit"
    )
}

fn glob_match(pat: &str, hay: &str) -> bool {
    // Uniform glob via `globset` with literal_separator=false so `*` and `**`
    // both behave intuitively for non-path inputs (URLs, commands).
    let g = globset::GlobBuilder::new(pat)
        .literal_separator(false)
        .build();
    match g {
        Ok(g) => g.compile_matcher().is_match(hay),
        Err(_) => false, // bad pattern => never match (loud at config time)
    }
}

fn glob_match_path(pat: &str, hay: &std::path::Path) -> bool {
    let g = globset::GlobBuilder::new(pat)
        .literal_separator(true) // for path globs, `*` doesn't cross `/`
        .build();
    match g {
        Ok(g) => g.compile_matcher().is_match(hay),
        Err(_) => false,
    }
}

/// Match `pattern` against `ctx`, treating `workspace` as the repo root for
/// path normalization. Exported for testing and `caliban perms test/explain`.
///
/// # Pattern grammar
///
/// - `Tool` — match any invocation of `Tool`.
/// - `Tool:<glob>` — glob the tool's first arg (`*`, `?`, `**`).
/// - `Bash:~<glob>` — match anywhere in the bash command (sliding-window).
/// - `Tool:key=<glob>` / `Tool:k1.k2=<glob>` — dotted-key accessor; comma-separated pairs are AND-combined.
/// - `*` — catch-all.
///
/// For file-edit tools (`Read`, `Write`, `Edit`, `MultiEdit`, `NotebookEdit`) the file path
/// is workspace-normalized and relative patterns implicitly anchor with `**/`.
pub fn matches_with_workspace(
    pattern: &str,
    ctx: &ToolCtx<'_>,
    workspace: &std::path::Path,
) -> bool {
    let (tool_pat, arg_pat) = split_pattern(pattern);
    if tool_pat != "*" && !glob_match(tool_pat, ctx.tool_name) {
        return false;
    }
    let Some(spec) = arg_pat else {
        return true;
    };

    // ~glob: match anywhere in the Bash command line.
    if let Some(rest) = spec.strip_prefix('~') {
        if ctx.tool_name != "Bash" {
            return false;
        }
        let cmd = ctx
            .input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        return contains_glob(rest, cmd);
    }

    // dotted-key=value pairs: AND-combined.
    if spec.contains('=') {
        return spec.split(',').all(|kv| kv_match(kv, ctx.input));
    }

    // Path globs for file-edit tools — workspace-normalize both sides.
    if is_file_edit_tool(ctx.tool_name) {
        let raw = ctx
            .input
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let target = workspace_normalize(raw, workspace);
        let spec_path = std::path::Path::new(spec);
        // If the pattern is absolute, match directly.
        // If relative, prepend `**/` so `src/**/*.rs` matches at any depth
        // in the repo (e.g. `/repo/crates/x/src/y.rs`).
        let glob_pat: String = if spec_path.is_absolute() {
            spec.to_owned()
        } else {
            // Strip a leading `./` first, then anchor with `**/`.
            let stripped = spec.strip_prefix("./").unwrap_or(spec);
            format!("**/{stripped}")
        };
        return glob_match_path(&glob_pat, &target);
    }

    // Default: glob over the first-arg string of known tools.
    let first = first_arg(ctx).unwrap_or_default();
    glob_match(spec, &first)
}

fn first_arg(ctx: &ToolCtx<'_>) -> Option<String> {
    let key = match ctx.tool_name {
        "Bash" => "command",
        "WebFetch" => "url",
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => "file_path",
        _ => return None,
    };
    ctx.input.get(key)?.as_str().map(str::to_owned)
}

fn contains_glob(pat: &str, hay: &str) -> bool {
    // Sliding-window glob match. Cheap because hay is short (a shell line).
    for i in 0..=hay.len() {
        for j in i..=hay.len() {
            if !hay.is_char_boundary(i) || !hay.is_char_boundary(j) {
                continue;
            }
            if glob_match(pat, &hay[i..j]) {
                return true;
            }
        }
    }
    false
}

fn kv_match(kv: &str, input: &serde_json::Value) -> bool {
    let Some((key, glob)) = kv.split_once('=') else {
        return false;
    };
    let mut cursor = input;
    for part in key.split('.') {
        match cursor.get(part) {
            Some(next) => cursor = next,
            None => return glob_match(glob, ""), // missing key → empty
        }
    }
    let val = cursor.as_str().unwrap_or("");
    glob_match(glob, val)
}

fn workspace_normalize(p: &str, workspace: &std::path::Path) -> std::path::PathBuf {
    let path = std::path::Path::new(p);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    let stripped: &std::path::Path = path.strip_prefix("./").unwrap_or(path);
    workspace.join(stripped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx<'a>(name: &'a str, input: &'a serde_json::Value) -> ToolCtx<'a> {
        ToolCtx {
            turn_index: 0,
            tool_use_id: "t",
            tool_name: name,
            input,
        }
    }

    #[test]
    fn globstar_path_matches_nested_rs_file() {
        let ws = std::path::Path::new("/repo");
        let i = json!({"file_path": "/repo/crates/x/src/y.rs"});
        assert!(
            matches_with_workspace("Edit:src/**/*.rs", &ctx("Edit", &i), ws),
            "globstar should match nested .rs under the workspace src tree"
        );
    }

    #[test]
    fn path_normalization_handles_relative_pattern() {
        let ws = std::path::Path::new("/repo");
        let i = json!({"file_path": "/repo/foo.rs"});
        assert!(matches_with_workspace(
            "Edit:./foo.rs",
            &ctx("Edit", &i),
            ws
        ));
        assert!(matches_with_workspace("Edit:foo.rs", &ctx("Edit", &i), ws));
    }

    #[test]
    fn bash_anywhere_catches_sudo() {
        let i = json!({"command": "sudo rm -rf /"});
        assert!(matches_with_workspace(
            "Bash:~rm *",
            &ctx("Bash", &i),
            std::path::Path::new("/")
        ));
    }

    #[test]
    fn bash_anywhere_only_for_bash() {
        let i = json!({"file_path": "rm"});
        // ~glob on Read is not allowed; should return false (NOT match).
        assert!(!matches_with_workspace(
            "Read:~rm",
            &ctx("Read", &i),
            std::path::Path::new("/")
        ));
    }

    #[test]
    fn mcp_dotted_key_matches() {
        let i = json!({"repo": "anthropic/caliban", "title": "feat"});
        assert!(matches_with_workspace(
            "mcp__github__create_issue:repo=anthropic/*",
            &ctx("mcp__github__create_issue", &i),
            std::path::Path::new("/")
        ));
    }

    #[test]
    fn mcp_multi_kv_all_must_match() {
        let i = json!({"repo": "anthropic/caliban", "title": "feat"});
        assert!(matches_with_workspace(
            "mcp__github__create_issue:repo=anthropic/*,title=feat*",
            &ctx("mcp__github__create_issue", &i),
            std::path::Path::new("/")
        ));
        assert!(!matches_with_workspace(
            "mcp__github__create_issue:repo=anthropic/*,title=docs*",
            &ctx("mcp__github__create_issue", &i),
            std::path::Path::new("/")
        ));
    }

    #[test]
    fn first_arg_fallback_preserved() {
        let i = json!({"command": "git push"});
        assert!(matches_with_workspace(
            "Bash:git *",
            &ctx("Bash", &i),
            std::path::Path::new("/")
        ));
        assert!(!matches_with_workspace(
            "Bash:git *",
            &ctx("Bash", &json!({"command": "gitk"})),
            std::path::Path::new("/")
        ));
    }

    #[test]
    fn star_matches_unknown_mcp_tool() {
        let i = json!({});
        assert!(matches_with_workspace(
            "*",
            &ctx("mcp__weird__tool", &i),
            std::path::Path::new("/")
        ));
    }
}
