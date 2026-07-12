//! Atomic file writes (tmp + rename) — one canonical recipe that replaces
//! the four ad-hoc copies that had drifted across the workspace.
//!
//! The temp file is created in the destination's parent directory so the
//! final `rename(2)` is atomic on POSIX filesystems.

use std::io::Write;
use std::path::Path;

/// The mode a *fresh* file should get, honoring the process umask (#417).
///
/// `std` exposes no umask accessor, so probe once and cache: a file created via
/// `File::create` gets `0o666 & !umask`, and since `0o644 ⊆ 0o666` we have
/// `0o644 & !umask == 0o644 & probe_mode`. Falls back to the historical `0o644`
/// if the probe fails.
#[cfg(unix)]
fn umask_respecting_default() -> u32 {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::OnceLock;
    static MODE: OnceLock<u32> = OnceLock::new();
    *MODE.get_or_init(|| {
        let path =
            std::env::temp_dir().join(format!(".caliban-umask-probe-{}", std::process::id()));
        let probed = std::fs::File::create(&path)
            .and_then(|f| f.metadata())
            .map(|m| 0o644 & m.permissions().mode());
        let _ = std::fs::remove_file(&path);
        probed.unwrap_or(0o644)
    })
}

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
    // When `path` is a symlink, write *through* it to its target so the link's
    // identity survives (#335): otherwise the tmp+rename would replace the link
    // itself with a fresh regular file (`latest → v1.2.3` becomes a divergent
    // regular file). `canonicalize` follows the whole chain; a dangling or
    // unresolvable link falls back to the path as given (replace-in-place).
    let dest: std::path::PathBuf = std::fs::symlink_metadata(path)
        .ok()
        .filter(|m| m.file_type().is_symlink())
        .and_then(|_| std::fs::canonicalize(path).ok())
        .unwrap_or_else(|| path.to_path_buf());
    let dest = dest.as_path();

    let parent = dest.parent().ok_or_else(|| {
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
    // Mask `& 0o7777` (not `0o777`) so setuid/setgid/sticky bits survive a
    // rewrite — a `2755` setgid script must not come back `0755` (#335).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = match std::fs::metadata(dest) {
            Ok(m) => m.permissions().mode() & 0o7777,
            // Fresh file: honor the process umask instead of a hardcoded 0644,
            // so a hardened umask (e.g. 0077 expecting 0600) isn't overridden
            // to world-readable (#417).
            Err(_) => umask_respecting_default(),
        };
        tmp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(mode))?;
    }
    tmp.persist(dest).map_err(|e| e.error)?;
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

