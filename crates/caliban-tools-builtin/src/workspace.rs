//! `WorkspaceRoot` — resolves and optionally restricts paths for built-in tools.

use std::path::{Path, PathBuf};

use caliban_agent_core::ToolError;

/// Path resolver for built-in tools.
#[derive(Debug, Clone)]
pub struct WorkspaceRoot {
    root: PathBuf,
    restrict_to_root: bool,
}

impl WorkspaceRoot {
    /// Construct from an absolute (canonicalized) root path.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        Self {
            root,
            restrict_to_root: false,
        }
    }

    /// Construct from the current working directory.
    ///
    /// # Errors
    /// Returns an `io::Error` if the cwd cannot be obtained.
    pub fn current_dir() -> std::io::Result<Self> {
        let cwd = std::env::current_dir()?;
        Ok(Self::new(cwd))
    }

    /// Mark this root as restricted; subsequent `resolve` calls will reject
    /// paths outside the root.
    #[must_use]
    pub fn restricted(mut self) -> Self {
        self.restrict_to_root = true;
        self
    }

    /// Get the canonical root path.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Whether resolution rejects out-of-root paths.
    #[must_use]
    pub fn is_restricted(&self) -> bool {
        self.restrict_to_root
    }

    /// Resolve an input string into an absolute path.
    ///
    /// Relative paths are joined with the root; absolute paths pass through
    /// (or are rejected in restricted mode if outside root).
    ///
    /// # Errors
    /// Returns `ToolError::InvalidInput` if the path is empty or, in restricted mode,
    /// if the resolved path is outside the workspace root.
    pub fn resolve(&self, input: &str) -> Result<PathBuf, ToolError> {
        if input.is_empty() {
            return Err(ToolError::invalid_input("empty path"));
        }
        let candidate = PathBuf::from(input);
        let abs = if candidate.is_absolute() {
            candidate
        } else {
            self.root.join(&candidate)
        };
        // Canonicalize parent (file may not exist yet for Write tool).
        let canon = canonicalize_existing_ancestor(&abs);
        if self.restrict_to_root && !canon.starts_with(&self.root) {
            return Err(ToolError::invalid_input(format!(
                "path {} is outside workspace root {}",
                canon.display(),
                self.root.display(),
            )));
        }
        Ok(canon)
    }

    /// Make an absolute path relative to the workspace root if it lies within;
    /// otherwise return the input unchanged.
    #[must_use]
    pub fn relativize(&self, abs: &Path) -> PathBuf {
        abs.strip_prefix(&self.root)
            .map_or_else(|_| abs.to_path_buf(), Path::to_path_buf)
    }
}

/// Canonicalize as much of the path as exists, then append the rest. This
/// lets us check restriction even for paths that don't yet exist.
fn canonicalize_existing_ancestor(p: &Path) -> PathBuf {
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    let mut cur = p;
    loop {
        if let Ok(canon) = std::fs::canonicalize(cur) {
            let mut full = canon;
            for seg in tail.iter().rev() {
                full.push(seg);
            }
            return full;
        }
        match (cur.file_name(), cur.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name);
                cur = parent;
            }
            _ => return p.to_path_buf(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn resolve_relative() {
        let tmp = TempDir::new().unwrap();
        let root = WorkspaceRoot::new(tmp.path());
        let resolved = root.resolve("foo.txt").unwrap();
        assert!(resolved.starts_with(root.root()));
    }

    #[test]
    fn resolve_absolute_unrestricted() {
        let tmp = TempDir::new().unwrap();
        let root = WorkspaceRoot::new(tmp.path());
        let resolved = root.resolve("/tmp").unwrap();
        // /tmp may canonicalize differently on macOS, but resolution should succeed.
        let _ = resolved;
    }

    #[test]
    fn restricted_rejects_outside() {
        let tmp = TempDir::new().unwrap();
        let root = WorkspaceRoot::new(tmp.path()).restricted();
        let err = root.resolve("/etc/passwd").unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[test]
    fn restricted_allows_inside() {
        let tmp = TempDir::new().unwrap();
        let root = WorkspaceRoot::new(tmp.path()).restricted();
        let resolved = root.resolve("foo.txt").unwrap();
        assert!(resolved.starts_with(root.root()));
    }

    #[test]
    fn restricted_rejects_traversal() {
        let tmp = TempDir::new().unwrap();
        let inner = tmp.path().join("inner");
        std::fs::create_dir_all(&inner).unwrap();
        let root = WorkspaceRoot::new(&inner).restricted();
        let err = root.resolve("../escape.txt").unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[test]
    fn empty_path_errors() {
        let tmp = TempDir::new().unwrap();
        let root = WorkspaceRoot::new(tmp.path());
        let err = root.resolve("").unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }
}
