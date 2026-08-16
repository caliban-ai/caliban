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

/// Split a rule pattern into `(tool glob, optional arg spec)`.
///
/// Both documented spellings are accepted and mean exactly the same thing
/// (#518): `Tool(<glob>)` — the form the non-interactive denial message and
/// the README tell operators to paste — is normalized to `Tool:<glob>` before
/// the colon split, so a pasted `--allow 'Bash(git *)'` actually matches.
///
/// The glob is everything between the *first* `(` and the *trailing* `)`, so a
/// legitimate `)` or `:` inside the glob survives (`Bash(echo (hi))`,
/// `Bash(scp host:/tmp/*)`). A `:` occurring *before* the first `(` means the
/// operator used the colon form, so the parens are literal glob text and are
/// left alone. Anything else — an unclosed `Tool(<glob>` in particular — is
/// left verbatim so it fails closed rather than widening into an allow rule;
/// [`validate_pattern`] is what makes such a typo visible.
pub(crate) fn split_pattern(pattern: &str) -> (&str, Option<&str>) {
    if let Some(open) = paren_open(pattern)
        && let Some(inner) = pattern[open + 1..].strip_suffix(')')
    {
        return (&pattern[..open], Some(inner));
    }
    pattern
        .split_once(':')
        .map_or((pattern, None), |(name, spec)| (name, Some(spec)))
}

/// Report why `pattern` can never match, or `None` when it is well-formed.
///
/// `glob_match` maps an uncompilable glob to `false`, so a typo in a rule
/// fails *closed and silently* — the operator sees a denial with no hint that
/// their rule is the problem. Callers that load rules (startup, `caliban
/// perms`) use this to say so out loud (#518).
///
/// Deliberately conservative: it only reports patterns that cannot match any
/// input, never stylistic complaints, so wiring it into startup can't spam
/// operators about rules that work.
#[must_use]
pub fn validate_pattern(pattern: &str) -> Option<String> {
    // An unclosed `Tool(<glob>` is almost certainly a typo of the paren form:
    // it degrades into a literal tool name that no tool can ever be called.
    if paren_open(pattern).is_some() && !pattern.ends_with(')') {
        return Some(format!(
            "`{pattern}` has an unclosed `(` — write `Tool(<glob>)` or `Tool:<glob>`"
        ));
    }
    let (tool_pat, spec) = split_pattern(pattern);
    if tool_pat.is_empty() {
        return Some(format!(
            "`{pattern}` names no tool — write `Tool(<glob>)`, `Tool:<glob>`, or `*`"
        ));
    }
    if tool_pat != "*"
        && let Err(e) = compile_glob(tool_pat, false)
    {
        return Some(format!(
            "`{pattern}` has an invalid tool glob `{tool_pat}`: {e}"
        ));
    }
    let spec = spec?;
    if spec.is_empty() {
        return Some(format!(
            "`{pattern}` has an empty glob — it matches nothing; use bare `{tool_pat}` to match every call"
        ));
    }
    // `key=<glob>` pairs validate each glob; `~<glob>` and plain globs validate
    // the one glob they carry.
    let globs: Vec<&str> = if spec.contains('=') {
        spec.split(',')
            .map(|kv| kv.split_once('=').map_or(kv, |(_, g)| g))
            .collect()
    } else {
        vec![spec.strip_prefix('~').unwrap_or(spec)]
    };
    for g in globs {
        if let Err(e) = compile_glob(g, false) {
            return Some(format!("`{pattern}` has an invalid glob `{g}`: {e}"));
        }
    }
    None
}

/// Byte index of the `(` that opens a `Tool(<glob>)` spec, or `None` when the
/// pattern isn't in that grammar. A `:` before the `(` means the operator used
/// the colon form and the parens are literal glob text.
fn paren_open(pattern: &str) -> Option<usize> {
    let open = pattern.find('(')?;
    match pattern.find(':') {
        Some(colon) if colon < open => None,
        _ => Some(open),
    }
}