/// Atomically write `bytes` to `path`, confined beneath `root`.
///
/// Like [`write_atomic`], but every path component below `root` is opened with
/// `O_NOFOLLOW` starting from `root` itself, so a symlink planted anywhere in
/// the path *after* the caller validated containment cannot redirect the write
/// outside `root`. This closes the TOCTOU window between the workspace fence
/// check and the write (#415): a concurrent `ln -s /etc work/sub` can no longer
/// steer a workspace write to `/etc`.
///
/// `root` must be an existing, canonical directory and `path` must lie beneath
/// it (the caller's fence guarantees this; it is re-checked defensively). A
/// symlink encountered at *any* component — including the final name — is
/// refused rather than followed: the caller's path resolution has already
/// canonicalized legitimate in-tree symlinks, so a link appearing here is a
/// concurrent swap, and writing *through* it would cross the fence. The final
/// name is replaced via `renameat`, which retargets the name, not a link's
/// destination.
///
/// # Errors
/// I/O errors from opening/creating the confined path or writing, plus
/// `PermissionDenied` if `path` is not within `root` or a component is a
/// symlink.
// The component walk, mode preservation, temp-create/retry, and cleanup form
// one linear procedure that reads better whole than split across helpers.
#[allow(clippy::too_many_lines)]
#[cfg(unix)]
pub fn write_atomic_within(root: &Path, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use rustix::fs::{
        AtFlags, CWD, FileType, Mode, OFlags, RawMode, fchmod, mkdirat, openat, renameat, statat,
        unlinkat,
    };
    use rustix::io::Errno;
    use std::sync::atomic::{AtomicU64, Ordering};

    // Per-process temp-name counter (hoisted to satisfy items-after-statements).
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let rel = path.strip_prefix(root).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "write_atomic_within: {} is not within workspace root {}",
                path.display(),
                root.display()
            ),
        )
    })?;

    // Split the relative path into directory components + final filename,
    // rejecting any `..` that survived (defense-in-depth; the caller normalizes).
    let mut comps: Vec<&std::ffi::OsStr> = Vec::new();
    for c in rel.components() {
        match c {
            std::path::Component::Normal(s) => comps.push(s),
            std::path::Component::CurDir => {}
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "write_atomic_within: path escapes root after normalization",
                ));
            }
        }
    }
    let Some((filename, dirs)) = comps.split_last() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "write_atomic_within: path has no filename (equals root)",
        ));
    };

    let symlink_refused = || {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "write_atomic_within: refusing to traverse a symlinked path component",
        )
    };

    // Open the trusted, canonical root. Following its own (already-resolved)
    // components is fine — confinement only forbids escaping *below* it.
    let mut dir = openat(
        CWD,
        root,
        OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;

    // Walk/create each intermediate directory without ever following a symlink.
    let dir_flags = OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    for comp in dirs {
        let next = match openat(&dir, *comp, dir_flags, Mode::empty()) {
            Ok(fd) => fd,
            Err(Errno::NOENT) => {
                match mkdirat(&dir, *comp, Mode::from_raw_mode(0o777)) {
                    Ok(()) | Err(Errno::EXIST) => {}
                    Err(e) => return Err(e.into()),
                }
                openat(&dir, *comp, dir_flags, Mode::empty()).map_err(|e| match e {
                    Errno::LOOP | Errno::NOTDIR => symlink_refused(),
                    other => other.into(),
                })?
            }
            Err(Errno::LOOP | Errno::NOTDIR) => return Err(symlink_refused()),
            Err(e) => return Err(e.into()),
        };
        dir = next;
    }

    // Destination mode: preserve an existing regular file's bits (an executable
    // stays executable, setgid survives), else a fresh umask-respecting default.
    // A symlink at the final name is a planted swap — ignore its mode; it will
    // be replaced in place.
    let mode_bits: RawMode = match statat(&dir, *filename, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(st) if FileType::from_raw_mode(st.st_mode) == FileType::RegularFile => {
            st.st_mode & 0o7777
        }
        _ => RawMode::try_from(umask_respecting_default()).unwrap_or(0o644),
    };

    // Create a uniquely-named temp file *in this pinned directory* (O_EXCL).
    let create_flags = OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::CLOEXEC;
    let mut attempts = 0u32;
    let (tmp_name, tmp_fd) = loop {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let name = format!(".caliban-tmp-{}-{n}", std::process::id());
        match openat(
            &dir,
            name.as_str(),
            create_flags,
            Mode::from_raw_mode(mode_bits),
        ) {
            Ok(fd) => break (name, fd),
            Err(Errno::EXIST) if attempts < 10_000 => attempts += 1,
            Err(e) => return Err(e.into()),
        }
    };

    // Write, force the exact mode (O_CREAT already applied the umask), then
    // atomically rename over the final name within the same pinned directory.
    let persisted = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::from(tmp_fd);
        file.write_all(bytes)?;
        file.flush()?;
        fchmod(&file, Mode::from_raw_mode(mode_bits))?;
        drop(file);
        renameat(&dir, tmp_name.as_str(), &dir, *filename)?;
        Ok(())
    })();

    if persisted.is_err() {
        let _ = unlinkat(&dir, tmp_name.as_str(), AtFlags::empty());
    }
    persisted
}

