//! Atomic file writes (tmp + rename) — one canonical recipe that replaces
//! the four ad-hoc copies that had drifted across the workspace.
//!
//! The temp file is created in the destination's parent directory so the
//! final `rename(2)` is atomic on POSIX filesystems.

use std::io::Write;
use std::path::Path;

/// Atomically write `bytes` to `path`.
///
/// Writes to a uniquely-named tempfile in `path`'s parent directory, then
/// `persist`s (renames) it onto `path`. On failure before persist the
/// tempfile is removed automatically when the `NamedTempFile` is dropped,
/// so no partial write is left behind.
///
/// # Errors
/// I/O errors from creating the tempfile, writing bytes, or the final
/// rename.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "write_atomic: path has no parent directory",
        )
    })?;
    if !parent.as_os_str().is_empty() {
        std::fs::create_dir_all(parent)?;
    }
    // Empty parent means CWD-relative path with no directory component;
    // tempfile::NamedTempFile::new_in("") fails, so substitute ".".
    let parent_for_temp: &Path = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    let mut tmp = tempfile::NamedTempFile::new_in(parent_for_temp)?;
    tmp.write_all(bytes)?;
    tmp.flush()?;
    // `NamedTempFile` (mkstemp) creates the tempfile at a private 0600, and a
    // rename preserves the source's mode — so without this the destination
    // would inherit 0600 (#224). Set the tempfile's mode before persisting:
    // preserve the destination's existing mode on overwrite (an executable
    // stays executable), otherwise apply the ordinary 0644 for a fresh file.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).map_or(0o644, |m| m.permissions().mode() & 0o777);
        tmp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(mode))?;
    }
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// Like [`write_atomic`], but `chmod`s the resulting file to `mode` on
/// Unix. On Windows the mode is ignored.
///
/// Use for credential blobs (`mode = 0o600`) and other security-sensitive
/// writes that need a non-default file mode.
///
/// # Errors
/// I/O errors as in [`write_atomic`], plus errors setting the mode.
pub fn write_atomic_with_mode(path: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    write_atomic(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(mode);
        std::fs::set_permissions(path, perms)?;
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn write_atomic_creates_file_with_correct_bytes() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("out.txt");
        write_atomic(&p, b"hello").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"hello");
    }

    #[test]
    fn write_atomic_overwrites_existing_file() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("out.txt");
        std::fs::write(&p, b"old").unwrap();
        write_atomic(&p, b"new").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"new");
    }

    #[test]
    fn write_atomic_leaves_no_temp_behind_on_success() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("out.txt");
        write_atomic(&p, b"hi").unwrap();
        // The destination is the only file in the directory.
        let entries: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name())
            .collect();
        assert_eq!(entries, vec![p.file_name().unwrap().to_owned()]);
    }

    #[test]
    fn write_atomic_creates_missing_parent_dir() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("nested").join("dir").join("out.txt");
        write_atomic(&p, b"x").unwrap();
        assert!(p.exists());
    }

    #[test]
    fn dropping_tempfile_before_persist_leaves_no_partial_write() {
        // Simulates "construct the tempfile, fail before persist" — the
        // destination must not exist and no leftover should remain in the
        // parent directory.
        let tmp = TempDir::new().unwrap();
        let dest = tmp.path().join("never.txt");
        {
            let mut tf = tempfile::NamedTempFile::new_in(tmp.path()).unwrap();
            tf.write_all(b"partial").unwrap();
            // drop without persist
        }
        assert!(!dest.exists(), "destination should not have appeared");
        // The directory should now be empty (NamedTempFile cleans up on drop).
        let count = std::fs::read_dir(tmp.path()).unwrap().count();
        assert_eq!(count, 0, "tempfile leak");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_with_mode_sets_0600() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("secret.txt");
        write_atomic_with_mode(&p, b"sssh", 0o600).unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "got mode {mode:o}");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_creates_new_file_with_0644() {
        // Regression for #224: the tempfile-then-rename recipe used to leak the
        // tempfile's private 0600 onto the destination. New files must land at
        // the ordinary 0644, matching `File::create` under the common umask.
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("fresh.txt");
        write_atomic(&p, b"content").unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "new file got mode {mode:o}, expected 0644");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_preserves_existing_mode_on_overwrite() {
        // A rewrite must keep the destination's mode (e.g. an executable stays
        // executable) rather than silently dropping to the tempfile's 0600.
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("script.sh");
        std::fs::write(&p, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        write_atomic(&p, b"#!/bin/sh\necho hi\n").unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o755,
            "overwrite changed mode to {mode:o}, expected 0755"
        );
    }
}
