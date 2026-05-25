//! Session-local image blob storage.
//!
//! Blobs are written to `<session_dir>/blobs/<sha256>.bin` keyed by SHA-256
//! over the post-resize image bytes. The [`caliban_provider::ImageSource::BlobRef`]
//! variant references entries here.

use std::fs;
use std::path::{Path, PathBuf};

/// Errors raised by [`BlobStore`].
#[derive(Debug, thiserror::Error)]
pub enum BlobStoreError {
    /// I/O error reading or writing a blob.
    #[error("blob store I/O at {path}: {source}", path = path.display())]
    Io {
        /// Path that failed.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// Requested blob was not present in the store.
    #[error("blob {sha256} not found")]
    NotFound {
        /// SHA-256 of the missing blob.
        sha256: String,
    },
    /// SHA-256 looks malformed (must be 64 hex chars).
    #[error("invalid sha256 fingerprint: {0:?}")]
    InvalidSha(String),
}

/// File-backed blob store rooted at a session directory.
#[derive(Debug, Clone)]
pub struct BlobStore {
    root: PathBuf,
}

impl BlobStore {
    /// Construct a store rooted at `<session_dir>/blobs/`.
    ///
    /// The directory is created lazily on the first [`Self::put`].
    #[must_use]
    pub fn new(session_dir: &Path) -> Self {
        Self {
            root: session_dir.join("blobs"),
        }
    }

    /// Root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path to the blob with `sha256`.
    fn path_for(&self, sha256: &str) -> Result<PathBuf, BlobStoreError> {
        if !is_valid_sha(sha256) {
            return Err(BlobStoreError::InvalidSha(sha256.to_string()));
        }
        Ok(self.root.join(format!("{sha256}.bin")))
    }

    /// Write `bytes` keyed by `sha256`. Skips the write if the target already
    /// exists (idempotent).
    ///
    /// # Errors
    ///
    /// Returns [`BlobStoreError::InvalidSha`] if `sha256` is not 64 lowercase
    /// hex chars, or [`BlobStoreError::Io`] on file I/O failure.
    pub fn put(&self, sha256: &str, bytes: &[u8]) -> Result<PathBuf, BlobStoreError> {
        let path = self.path_for(sha256)?;
        if path.exists() {
            return Ok(path);
        }
        fs::create_dir_all(&self.root).map_err(|source| BlobStoreError::Io {
            path: self.root.clone(),
            source,
        })?;
        fs::write(&path, bytes).map_err(|source| BlobStoreError::Io {
            path: path.clone(),
            source,
        })?;
        Ok(path)
    }

    /// Read the bytes of the blob keyed by `sha256`.
    ///
    /// # Errors
    ///
    /// Returns [`BlobStoreError::NotFound`] if no such blob, or
    /// [`BlobStoreError::Io`] on I/O failure.
    pub fn get(&self, sha256: &str) -> Result<Vec<u8>, BlobStoreError> {
        let path = self.path_for(sha256)?;
        if !path.exists() {
            return Err(BlobStoreError::NotFound {
                sha256: sha256.to_string(),
            });
        }
        fs::read(&path).map_err(|source| BlobStoreError::Io { path, source })
    }

    /// `true` if a blob with `sha256` exists in this store.
    #[must_use]
    pub fn contains(&self, sha256: &str) -> bool {
        self.path_for(sha256).is_ok_and(|p| p.exists())
    }
}

fn is_valid_sha(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sha(s: &str) -> String {
        // Build a 64-char lowercase-hex string by repeating `s`.
        let mut out = String::new();
        while out.len() < 64 {
            out.push_str(s);
        }
        out.truncate(64);
        out
    }

    #[test]
    fn put_get_roundtrip() {
        let td = TempDir::new().unwrap();
        let bs = BlobStore::new(td.path());
        let id = sha("ab");
        bs.put(&id, b"hello").unwrap();
        let bytes = bs.get(&id).unwrap();
        assert_eq!(bytes, b"hello");
    }

    #[test]
    fn put_is_idempotent() {
        let td = TempDir::new().unwrap();
        let bs = BlobStore::new(td.path());
        let id = sha("cd");
        let a = bs.put(&id, b"first").unwrap();
        // Second call must not overwrite even with different bytes.
        let b = bs.put(&id, b"different").unwrap();
        assert_eq!(a, b);
        assert_eq!(bs.get(&id).unwrap(), b"first");
    }

    #[test]
    fn missing_blob_errors() {
        let td = TempDir::new().unwrap();
        let bs = BlobStore::new(td.path());
        let id = sha("ef");
        let err = bs.get(&id).unwrap_err();
        assert!(matches!(err, BlobStoreError::NotFound { .. }));
    }

    #[test]
    fn invalid_sha_rejected() {
        let td = TempDir::new().unwrap();
        let bs = BlobStore::new(td.path());
        let err = bs.put("not-hex", b"x").unwrap_err();
        assert!(matches!(err, BlobStoreError::InvalidSha(_)));
    }
}
