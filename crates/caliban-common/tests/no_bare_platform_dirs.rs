//! Guard (ADR 0050): no code outside `caliban-common::paths` may call the bare
//! OS-native base-dir functions. On macOS those resolve to
//! `~/Library/Application Support`, which is the GUI-app store we deliberately
//! do not use — all caliban config/data/cache/state must flow through the
//! XDG-first helpers in `paths.rs` (`platform_config_dir`, `platform_data_dir`,
//! `platform_state_dir`, `platform_cache_dir`).
//!
//! `dirs::home_dir()` is allowed everywhere — it is the base the XDG helpers
//! themselves build on.
//!
//! The guard covers three evasion routes (#337): qualified calls
//! (`dirs::config_local_dir(...)`, `directories::BaseDirs`), aliased imports
//! (`use dirs::config_dir as cfg;`), and `//` sequences inside string literals
//! that a naive comment-stripper would mistake for a comment.

use std::fs;
use std::path::{Path, PathBuf};

/// Fully-qualified call/type fragments that must never appear outside paths.rs.
const FORBIDDEN_CALLS: &[&str] = &[
    "dirs::config_dir(",
    "dirs::config_local_dir(",
    "dirs::data_dir(",
    "dirs::data_local_dir(",
    "dirs::state_dir(",
    "dirs::cache_dir(",
    "dirs::preference_dir(",
    "directories::BaseDirs",
    "directories::UserDirs",
    "ProjectDirs",
];

/// Bare item names that must not be imported from `dirs`/`directories` — an
/// aliased import (`use dirs::config_dir as cfg;`) otherwise defeats the
/// qualified-call guard. `home_dir` is intentionally absent: it is allowed.
const FORBIDDEN_IMPORTS: &[&str] = &[
    "config_dir",
    "config_local_dir",
    "data_dir",
    "data_local_dir",
    "state_dir",
    "cache_dir",
    "preference_dir",
    "BaseDirs",
    "UserDirs",
    "ProjectDirs",
];

fn is_ident_byte(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphanumeric()
}

/// True if `needle` occurs in `hay` on identifier boundaries, so `config_dir`
/// does not match inside `platform_config_dir`.
fn contains_ident(hay: &str, needle: &str) -> bool {
    let bytes = hay.as_bytes();
    let n = needle.len();
    hay.match_indices(needle).any(|(idx, _)| {
        let before_ok = idx == 0 || !is_ident_byte(bytes[idx - 1]);
        let end = idx + n;
        let after_ok = end >= bytes.len() || !is_ident_byte(bytes[end]);
        before_ok && after_ok
    })
}

// ─── scanning helpers (unit-tested below) ───────────────────────────────────

/// Drop a `//` line comment but keep a `//` inside a string literal (e.g. a URL
/// like `http://…`) — a naive `split("//")` would truncate real code there.
fn strip_line_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_str = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' if !(i > 0 && bytes[i - 1] == b'\\') => in_str = !in_str,
            b'/' if !in_str && bytes.get(i + 1) == Some(&b'/') => return &line[..i],
            _ => {}
        }
        i += 1;
    }
    line
}

/// If a `use` line imports a forbidden base-dir item from `dirs`/`directories`,
/// return the offending item name — catching aliased/grouped imports that the
/// qualified-call fragments miss.
fn forbidden_import(code: &str) -> Option<&'static str> {
    if !code.trim_start().starts_with("use ") {
        return None;
    }
    if !(code.contains("dirs::") || code.contains("directories::")) {
        return None;
    }
    FORBIDDEN_IMPORTS
        .iter()
        .copied()
        .find(|item| contains_ident(code, item))
}

// ─── the guard ──────────────────────────────────────────────────────────────

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root above crates/caliban-common")
        .to_path_buf()
}

fn is_exempt(path: &Path) -> bool {
    let s = path.to_string_lossy();
    // The canonical implementation lives here, and this guard names the
    // forbidden fragments as string literals.
    s.ends_with("caliban-common/src/paths.rs") || s.ends_with("no_bare_platform_dirs.rs")
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            collect_rs(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn no_bare_platform_dirs_outside_paths_helper() {
    let root = workspace_root();
    let mut files = Vec::new();
    collect_rs(&root.join("crates"), &mut files);
    collect_rs(&root.join("caliban").join("src"), &mut files);

    let mut violations = Vec::new();
    for file in files {
        if is_exempt(&file) {
            continue;
        }
        let Ok(body) = fs::read_to_string(&file) else {
            continue;
        };
        for (i, line) in body.lines().enumerate() {
            let code = strip_line_comment(line);
            let rel = file.strip_prefix(&root).unwrap_or(&file);
            for frag in FORBIDDEN_CALLS {
                if code.contains(frag) {
                    violations.push(format!(
                        "{}:{}: `{}` — route through caliban_common::paths instead",
                        rel.display(),
                        i + 1,
                        frag,
                    ));
                }
            }
            if let Some(item) = forbidden_import(code) {
                violations.push(format!(
                    "{}:{}: imports `{}` from dirs/directories — route through caliban_common::paths instead",
                    rel.display(),
                    i + 1,
                    item,
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "bare OS-native base-dir calls found (ADR 0050 — use caliban_common::paths):\n{}",
        violations.join("\n"),
    );
}

// ─── unit tests for the helpers (this file is exempt from the scan above) ────

#[test]
fn strip_comment_keeps_url_inside_string_literal() {
    // A `//` inside a string is not a comment.
    assert_eq!(
        strip_line_comment(r#"let u = "http://example/x"; call_it();"#),
        r#"let u = "http://example/x"; call_it();"#,
    );
    // A real trailing comment is still removed.
    assert_eq!(strip_line_comment("do_it(); // trailing"), "do_it(); ");
}

#[test]
fn forbidden_call_list_covers_config_local_and_preference() {
    for frag in ["dirs::config_local_dir(", "dirs::preference_dir("] {
        assert!(
            FORBIDDEN_CALLS.contains(&frag),
            "FORBIDDEN_CALLS should include {frag}",
        );
    }
    assert!("let p = dirs::config_local_dir();".contains("dirs::config_local_dir("));
}

#[test]
fn flags_aliased_and_grouped_imports() {
    assert!(forbidden_import("use dirs::config_dir as cfg;").is_some());
    assert!(forbidden_import("use directories::BaseDirs;").is_some());
    assert!(forbidden_import("use dirs::{home_dir, cache_dir};").is_some());
}

#[test]
fn allows_home_dir_and_platform_helpers() {
    assert!(forbidden_import("use dirs::home_dir;").is_none());
    assert!(forbidden_import("use caliban_common::paths::platform_config_dir;").is_none());
    // The XDG helper name must not trip the call guard either.
    assert!(
        !FORBIDDEN_CALLS
            .iter()
            .any(|f| "let p = platform_config_dir();".contains(f))
    );
}
