//! Resolve `@path` tokens to file attachments at submit time.

use std::path::{Path, PathBuf};

/// Split an `@<token>` (passed WITHOUT the leading `@`) into the directory
/// to enumerate and the name fragment to match against.
#[allow(dead_code, reason = "wired in T6 @-completion")]
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

#[cfg(test)]
mod tests {
    use super::*;

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
