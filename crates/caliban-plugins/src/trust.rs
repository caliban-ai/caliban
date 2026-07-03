//! Trust store for marketplace plugin installs.
//!
//! Persisted at `~/.caliban/marketplaces-allowlist.json` (and a per-plugin
//! trust file at `$XDG_DATA_HOME/caliban/trust/plugins.json`). The first
//! install of a plugin from a new marketplace triggers a trust prompt; on
//! approval the record is cached and subsequent installs from the same
//! `(marketplace, plugin, manifest_hash)` triple skip the prompt.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::PluginError;

/// Per-plugin trust record. Persisted under `plugins.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginTrustRecord {
    /// Version installed.
    pub version: String,
    /// Marketplace URL (or `"sideload"`).
    pub marketplace: String,
    /// Hex-encoded sha256 of the manifest at install time.
    pub manifest_sha256: String,
    /// RFC 3339 timestamp.
    pub installed_at: String,
}

/// On-disk format of the per-plugin trust file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrustFile {
    /// `<plugin-name>` → record.
    #[serde(flatten)]
    pub plugins: BTreeMap<String, PluginTrustRecord>,
}

/// On-disk format of the marketplaces allowlist.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MarketplacesAllowlist {
    /// Approved marketplace URLs.
    #[serde(default)]
    pub approved: Vec<String>,
}

/// In-memory wrapper around the two on-disk files.
#[derive(Debug, Clone)]
pub struct TrustStore {
    /// Path to `plugins.json`.
    pub trust_path: PathBuf,
    /// Path to `marketplaces-allowlist.json`.
    pub allowlist_path: PathBuf,
    /// Cached trust records.
    pub records: TrustFile,
    /// Cached marketplace allowlist.
    pub allowlist: MarketplacesAllowlist,
}

impl TrustStore {
    /// Construct a store at the given paths and lazily load both files (if
    /// they exist; missing files yield empty defaults).
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::Io`] on read failure other than `NotFound`
    /// and [`PluginError::Parse`] on malformed JSON.
    pub fn open(trust_path: PathBuf, allowlist_path: PathBuf) -> Result<Self, PluginError> {
        let records = read_json_or_default::<TrustFile>(&trust_path)?;
        let allowlist = read_json_or_default::<MarketplacesAllowlist>(&allowlist_path)?;
        Ok(Self {
            trust_path,
            allowlist_path,
            records,
            allowlist,
        })
    }

    /// Default on-disk paths under `$HOME` / `$XDG_DATA_HOME`.
    #[must_use]
    pub fn default_paths() -> (PathBuf, PathBuf) {
        let trust = caliban_common::paths::platform_data_dir().map_or_else(
            || PathBuf::from(".caliban-trust/plugins.json"),
            |d| d.join("caliban").join("trust").join("plugins.json"),
        );
        let allow = caliban_common::paths::platform_data_dir().map_or_else(
            || PathBuf::from(".caliban-trust/marketplaces-allowlist.json"),
            |d| d.join("caliban").join("marketplaces-allowlist.json"),
        );
        (trust, allow)
    }

    /// Open the default trust store.
    ///
    /// # Errors
    ///
    /// See [`TrustStore::open`].
    pub fn open_default() -> Result<Self, PluginError> {
        let (t, a) = Self::default_paths();
        Self::open(t, a)
    }

    /// Persist both files to disk (best-effort; creates parent dirs).
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::Io`] on write failure.
    pub fn save(&self) -> Result<(), PluginError> {
        write_json(&self.trust_path, &self.records)?;
        write_json(&self.allowlist_path, &self.allowlist)?;
        Ok(())
    }

    /// Returns true if the marketplace URL was previously approved.
    #[must_use]
    pub fn is_marketplace_approved(&self, url: &str) -> bool {
        self.allowlist.approved.iter().any(|u| u == url)
    }

    /// Mark a marketplace URL as approved (idempotent).
    pub fn approve_marketplace(&mut self, url: &str) {
        if !self.is_marketplace_approved(url) {
            self.allowlist.approved.push(url.to_string());
        }
    }

    /// Return the existing trust record for `name`, if any.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&PluginTrustRecord> {
        self.records.plugins.get(name)
    }

    /// Record an install. Replaces any existing record for the same name.
    pub fn record(&mut self, name: &str, record: PluginTrustRecord) {
        self.records.plugins.insert(name.to_string(), record);
    }

    /// Drop the trust record for `name`. Returns the previous record if any.
    pub fn forget(&mut self, name: &str) -> Option<PluginTrustRecord> {
        self.records.plugins.remove(name)
    }

    /// Decide whether an install should reprompt. The rule (from the spec):
    /// same `(marketplace, version, manifest_sha256)` → skip; any change →
    /// reprompt.
    #[must_use]
    pub fn needs_prompt(
        &self,
        name: &str,
        marketplace: &str,
        version: &str,
        manifest_sha256: &str,
    ) -> bool {
        match self.records.plugins.get(name) {
            None => true,
            Some(rec) => {
                rec.marketplace != marketplace
                    || rec.version != version
                    || rec.manifest_sha256 != manifest_sha256
            }
        }
    }
}

fn read_json_or_default<T: serde::de::DeserializeOwned + Default>(
    path: &Path,
) -> Result<T, PluginError> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(T::default()),
        Err(source) => {
            return Err(PluginError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if raw.trim().is_empty() {
        return Ok(T::default());
    }
    serde_json::from_str(&raw).map_err(|source| PluginError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), PluginError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| PluginError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let body = serde_json::to_string_pretty(value).map_err(|source| PluginError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    std::fs::write(path, body).map_err(|source| PluginError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_persists_records() {
        let tmp = tempfile::TempDir::new().unwrap();
        let trust = tmp.path().join("plugins.json");
        let allow = tmp.path().join("marketplaces-allowlist.json");
        {
            let mut s = TrustStore::open(trust.clone(), allow.clone()).unwrap();
            s.approve_marketplace("https://m.example.com/index.json");
            s.record(
                "demo",
                PluginTrustRecord {
                    version: "1.0.0".into(),
                    marketplace: "https://m.example.com/index.json".into(),
                    manifest_sha256: "abc".into(),
                    installed_at: "2026-05-24T00:00:00Z".into(),
                },
            );
            s.save().unwrap();
        }
        let s2 = TrustStore::open(trust, allow).unwrap();
        assert!(s2.is_marketplace_approved("https://m.example.com/index.json"));
        assert_eq!(s2.get("demo").unwrap().version, "1.0.0");
    }

    #[test]
    fn needs_prompt_on_version_bump() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut s = TrustStore::open(
            tmp.path().join("plugins.json"),
            tmp.path().join("allow.json"),
        )
        .unwrap();
        s.record(
            "demo",
            PluginTrustRecord {
                version: "1.0.0".into(),
                marketplace: "https://m/index.json".into(),
                manifest_sha256: "abc".into(),
                installed_at: "now".into(),
            },
        );
        assert!(!s.needs_prompt("demo", "https://m/index.json", "1.0.0", "abc"));
        assert!(s.needs_prompt("demo", "https://m/index.json", "1.1.0", "abc"));
        assert!(s.needs_prompt("demo", "https://m/index.json", "1.0.0", "xyz"));
    }

    #[test]
    fn missing_files_open_with_defaults() {
        let tmp = tempfile::TempDir::new().unwrap();
        let s = TrustStore::open(
            tmp.path().join("does-not-exist.json"),
            tmp.path().join("nope.json"),
        )
        .unwrap();
        assert!(s.records.plugins.is_empty());
        assert!(s.allowlist.approved.is_empty());
    }
}