fn compile_glob(pat: &str, literal_separator: bool) -> Result<globset::Glob, globset::Error> {
    globset::GlobBuilder::new(pat)
        .literal_separator(literal_separator)
        .build()
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
/// `Tool(<glob>)` and `Tool:<glob>` are interchangeable spellings of the same
/// rule — pick either (#518).
///
/// - `Tool` — match any invocation of `Tool`.
/// - `Tool:<glob>` / `Tool(<glob>)` — glob the tool's first arg (`*`, `?`, `**`).
/// - `Bash:~<glob>` / `Bash(~<glob>)` — match anywhere in the bash command (sliding-window).
/// - `Tool:key=<glob>` / `Tool(k1.k2=<glob>)` — dotted-key accessor; comma-separated pairs are AND-combined.
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
        // File-edit tools deserialize as `{"path": "..."}`; the canonical
        // accessor lives in `caliban_common` so all permission codepaths
        // agree on the key (previously this site looked up "file_path",
        // which silently produced an empty target and made every
        // `Tool:<glob>` rule a no-op for MultiEdit/NotebookEdit).
        let raw = first_arg(ctx).unwrap_or_default();
        // `workspace_normalize` lexically resolves `.`/`..` so a traversal in
        // the input can't slip past the anchored glob below (#216).
        let target = workspace_normalize(&raw, workspace);
        let spec_path = std::path::Path::new(spec);
        if spec_path.is_absolute() {
            // Absolute pattern: match directly against the normalized target.
            return glob_match_path(spec, &target);
        }
        // Relative (workspace-scoped) pattern. The normalized target must stay
        // *inside* the workspace — a `..` escape (e.g. `../../etc/passwd`) that
        // resolves outside it must never match a workspace-scoped rule (#216,
        // gap in #177). #177 only blocked a same-named subtree elsewhere; it
        // didn't resolve `..`, so `<ws>/../../etc/passwd` still matched a
        // `<ws>/**`-anchored glob.
        if !target.starts_with(workspace) {
            return false;
        }
        // Strip a leading `./`, then anchor to the workspace root so the
        // pattern stays inside the repo. `<ws>/**/<stripped>` lets `src/**`
        // match at any depth within the workspace. Escape the workspace prefix
        // so a literal path containing glob metacharacters isn't reinterpreted.
        let stripped = spec.strip_prefix("./").unwrap_or(spec);
        let ws = globset::escape(&workspace.to_string_lossy());
        let glob_pat = format!("{ws}/**/{stripped}");
        return glob_match_path(&glob_pat, &target);
    }

    // Default: glob over the first-arg string of known tools.
    let first = first_arg(ctx).unwrap_or_default();
    glob_match(spec, &first)
}

/// Thin wrapper around the canonical [`caliban_common::glob_match::first_arg`]
/// so the matcher and the rest of caliban agree on the JSON key used to
/// extract a tool's first arg (e.g. `path` for file-edit tools).
fn first_arg(ctx: &ToolCtx<'_>) -> Option<String> {
    caliban_common::glob_match::first_arg(ctx.tool_name, ctx.input)
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
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        let stripped: &std::path::Path = path.strip_prefix("./").unwrap_or(path);
        workspace.join(stripped)
    };
    lexical_normalize(&joined)
}

