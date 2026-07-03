//! Guard (ADR 0050): no code outside `caliban-common::paths` may call the bare
//! OS-native base-dir functions. On macOS those resolve to
//! `~/Library/Application Support`, which is the GUI-app store we deliberately
//! do not use — all caliban config/data/cache/state must flow through the
//! XDG-first helpers in `paths.rs` (`platform_config_dir`, `platform_data_dir`,
//! `platform_state_dir`, `platform_cache_dir`).
//!
//! `dirs::home_dir()` is allowed everywhere — it is the base the XDG helpers
//! themselves build on.

use std::fs;
use std::path::{Path, PathBuf};

/// Forbidden call fragments (matched after stripping `//` comments per line).
const FORBIDDEN: &[&str] = &[
    "dirs::config_dir(",
    "dirs::data_dir(",
    "dirs::data_local_dir(",
    "dirs::state_dir(",
    "dirs::cache_dir(",
    "ProjectDirs",
];

/// Workspace root = two levels up from this crate's manifest dir
/// (`<root>/crates/caliban-common`).
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
            // Strip line comments so doc/`//` references don't trip the guard.
            let code = line.split("//").next().unwrap_or("");
            for frag in FORBIDDEN {
                if code.contains(frag) {
                    violations.push(format!(
                        "{}:{}: `{}` — route through caliban_common::paths instead",
                        file.strip_prefix(&root).unwrap_or(&file).display(),
                        i + 1,
                        frag,
                    ));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "bare OS-native base-dir calls found (ADR 0050 — use caliban_common::paths):\n{}",
        violations.join("\n"),
    );
}
