//! Explicit `caliban.toml` loading for the model router.
//!
//! Router configuration now resolves through the caliban **settings** layer —
//! `.caliban/settings.toml` `[router]`, with the standard settings precedence
//! chain (ADR 0060). The only file-based path that remains is an *explicit*
//! override: `--config <PATH>` (also bound to `CALIBAN_ROUTER_CONFIG`). The
//! walk-up + home-dir *discovery* that used to auto-locate a `caliban.toml`
//! anywhere above the cwd was removed in #699 — a bare repo-root `caliban.toml`
//! is no longer picked up implicitly (migrate it with `caliban config
//! import-router`).

use std::path::{Path, PathBuf};

use crate::config::{CalibanConfig, parse_caliban_config};

/// A `caliban.toml` loaded from an explicit path.
#[derive(Debug, Clone)]
pub struct LoadedConfig {
    /// Path the config was loaded from.
    pub path: PathBuf,
    /// Parsed config.
    pub config: CalibanConfig,
}

/// Failure modes when reading / parsing an explicit `caliban.toml`.
#[derive(thiserror::Error, Debug)]
pub enum ConfigFileError {
    /// IO error reading a config file.
    #[error("caliban.toml: I/O error at {path}: {source}")]
    Io {
        /// Path that failed.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// Parse error.
    #[error("caliban.toml: parse error at {path}: {source}")]
    Parse {
        /// Path that failed.
        path: PathBuf,
        /// Underlying parse error.
        #[source]
        source: toml::de::Error,
    },
}

/// Load an *explicit* router config file, if one was requested.
///
/// `explicit` is the `--config <PATH>` flag (also bound to
/// `CALIBAN_ROUTER_CONFIG`). Returns `Ok(None)` when no explicit path was given
/// — the caller then consults the settings layer, falling back to the
/// single-provider construction path. There is no implicit discovery: unlike
/// pre-#699, an unset path does **not** trigger a walk-up or home-dir search.
///
/// # Errors
/// Returns [`ConfigFileError`] when the explicit path can't be read or parsed.
pub fn load_router_config_file(
    explicit: Option<&Path>,
) -> Result<Option<LoadedConfig>, ConfigFileError> {
    match explicit {
        Some(p) => read_and_parse(p).map(Some),
        None => Ok(None),
    }
}

fn read_and_parse(path: &Path) -> Result<LoadedConfig, ConfigFileError> {
    let body = std::fs::read_to_string(path).map_err(|e| ConfigFileError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let config = parse_caliban_config(&body).map_err(|e| ConfigFileError::Parse {
        path: path.to_path_buf(),
        source: e,
    })?;
    Ok(LoadedConfig {
        path: path.to_path_buf(),
        config,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    const MINIMAL: &str = r#"
[router]
default_purpose = "main_loop"

[[router.route]]
purpose = "main_loop"
provider = "anthropic"
model = "x"
"#;

    #[test]
    fn explicit_path_loads() {
        let tmp = tempdir().unwrap();
        let explicit_path = tmp.path().join("override.toml");
        fs::write(&explicit_path, MINIMAL).unwrap();
        let loaded = load_router_config_file(Some(&explicit_path))
            .unwrap()
            .unwrap();
        assert_eq!(
            loaded.path.canonicalize().unwrap(),
            explicit_path.canonicalize().unwrap()
        );
        assert!(loaded.config.router.is_some());
    }

    #[test]
    fn no_explicit_path_returns_none() {
        // #699: discovery is gone — an unset path never walks up or searches
        // the home dir; it just yields None so the caller uses settings.
        assert!(load_router_config_file(None).unwrap().is_none());
    }

    #[test]
    fn parse_error_surfaces() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("caliban.toml");
        fs::write(&p, "this is not = valid toml [[[").unwrap();
        let err = load_router_config_file(Some(&p)).unwrap_err();
        assert!(matches!(err, ConfigFileError::Parse { .. }));
    }

    #[test]
    fn missing_explicit_file_is_io_error() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("does-not-exist.toml");
        let err = load_router_config_file(Some(&p)).unwrap_err();
        assert!(matches!(err, ConfigFileError::Io { .. }));
    }
}
