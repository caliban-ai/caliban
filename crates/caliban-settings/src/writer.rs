//! Atomic, flock-protected TOML writes for caliban-owned config files.

#![allow(dead_code)] // until callers land in subsequent tasks

use std::path::{Path, PathBuf};

use fs2::FileExt;

use crate::Scope;

/// Kind of file being written within a scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// The main settings file (`settings.toml`).
    Settings,
    /// The permissions-specific file (`permissions.toml`).
    Permissions,
}

impl FileKind {
    fn filename(self) -> &'static str {
        match self {
            FileKind::Settings => "settings.toml",
            FileKind::Permissions => "permissions.toml",
        }
    }
}

/// Resolves to a scoped TOML file path under the caller's caliban config dir.
pub fn scope_path(scope: Scope, kind: FileKind, cwd: &Path) -> Option<PathBuf> {
    // Reuse the same paths as the loader. For brevity, support User and
    // Project here; Local/Managed land alongside their loader equivalents.
    match scope {
        Scope::Project => Some(cwd.join(".caliban").join(kind.filename())),
        Scope::Local => Some(cwd.join(".caliban").join(format!(
            "{}.local.toml",
            kind.filename().trim_end_matches(".toml")
        ))),
        Scope::User => dirs::config_dir().map(|d| d.join("caliban").join(kind.filename())),
        // Managed is read-only from caliban's perspective; CLI is in-memory only.
        Scope::Managed | Scope::Cli => None,
    }
}

/// Atomic write: flock a dedicated sibling `.lock` file, then write
/// to a uniquely-named temp file, fsync, rename onto the target.
///
/// **Why a dedicated lock file:** earlier versions flock'd the target
/// itself, which appeared to serialize writers on macOS but raced on
/// Linux CI. The atomic-rename pattern replaces the target's inode, so
/// a concurrent writer that opens the target *after* the rename
/// receives a different inode than the one the prior writer is holding
/// — the locks don't actually serialize. The race manifested as ENOENT
/// when two writers shared the same `.tmp` path and one renamed before
/// the other finished. A persistent sibling lock file is never renamed,
/// so all writers serialize on the same inode.
///
/// **Per-thread temp file:** the temp path additionally embeds the
/// writer's process+thread id so two concurrent writers don't collide
/// on the same `.tmp` file even within the same atomic-write window.
pub fn write_toml_atomic(target: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Lock on a sibling `.lock` file that persists across renames.
    let lock_path = lock_path_for(target);
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;
    lock.lock_exclusive()?;

    // Unique temp path per writer so concurrent writers don't trash
    // each other's tmp file before rename.
    let tid = std::thread::current().id();
    let pid = std::process::id();
    let tmp = target.with_extension(format!("toml.tmp.{pid}.{tid:?}"));
    let res: std::io::Result<()> = (|| {
        std::fs::write(&tmp, contents)?;
        if let Ok(f) = std::fs::File::open(&tmp) {
            let _ = f.sync_all();
        }
        std::fs::rename(&tmp, target)?;
        Ok(())
    })();

    // Best-effort unlock + cleanup of an orphaned tmp on error.
    let _ = fs2::FileExt::unlock(&lock);
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

fn lock_path_for(target: &Path) -> std::path::PathBuf {
    // Sibling lock file: `settings.toml` → `.settings.toml.lock`. Hidden
    // so it doesn't clutter ls; the leading dot also keeps editors from
    // accidentally opening it.
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("write");
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    parent.join(format!(".{name}.lock"))
}

/// Read the TOML at `target` (or start with an empty string if missing),
/// append a `[[permissions.rules]]` entry, write atomically.
///
/// Parent directories are created as needed (via [`write_toml_atomic`]).
pub fn append_rule_to_file(target: &Path, rule: &crate::RuleSpec) -> std::io::Result<()> {
    let mut existing = if target.exists() {
        std::fs::read_to_string(target)?
    } else {
        String::new()
    };
    let snippet = format_rule(rule);
    if !existing.ends_with('\n') && !existing.is_empty() {
        existing.push('\n');
    }
    existing.push_str(&snippet);
    write_toml_atomic(target, &existing)
}

fn format_rule(r: &crate::RuleSpec) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    s.push_str("\n[[permissions.rules]]\n");
    let _ = writeln!(s, "pattern = {}", toml_str(&r.pattern));
    let _ = writeln!(s, "action  = {}", toml_str(&r.action));
    if let Some(c) = &r.comment {
        let _ = writeln!(s, "comment = {}", toml_str(c));
    }
    if let Some(reason) = &r.reason {
        let _ = writeln!(s, "reason  = {}", toml_str(reason));
    }
    s
}

