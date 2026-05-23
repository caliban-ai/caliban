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

/// Successfully resolved message ready to send.
#[derive(Debug)]
pub(crate) struct ResolvedMessage {
    pub(crate) visible_text: String,
    pub(crate) attachments: Vec<Attachment>,
}

#[derive(Debug)]
pub(crate) struct Attachment {
    /// Absolute path to the file on disk. Currently read-only metadata
    /// for callers (we ship the content inline, not the path) but kept
    /// because the next slice — tool-call rendering — will use it.
    #[allow(dead_code, reason = "consumed by future tool-call rendering")]
    pub(crate) path: PathBuf,
    pub(crate) display_path: String,
    pub(crate) bytes: u64,
    pub(crate) content: String,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum AttachError {
    #[error("@{} is {bytes} bytes; over the per-file limit of {limit}", path.display())]
    Oversize {
        path: PathBuf,
        bytes: u64,
        limit: u64,
    },
    #[error("attachments total {running_total} bytes; over the budget of {limit}")]
    BudgetExceeded { running_total: u64, limit: u64 },
    #[error("@{} is not valid UTF-8", path.display())]
    NotUtf8 { path: PathBuf },
    #[error("@{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Resolve every `@<path>` token in `buffer`. Tokens that don't resolve to an
/// existing regular file are left as literal text. If any resolved file
/// violates the per-file or aggregate size caps, returns an error and DOES
/// NOT attach anything.
pub(crate) fn resolve_attachments(
    buffer: &str,
    workspace_root: &Path,
    cwd: &Path,
    per_file_max: u64,
    total_budget: u64,
) -> Result<ResolvedMessage, AttachError> {
    let home = dirs::home_dir();
    let mut attachments = Vec::new();
    let mut running_total: u64 = 0;

    for tok in extract_at_tokens(buffer) {
        let (dir, name) = split_at_token(&tok, workspace_root, cwd, home.as_deref());
        let candidate = if name.is_empty() {
            dir.clone()
        } else {
            dir.join(&name)
        };
        if !candidate.is_file() {
            continue;
        }
        let meta = match std::fs::metadata(&candidate) {
            Ok(m) => m,
            Err(source) => {
                return Err(AttachError::Io {
                    path: candidate,
                    source,
                });
            }
        };
        let bytes = meta.len();
        if bytes > per_file_max {
            return Err(AttachError::Oversize {
                path: candidate,
                bytes,
                limit: per_file_max,
            });
        }
        running_total = running_total.saturating_add(bytes);
        if running_total > total_budget {
            return Err(AttachError::BudgetExceeded {
                running_total,
                limit: total_budget,
            });
        }
        let bytes_vec = match std::fs::read(&candidate) {
            Ok(b) => b,
            Err(source) => {
                return Err(AttachError::Io {
                    path: candidate,
                    source,
                });
            }
        };
        let Ok(content) = String::from_utf8(bytes_vec) else {
            return Err(AttachError::NotUtf8 { path: candidate });
        };
        let display_path = candidate
            .strip_prefix(workspace_root)
            .map_or_else(
                |_| candidate.display().to_string(),
                |p| p.display().to_string(),
            );
        attachments.push(Attachment {
            path: candidate,
            display_path,
            bytes,
            content,
        });
    }

    Ok(ResolvedMessage {
        visible_text: buffer.to_string(),
        attachments,
    })
}

/// Pull out every `@<token>` from `buffer`. Token = run of non-whitespace
/// after `@`, where the `@` sits at start-of-buffer or after whitespace.
fn extract_at_tokens(buffer: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = buffer.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'@' && (i == 0 || bytes[i - 1].is_ascii_whitespace()) {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && !(bytes[end] as char).is_whitespace() {
                end += 1;
            }
            if end > start {
                out.push(buffer[start..end].to_string());
            }
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

/// Build the outgoing wire string: `visible_text` followed by framed
/// `--- attached: ... ---` blocks for each attachment.
pub(crate) fn format_outgoing(msg: &ResolvedMessage) -> String {
    if msg.attachments.is_empty() {
        return msg.visible_text.clone();
    }
    let mut out = msg.visible_text.clone();
    for a in &msg.attachments {
        out.push_str("\n\n--- attached: ");
        out.push_str(&a.display_path);
        out.push_str(" (");
        out.push_str(&a.bytes.to_string());
        out.push_str(" bytes) ---\n");
        out.push_str(&a.content);
    }
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

    #[test]
    fn resolves_single_attachment() {
        let td = TempDir::new().unwrap();
        let p = td.path().join("hello.txt");
        fs::write(&p, "hi there").unwrap();
        let msg = format!("Look at @{}", p.display());
        let r = resolve_attachments(&msg, td.path(), td.path(), 1024, 4096).unwrap();
        assert_eq!(r.attachments.len(), 1);
        assert_eq!(r.attachments[0].bytes, 8);
        assert_eq!(r.attachments[0].content, "hi there");
    }

    #[test]
    fn missing_path_left_as_literal() {
        let td = TempDir::new().unwrap();
        let msg = "hello @nonexistent there";
        let r = resolve_attachments(msg, td.path(), td.path(), 1024, 4096).unwrap();
        assert!(r.attachments.is_empty());
        assert_eq!(r.visible_text, msg);
    }

    #[test]
    fn oversize_returns_error() {
        let td = TempDir::new().unwrap();
        let p = td.path().join("big.txt");
        fs::write(&p, vec![b'x'; 4096]).unwrap();
        let msg = format!("@{}", p.display());
        let err = resolve_attachments(&msg, td.path(), td.path(), 1024, 8192).unwrap_err();
        assert!(matches!(err, AttachError::Oversize { .. }));
    }

    #[test]
    fn budget_exceeded_returns_error() {
        let td = TempDir::new().unwrap();
        let a = td.path().join("a.txt");
        let b = td.path().join("b.txt");
        fs::write(&a, vec![b'x'; 700]).unwrap();
        fs::write(&b, vec![b'x'; 700]).unwrap();
        let msg = format!("@{} @{}", a.display(), b.display());
        let err = resolve_attachments(&msg, td.path(), td.path(), 1024, 1024).unwrap_err();
        assert!(matches!(err, AttachError::BudgetExceeded { .. }));
    }

    #[test]
    fn multiple_attachments_in_order() {
        let td = TempDir::new().unwrap();
        let a = td.path().join("a.txt");
        let b = td.path().join("b.txt");
        fs::write(&a, "aa").unwrap();
        fs::write(&b, "bb").unwrap();
        let msg = format!("@{} and @{}", a.display(), b.display());
        let r = resolve_attachments(&msg, td.path(), td.path(), 1024, 4096).unwrap();
        assert_eq!(r.attachments.len(), 2);
        assert_eq!(r.attachments[0].content, "aa");
        assert_eq!(r.attachments[1].content, "bb");
    }

    #[test]
    fn format_outgoing_includes_attachment_block() {
        let td = TempDir::new().unwrap();
        let p = td.path().join("note.txt");
        fs::write(&p, "body").unwrap();
        let msg = format!("see @{}", p.display());
        let r = resolve_attachments(&msg, td.path(), td.path(), 1024, 4096).unwrap();
        let wire = format_outgoing(&r);
        assert!(wire.contains("--- attached: "));
        assert!(wire.contains("note.txt"));
        assert!(wire.contains("body"));
    }
}
