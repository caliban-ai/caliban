//! `caliban.toml` discovery.
//!
//! Layering (highest precedence first):
//! 1. Explicit path (`--config <PATH>` CLI flag).
//! 2. `CALIBAN_ROUTER_CONFIG` env var.
//! 3. Walk up from `start` to the nearest git root or `$HOME`.
//! 4. `~/.config/caliban/caliban.toml`.
//!
//! The walk-up uses `caliban_common::paths::walk_up_for_file` so both
//! CLAUDE.md (ADR 0018) and `caliban.toml` (ADR 0038) share one algorithm.

use std::path::{Path, PathBuf};

use caliban_common::paths::walk_up_for_file;

use crate::config::{CalibanConfig, parse_caliban_config};

/// Result of [`discover_caliban_toml`].
#[derive(Debug, Clone)]
pub struct DiscoveredConfig {
    /// Path the config was loaded from.
    pub path: PathBuf,
    /// Parsed config.
    pub config: CalibanConfig,
}

/// Failure modes during discovery / parse.
#[derive(thiserror::Error, Debug)]
pub enum DiscoveryError {
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

/// Discover and load `caliban.toml` per the precedence rules above.
///
/// Returns `Ok(None)` when no config is found anywhere — the caller should
/// fall back to the single-provider construction path.
///
/// # Errors
/// Returns [`DiscoveryError`] when an explicit path can't be read/parsed or
/// when a discovered file is malformed.
pub fn discover_caliban_toml(
    explicit: Option<&Path>,
    start: &Path,
) -> Result<Option<DiscoveredConfig>, DiscoveryError> {
    if let Some(p) = explicit {
        return read_and_parse(p).map(Some);
    }
    if let Some(env_path) = std::env::var_os("CALIBAN_ROUTER_CONFIG") {
        let p = PathBuf::from(env_path);
        if p.is_file() {
            return read_and_parse(&p).map(Some);
        }
    }
    if let Some(found) = walk_up_for_file(start, "caliban.toml") {
        return read_and_parse(&found).map(Some);
    }
    if let Some(home_cfg) = dirs::config_dir().map(|d| d.join("caliban").join("caliban.toml"))
        && home_cfg.is_file()
    {
        return read_and_parse(&home_cfg).map(Some);
    }
    Ok(None)
}

fn read_and_parse(path: &Path) -> Result<DiscoveredConfig, DiscoveryError> {
    let body = std::fs::read_to_string(path).map_err(|e| DiscoveryError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let config = parse_caliban_config(&body).map_err(|e| DiscoveryError::Parse {
        path: path.to_path_buf(),
        source: e,
    })?;
    Ok(DiscoveredConfig {
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
    fn discovers_via_walk_up_from_subdir() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("caliban.toml"), MINIMAL).unwrap();
        fs::create_dir_all(tmp.path().join(".git")).unwrap();
        let sub = tmp.path().join("a/b");
        fs::create_dir_all(&sub).unwrap();
        let found = discover_caliban_toml(None, &sub).unwrap().unwrap();
        assert_eq!(
            found.path.canonicalize().unwrap(),
            tmp.path().join("caliban.toml").canonicalize().unwrap()
        );
    }

    #[test]
    fn explicit_path_overrides_discovery() {
        let tmp = tempdir().unwrap();
        // Discovery would otherwise find this one.
        fs::write(tmp.path().join("caliban.toml"), "this is malformed").unwrap();
        fs::create_dir_all(tmp.path().join(".git")).unwrap();
        // Explicit one is correct.
        let explicit_path = tmp.path().join("override.toml");
        fs::write(&explicit_path, MINIMAL).unwrap();
        let found = discover_caliban_toml(Some(&explicit_path), tmp.path())
            .unwrap()
            .unwrap();
        assert_eq!(
            found.path.canonicalize().unwrap(),
            explicit_path.canonicalize().unwrap()
        );
    }

    #[test]
    fn missing_returns_ok_none() {
        // Only meaningful when CALIBAN_ROUTER_CONFIG isn't set in the test
        // environment; if it is, skip the assertion. tempdir's .git stops
        // the walk-up.
        if std::env::var_os("CALIBAN_ROUTER_CONFIG").is_some() {
            return;
        }
        let tmp = tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(".git")).unwrap();
        let found = discover_caliban_toml(None, tmp.path()).unwrap();
        // The ~/.config/caliban/caliban.toml fallback may exist on this
        // machine; treat `Some` results as "not from the walk-up".
        if let Some(d) = found {
            assert!(
                !d.path
                    .canonicalize()
                    .unwrap()
                    .starts_with(tmp.path().canonicalize().unwrap()),
                "discovery returned a path inside the empty tempdir: {:?}",
                d.path
            );
        }
    }

    #[test]
    fn parse_error_surfaces() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("caliban.toml");
        fs::write(&p, "this is not = valid toml [[[").unwrap();
        let err = discover_caliban_toml(Some(&p), tmp.path()).unwrap_err();
        assert!(matches!(err, DiscoveryError::Parse { .. }));
    }
}