/// Resolve `.` and `..` components purely lexically (no filesystem access, so
/// it works for paths that don't exist and never follows symlinks). A leading
/// `..` that can't be popped is preserved so the result still lies outside any
/// absolute prefix — that's what lets the caller detect a workspace escape
/// (#216).
fn lexical_normalize(p: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                // Pop a preceding normal component …
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                // … otherwise keep the `..` (root / leading `..` / `..` chain)
                // so the result still lies outside the prefix we joined onto —
                // that's what lets the caller detect a workspace escape (#216).
                _ => out.push(".."),
            },
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx<'a>(name: &'a str, input: &'a serde_json::Value) -> ToolCtx<'a> {
        ToolCtx {
            session_id: "test-session",
            turn_index: 0,
            tool_use_id: "t",
            tool_name: name,
            input,
            is_read_only: false,
        }
    }

    #[test]
    fn globstar_path_matches_nested_rs_file() {
        // File-edit tools deserialize their JSON input as `{"path": "..."}`
        // — see caliban-tools-builtin/src/fs/{edit,multi_edit,...}.rs. The
        // matcher must look up the same key.
        let ws = std::path::Path::new("/repo");
        let i = json!({"path": "/repo/crates/x/src/y.rs"});
        assert!(
            matches_with_workspace("Edit:src/**/*.rs", &ctx("Edit", &i), ws),
            "globstar should match nested .rs under the workspace src tree"
        );
    }

    #[test]
    fn relative_pattern_does_not_escape_workspace() {
        // Security (#177): a relative file-edit pattern must scope to the
        // workspace. It must NOT match a same-named subtree elsewhere on the
        // filesystem.
        let ws = std::path::Path::new("/repo");
        let outside = json!({"path": "/etc/src/evil.rs"});
        assert!(
            !matches_with_workspace("Edit:src/**/*.rs", &ctx("Edit", &outside), ws),
            "relative pattern must be workspace-scoped, must not match /etc/src/..."
        );
        let home = json!({"path": "/home/attacker/src/evil.rs"});
        assert!(
            !matches_with_workspace("Edit:src/**/*.rs", &ctx("Edit", &home), ws),
            "relative pattern must not match /home/attacker/src/..."
        );
        // Still matches inside the workspace, at any depth.
        let inside = json!({"path": "/repo/crates/x/src/y.rs"});
        assert!(
            matches_with_workspace("Edit:src/**/*.rs", &ctx("Edit", &inside), ws),
            "relative pattern must still match src/** anywhere under the workspace"
        );
    }

    #[test]
    fn dotdot_traversal_does_not_escape_workspace() {
        // Security (#216, gap in #177): a `..` traversal in the input path must
        // not let a workspace-scoped Allow match a file outside the workspace.
        let ws = std::path::Path::new("/repo");
        let escape = json!({"path": "../../../../etc/passwd"});
        assert!(
            !matches_with_workspace("Edit:**", &ctx("Edit", &escape), ws),
            "workspace-scoped Edit:** must not match a ../ traversal outside the workspace"
        );
        // An absolute-but-traversing input normalizes and is rejected too.
        let escape_abs = json!({"path": "/repo/../../etc/passwd"});
        assert!(
            !matches_with_workspace("Edit:**", &ctx("Edit", &escape_abs), ws),
            "a path that lexically escapes the workspace must not match a workspace-scoped rule"
        );
        // Sanity: an in-workspace relative path still matches.
        let inside = json!({"path": "src/main.rs"});
        assert!(
            matches_with_workspace("Edit:**", &ctx("Edit", &inside), ws),
            "in-workspace relative path still matches Edit:**"
        );
        // And `..` that stays inside the workspace is fine.
        let inside_dotdot = json!({"path": "crates/x/../y/z.rs"});
        assert!(
            matches_with_workspace("Edit:**", &ctx("Edit", &inside_dotdot), ws),
            "a `..` that resolves to a path still inside the workspace matches"
        );
    }

    #[test]
    fn path_normalization_handles_relative_pattern() {
        let ws = std::path::Path::new("/repo");
        let i = json!({"path": "/repo/foo.rs"});
        assert!(matches_with_workspace(
            "Edit:./foo.rs",
            &ctx("Edit", &i),
            ws
        ));
        assert!(matches_with_workspace("Edit:foo.rs", &ctx("Edit", &i), ws));
    }

    #[test]
    fn multi_edit_path_matches_workspace_glob() {
        // Regression: prior to the path-key fix, MultiEdit rules never
        // matched any input because the matcher looked up "file_path"
        // while the tool's input shape is `{"path": "...", "edits": [...]}`.
        let ws = std::path::Path::new("/repo");
        let i = json!({"path": "/repo/src/foo.rs", "edits": []});
        assert!(
            matches_with_workspace("MultiEdit:src/**/*.rs", &ctx("MultiEdit", &i), ws),
            "MultiEdit rule must match against the tool's `path` field"
        );
    }

    #[test]
    fn notebook_edit_path_matches_workspace_glob() {
        // Same regression as MultiEdit — NotebookEdit also uses `path`.
        let ws = std::path::Path::new("/repo");
        let i = json!({"path": "/repo/nb.ipynb", "cell_id": "x", "new_source": ""});
        assert!(matches_with_workspace(
            "NotebookEdit:**/*.ipynb",
            &ctx("NotebookEdit", &i),
            ws
        ));
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
        let i = json!({"path": "rm"});
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

    /// Regression (#518): the non-interactive denial message and `README.md`
    /// both tell operators to re-run with `--allow 'Bash(git *)'`. Before this
    /// fix `split_pattern` only split on `:`, so that string parsed as a tool
    /// literally named `Bash(git *)` and could never match — following the
    /// printed guidance verbatim still left the call denied.
    #[test]
    fn suggested_paren_rule_from_deny_message_allows_the_denied_call() {
        // The exact string the deny message / README tell users to paste.
        let pattern = "Bash(git *)";
        let i = json!({"command": "git push"});
        assert!(
            matches_with_workspace(pattern, &ctx("Bash", &i), std::path::Path::new("/")),
            "the `--allow '{pattern}'` rule we print must actually match `git push`"
        );
        // …and it must still be *narrow*: a non-git command stays unmatched.
        assert!(
            !matches_with_workspace(
                pattern,
                &ctx("Bash", &json!({"command": "rm -rf /"})),
                std::path::Path::new("/")
            ),
            "the paren form must not widen into a catch-all"
        );
    }

    /// Option A: both grammars are accepted and mean the same thing, and a
    /// bare `Tool` still matches any invocation.
    #[test]
    fn paren_and_colon_forms_are_equivalent_and_bare_tool_still_works() {
        let ws = std::path::Path::new("/");
        let git = json!({"command": "git push"});
        let rm = json!({"command": "rm -rf /"});
        for pat in ["Bash(git *)", "Bash:git *"] {
            assert!(
                matches_with_workspace(pat, &ctx("Bash", &git), ws),
                "{pat} should match `git push`"
            );
            assert!(
                !matches_with_workspace(pat, &ctx("Bash", &rm), ws),
                "{pat} should not match `rm -rf /`"
            );
        }
        // Bare tool name: no spec, matches every invocation.
        assert!(matches_with_workspace("Bash", &ctx("Bash", &rm), ws));
        assert!(!matches_with_workspace("Bash", &ctx("Read", &rm), ws));
    }

    /// The `~` anywhere-match form works inside parens too (`Bash(~rm)`), and
    /// stays Bash-only just like `Bash:~rm`.
    #[test]
    fn paren_form_supports_anywhere_match() {
        let ws = std::path::Path::new("/");
        assert!(matches_with_workspace(
            "Bash(~rm)",
            &ctx("Bash", &json!({"command": "sudo rm -rf /"})),
            ws
        ));
        assert!(!matches_with_workspace(
            "Bash(~rm)",
            &ctx("Bash", &json!({"command": "ls -la"})),
            ws
        ));
        // ~glob is Bash-only regardless of which grammar spelled it.
        assert!(!matches_with_workspace(
            "Read(~rm)",
            &ctx("Read", &json!({"path": "rm"})),
            ws
        ));
    }

    /// Path globs for file-edit tools keep their workspace anchoring in the
    /// paren form, including the `..`-escape rejection from #216.
    #[test]
    fn paren_form_supports_file_edit_path_globs() {
        let ws = std::path::Path::new("/repo");
        assert!(matches_with_workspace(
            "Edit(src/**/*.rs)",
            &ctx("Edit", &json!({"path": "/repo/crates/x/src/y.rs"})),
            ws
        ));
        assert!(
            !matches_with_workspace(
                "Edit(src/**/*.rs)",
                &ctx("Edit", &json!({"path": "/etc/src/evil.rs"})),
                ws
            ),
            "paren form must stay workspace-scoped like the colon form"
        );
        assert!(
            !matches_with_workspace(
                "Edit(**)",
                &ctx("Edit", &json!({"path": "../../../../etc/passwd"})),
                ws
            ),
            "a `..` escape must not match a workspace-scoped paren rule either"
        );
    }

    /// The structured `key=<glob>` accessor works in the paren form, including
    /// comma-separated AND-combined pairs.
    #[test]
    fn paren_form_supports_structured_key_specs() {
        let ws = std::path::Path::new("/");
        let i = json!({"repo": "anthropic/caliban", "title": "feat"});
        assert!(matches_with_workspace(
            "mcp__github__create_issue(repo=anthropic/*)",
            &ctx("mcp__github__create_issue", &i),
            ws
        ));
        assert!(matches_with_workspace(
            "mcp__github__create_issue(repo=anthropic/*,title=feat*)",
            &ctx("mcp__github__create_issue", &i),
            ws
        ));
        assert!(!matches_with_workspace(
            "mcp__github__create_issue(repo=anthropic/*,title=docs*)",
            &ctx("mcp__github__create_issue", &i),
            ws
        ));
    }

    /// The glob is everything between the *first* `(` and the *trailing* `)`,
    /// so a legitimate `)` or `:` inside the glob survives. Splitting on `:`
    /// first would mangle `Bash(echo a:b)` into a tool named `Bash(echo a`.
    #[test]
    fn paren_form_preserves_inner_parens_and_colons() {
        let ws = std::path::Path::new("/");
        assert!(matches_with_workspace(
            "Bash(echo (hi))",
            &ctx("Bash", &json!({"command": "echo (hi)"})),
            ws
        ));
        assert!(matches_with_workspace(
            "Bash(scp host:/tmp/*)",
            &ctx("Bash", &json!({"command": "scp host:/tmp/x"})),
            ws
        ));
    }

    /// A `:` *before* the first `(` means the operator wrote the colon form,
    /// so the parens are literal glob text and must not be re-parsed.
    #[test]
    fn colon_form_wins_when_the_colon_comes_first() {
        let ws = std::path::Path::new("/");
        assert!(matches_with_workspace(
            "Bash:echo (hi)",
            &ctx("Bash", &json!({"command": "echo (hi)"})),
            ws
        ));
    }

    /// Unbalanced parens are *not* silently reinterpreted — they stay a
    /// literal (never-matching) tool name, so the rule fails closed rather
    /// than widening. `validate_pattern` is what makes this visible.
    #[test]
    fn unbalanced_paren_fails_closed() {
        let ws = std::path::Path::new("/");
        assert!(
            !matches_with_workspace(
                "Bash(git *",
                &ctx("Bash", &json!({"command": "git push"})),
                ws
            ),
            "an unclosed paren must not be treated as an allow rule for Bash"
        );
        assert!(validate_pattern("Bash(git *").is_some());
    }

    /// `Tool()` is an *empty* glob, not a bare `Tool`. Treating it as bare
    /// would silently widen `Bash()` into "allow every shell command", so it
    /// fails closed and is reported by `validate_pattern`.
    #[test]
    fn empty_paren_spec_fails_closed_and_is_flagged() {
        let ws = std::path::Path::new("/");
        assert!(
            !matches_with_workspace("Bash()", &ctx("Bash", &json!({"command": "git push"})), ws),
            "`Bash()` must not behave like a bare `Bash` catch-all"
        );
        assert!(validate_pattern("Bash()").is_some());
    }

    /// `validate_pattern` is quiet for every grammar we actually document, so
    /// wiring it into startup can't spam operators with false positives.
    #[test]
    fn validate_pattern_accepts_documented_grammar() {
        for pat in [
            "*",
            "Bash",
            "Bash:git *",
            "Bash(git *)",
            "Bash:~rm",
            "Bash(~rm)",
            "Edit:src/**/*.rs",
            "Edit(src/**/*.rs)",
            "mcp__github__create_issue:repo=anthropic/*,title=feat*",
            "mcp__github__create_issue(repo=anthropic/*)",
            "mcp__*",
            "Bash(echo (hi))",
        ] {
            assert_eq!(validate_pattern(pat), None, "{pat} should be accepted");
        }
    }

    /// …but it does flag patterns that can never match, which today fail
    /// closed and silently (`glob_match` maps a bad glob to `false`).
    #[test]
    fn validate_pattern_flags_never_matching_patterns() {
        for pat in [
            "Bash(git *",
            "Bash()",
            "Bash:[unclosed",
            "Ba[d",
            "(git *)",
            ":git *",
        ] {
            assert!(
                validate_pattern(pat).is_some(),
                "{pat} can never match and should be reported"
            );
        }
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