/// Non-Unix fallback for [`write_atomic_within`]: best-effort confinement by
/// re-canonicalizing the real parent after `create_dir_all` and refusing if it
/// escaped `root`, then an ordinary atomic write. Symlink creation is a
/// privileged operation on Windows, so the TOCTOU exposure is far smaller.
///
/// # Errors
/// As [`write_atomic`], plus `PermissionDenied` if the resolved parent escapes
/// `root`.
#[cfg(not(unix))]
pub fn write_atomic_within(root: &Path, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no parent")
    })?;
    if !parent.as_os_str().is_empty() {
        std::fs::create_dir_all(parent)?;
    }
    let real_parent = std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
    let real_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    if !real_parent.starts_with(&real_root) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "write_atomic_within: resolved parent escaped workspace root",
        ));
    }
    write_atomic(path, bytes)
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
    fn write_atomic_preserves_special_bits_on_overwrite() {
        // #335: setuid/setgid/sticky bits must survive a rewrite. Previously the
        // mode was masked `& 0o777`, so a `2755` setgid script came back `0755`.
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("tool");
        std::fs::write(&p, b"#!/bin/sh\n").unwrap();
        // setgid (0o2000) + sticky (0o1000) + 0o755.
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o3755)).unwrap();
        write_atomic(&p, b"#!/bin/sh\necho hi\n").unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o7777;
        assert_eq!(
            mode, 0o3755,
            "special bits stripped: got {mode:o}, expected 3755"
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_writes_through_symlink() {
        // #335: a write to a symlink must update the target and keep the link,
        // not replace `latest → v1` with a divergent regular file.
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("v1");
        std::fs::write(&target, b"old").unwrap();
        let link = tmp.path().join("latest");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        write_atomic(&link, b"new").unwrap();

        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "symlink identity destroyed — link became a regular file"
        );
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"new",
            "target not updated"
        );
        assert_eq!(
            std::fs::read(&link).unwrap(),
            b"new",
            "read-through mismatch"
        );
    }

    #[test]
    fn write_atomic_within_writes_confined_file() {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let dest = root.join("a").join("b").join("out.txt");
        write_atomic_within(&root, &dest, b"hello").unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"hello");
    }

    #[test]
    fn write_atomic_within_overwrites_existing() {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let dest = root.join("out.txt");
        std::fs::write(&dest, b"old").unwrap();
        write_atomic_within(&root, &dest, b"new").unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"new");
    }

    #[test]
    fn write_atomic_within_rejects_path_outside_root() {
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let outside = TempDir::new().unwrap();
        let dest = outside.path().join("f.txt");
        let err = write_atomic_within(&root, &dest, b"x").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(!dest.exists());
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_within_refuses_symlinked_dir_component() {
        // The TOCTOU acceptance case (#415): a symlink planted at an intermediate
        // path component — as a concurrent `background: true` Bash job could do in
        // the fence-check→write window — must NOT redirect the write out of root.
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let outside = TempDir::new().unwrap();
        let outside_dir = std::fs::canonicalize(outside.path()).unwrap();

        // root/sub -> /outside   (the planted swap)
        let sub = root.join("sub");
        std::os::unix::fs::symlink(&outside_dir, &sub).unwrap();

        // Target looks in-root (root/sub/pwned) but sub is a symlink to /outside.
        let dest = sub.join("pwned");
        let err = write_atomic_within(&root, &dest, b"escaped").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
        // The write must NOT have landed in the outside directory.
        assert!(
            !outside_dir.join("pwned").exists(),
            "write escaped the fence via a symlinked component"
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_within_replaces_planted_final_symlink_in_place() {
        // A symlink planted at the *final* name (pointing outside root) is
        // replaced in place rather than written through, so the outside target is
        // never touched and the write stays confined.
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let outside = TempDir::new().unwrap();
        let outside_target = std::fs::canonicalize(outside.path())
            .unwrap()
            .join("secret");
        std::fs::write(&outside_target, b"original").unwrap();

        // root/link -> /outside/secret
        let link = root.join("link");
        std::os::unix::fs::symlink(&outside_target, &link).unwrap();

        write_atomic_within(&root, &link, b"new").unwrap();

        // The outside target is untouched; `root/link` is now a plain in-root file.
        assert_eq!(std::fs::read(&outside_target).unwrap(), b"original");
        assert!(
            !std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&link).unwrap(), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_within_preserves_mode_on_overwrite() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let dest = root.join("script.sh");
        std::fs::write(&dest, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755)).unwrap();
        write_atomic_within(&root, &dest, b"#!/bin/sh\necho hi\n").unwrap();
        let mode = std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "overwrite changed mode to {mode:o}");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_within_new_file_is_0644() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let dest = root.join("nested").join("fresh.txt");
        write_atomic_within(&root, &dest, b"x").unwrap();
        let mode = std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "new file got {mode:o}, expected 0644");
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
