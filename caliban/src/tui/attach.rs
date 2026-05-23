//! Resolve `@path` tokens to file attachments at submit time.

use std::path::{Path, PathBuf};

use crate::tui::completer::Candidate;

/// Split an `@<token>` (passed WITHOUT the leading `@`) into the directory
/// to enumerate and the name fragment to match against.
///
/// Resolution rules:
/// - `""`           => dir = `workspace_root`, name = ""
/// - `"foo"`        => dir = `workspace_root`, name = "foo"
/// - `"src/ma"`     => dir = `workspace_root.join("src/")`, name = "ma"
/// - `"/etc/h"`     => dir = "/etc/", name = "h"
/// - `"~/.config/f"` => dir = `home.join(".config/")`, name = "f"
/// - `"../sib/x"`   => dir = `cwd.join("../sib/")`, name = "x"
pub(crate) fn split_at_token(
    token: &str,
    workspace_root: &Path,
    cwd: &Path,
    home: Option<&Path>,
) -> (PathBuf, String) {
    let (dir_str, name) = match token.rfind('/') {
        Some(i) => (&token[..=i], token[i + 1..].to_string()),
        None => ("", token.to_string()),
    };

    let dir: PathBuf = if dir_str.is_empty() {
        workspace_root.to_path_buf()
    } else if let Some(rest) = dir_str.strip_prefix("~/") {
        match home {
            Some(h) => h.join(rest),
            None => workspace_root.join(dir_str),
        }
    } else if dir_str == "~/" || dir_str == "~" {
        home.map_or_else(|| workspace_root.to_path_buf(), Path::to_path_buf)
    } else if Path::new(dir_str).is_absolute() {
        PathBuf::from(dir_str)
    } else if dir_str.starts_with("./") || dir_str.starts_with("../") {
        cwd.join(dir_str)
    } else {
        workspace_root.join(dir_str)
    };

    (dir, name)
}

/// One directory's immediate children, gitignore-aware. Returns names only;
/// directories include a trailing `/`. Caps output at 500 entries; sorted
/// alphabetically with directories grouped naturally by the `/` suffix.
pub(crate) fn read_dir_candidates(dir: &Path, show_hidden: bool) -> Vec<Candidate> {
    use ignore::WalkBuilder;
    let mut out = Vec::new();
    let walker = WalkBuilder::new(dir)
        .max_depth(Some(1))
        .hidden(!show_hidden)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(false)
        .build();
    for entry in walker.flatten() {
        if entry.path() == dir {
            continue;
        }
        let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
        let name = entry.file_name().to_string_lossy();
        let display = if is_dir {
            format!("{name}/")
        } else {
            name.to_string()
        };
        let insert_str = display.clone();
        out.push(Candidate {
            display,
            insert: insert_str,
            score: 0,
        });
        if out.len() >= 500 {
            break;
        }
    }
    out.sort_by(|a, b| a.display.cmp(&b.display));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use tempfile::TempDir;

    fn touch(dir: &Path, rel: &str) {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::File::create(&p).unwrap().write_all(b"x").unwrap();
    }

    #[test]
    fn read_dir_lists_files_and_dirs() {
        let td = TempDir::new().unwrap();
        touch(td.path(), "alpha.txt");
        touch(td.path(), "beta.rs");
        fs::create_dir(td.path().join("sub")).unwrap();
        let cands = read_dir_candidates(td.path(), false);
        let displays: Vec<&str> = cands.iter().map(|c| c.display.as_str()).collect();
        assert!(displays.contains(&"alpha.txt"));
        assert!(displays.contains(&"beta.rs"));
        assert!(displays.contains(&"sub/"));
    }

    #[test]
    fn read_dir_respects_gitignore() {
        let td = TempDir::new().unwrap();
        // .git/ marker is what the `ignore` crate uses to treat the dir as
        // a git root so .gitignore is honored. Without it the file would
        // be ignored as a stray .gitignore.
        fs::create_dir(td.path().join(".git")).unwrap();
        fs::write(td.path().join(".gitignore"), "secret.txt\n").unwrap();
        touch(td.path(), "visible.txt");
        touch(td.path(), "secret.txt");
        let cands = read_dir_candidates(td.path(), false);
        let displays: Vec<&str> = cands.iter().map(|c| c.display.as_str()).collect();
        assert!(displays.contains(&"visible.txt"));
        assert!(!displays.contains(&"secret.txt"));
    }

    #[test]
    fn read_dir_hides_dotfiles_unless_requested() {
        let td = TempDir::new().unwrap();
        touch(td.path(), ".env");
        touch(td.path(), "visible.txt");
        let hidden = read_dir_candidates(td.path(), false);
        assert!(!hidden.iter().any(|c| c.display == ".env"));
        let shown = read_dir_candidates(td.path(), true);
        assert!(shown.iter().any(|c| c.display == ".env"));
    }

    #[test]
    fn empty_token_is_workspace_root() {
        let (d, n) = split_at_token(
            "",
            Path::new("/ws"),
            Path::new("/ws/sub"),
            Some(Path::new("/home")),
        );
        assert_eq!(d, PathBuf::from("/ws"));
        assert_eq!(n, "");
    }

    #[test]
    fn bare_name_resolves_against_workspace() {
        let (d, n) = split_at_token("foo", Path::new("/ws"), Path::new("/ws"), None);
        assert_eq!(d, PathBuf::from("/ws"));
        assert_eq!(n, "foo");
    }

    #[test]
    fn nested_relative_under_workspace() {
        let (d, n) = split_at_token("src/ma", Path::new("/ws"), Path::new("/ws"), None);
        assert_eq!(d, PathBuf::from("/ws/src/"));
        assert_eq!(n, "ma");
    }

    #[test]
    fn absolute_path_passes_through() {
        let (d, n) = split_at_token("/etc/h", Path::new("/ws"), Path::new("/ws"), None);
        assert_eq!(d, PathBuf::from("/etc/"));
        assert_eq!(n, "h");
    }

    #[test]
    fn tilde_expands_to_home() {
        let (d, n) = split_at_token(
            "~/.config/f",
            Path::new("/ws"),
            Path::new("/ws"),
            Some(Path::new("/home/john")),
        );
        assert_eq!(d, PathBuf::from("/home/john/.config/"));
        assert_eq!(n, "f");
    }

    #[test]
    fn dotdot_resolves_against_cwd() {
        let (d, n) = split_at_token("../sib/x", Path::new("/ws"), Path::new("/ws/inner"), None);
        assert_eq!(d, PathBuf::from("/ws/inner/../sib/"));
        assert_eq!(n, "x");
    }

    #[test]
    fn trailing_slash_means_empty_name() {
        let (d, n) = split_at_token("src/", Path::new("/ws"), Path::new("/ws"), None);
        assert_eq!(d, PathBuf::from("/ws/src/"));
        assert_eq!(n, "");
    }
}