pub(crate) fn toml_str(v: &str) -> String {
    let escaped = v.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Remove the first `[[permissions.rules]]` entry whose `pattern` field equals
/// `pattern` from the TOML file at `target`, then write the result back
/// atomically.
///
/// Returns `Ok(true)` if a matching rule was found and removed, `Ok(false)` if
/// `target` does not exist, the section is missing, or no matching rule was
/// found. Returns `Err` only on I/O or TOML parse failures.
pub fn delete_rule_at(target: &Path, pattern: &str) -> std::io::Result<bool> {
    if !target.exists() {
        return Ok(false);
    }
    let body = std::fs::read_to_string(target)?;
    let mut doc: toml::Value = body
        .parse()
        .map_err(|e: toml::de::Error| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let perms = doc.get_mut("permissions").and_then(|p| p.as_table_mut());
    let Some(perms) = perms else {
        return Ok(false);
    };
    let Some(arr) = perms.get_mut("rules").and_then(|r| r.as_array_mut()) else {
        return Ok(false);
    };
    let before = arr.len();
    arr.retain(|v| v.get("pattern").and_then(|p| p.as_str()) != Some(pattern));
    if arr.len() == before {
        return Ok(false);
    }
    let new_contents = toml::to_string_pretty(&doc)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    write_toml_atomic(target, &new_contents)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_toml_atomic_creates_file_with_contents() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("settings.toml");
        write_toml_atomic(&target, "model = \"x\"\n").unwrap();
        let got = std::fs::read_to_string(&target).unwrap();
        assert_eq!(got, "model = \"x\"\n");
    }

    #[test]
    fn write_toml_atomic_overwrites_existing() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("settings.toml");
        std::fs::write(&target, "old = 1\n").unwrap();
        write_toml_atomic(&target, "new = 2\n").unwrap();
        let got = std::fs::read_to_string(&target).unwrap();
        assert_eq!(got, "new = 2\n");
    }

    #[test]
    fn scope_path_project_uses_caliban_dir() {
        let dir = tempfile::tempdir().unwrap();
        let p = scope_path(Scope::Project, FileKind::Permissions, dir.path()).unwrap();
        assert_eq!(p, dir.path().join(".caliban").join("permissions.toml"));
    }

    #[test]
    fn delete_rule_at_removes_matching_pattern() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("permissions.toml");
        std::fs::write(
            &target,
            "\n[[permissions.rules]]\npattern = \"A\"\naction = \"allow\"\n\n[[permissions.rules]]\npattern = \"B\"\naction = \"deny\"\n",
        )
        .unwrap();
        assert!(delete_rule_at(&target, "A").unwrap());
        let after = std::fs::read_to_string(&target).unwrap();
        assert!(!after.contains("\"A\""), "pattern A should be removed");
        assert!(after.contains("\"B\""), "pattern B should remain");
    }

    #[test]
    fn delete_rule_at_returns_false_for_nonexistent_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("missing.toml");
        assert!(!delete_rule_at(&target, "X").unwrap());
    }

    #[test]
    fn delete_rule_at_returns_false_for_missing_pattern() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("permissions.toml");
        std::fs::write(
            &target,
            "\n[[permissions.rules]]\npattern = \"B\"\naction = \"deny\"\n",
        )
        .unwrap();
        assert!(!delete_rule_at(&target, "A").unwrap());
        let after = std::fs::read_to_string(&target).unwrap();
        assert!(after.contains("\"B\""), "pattern B should remain untouched");
    }

    #[test]
    fn concurrent_writes_serialize_via_flock() {
        use std::sync::Arc;
        use std::thread;
        let dir = tempfile::tempdir().unwrap();
        let target = Arc::new(dir.path().join("settings.toml"));
        let mut handles = Vec::new();
        for i in 0..8 {
            let t = Arc::clone(&target);
            handles.push(thread::spawn(move || {
                // Each writer rewrites the file with a unique scalar.
                write_toml_atomic(&t, &format!("counter = {i}\n")).unwrap();
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let got = std::fs::read_to_string(&*target).unwrap();
        // Exact contents are "last writer wins"; verify the file is a valid TOML
        // line and that we didn't end up with truncated/interleaved content.
        assert!(got.starts_with("counter = ") && got.ends_with('\n'));
        let _: toml::Value =
            toml::from_str(&got).expect("must be valid TOML after concurrent writes");
    }
}
