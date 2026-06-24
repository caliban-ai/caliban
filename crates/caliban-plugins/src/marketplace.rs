//! Plugin marketplace client.
//!
//! v1 protocol: a marketplace is a single HTTP(S) URL serving a JSON
//! index. Each plugin entry lists one or more `versions` with a
//! `download_url` (tarball — `.tar.gz`) and a `sha256` digest. The client
//! fetches the index, downloads the tarball to a tmp file, verifies the
//! digest, extracts to `dest_root`, and writes a trust record.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::Digest as _;

use crate::error::PluginError;
use crate::manifest::PluginManifest;
use crate::trust::{PluginTrustRecord, TrustStore};

/// Top-level marketplace index. Field-compatible with the simpler spec
/// shape (`plugins: [{ name, version, … }]`); the richer
/// `versions: [{ … }]` form is also supported per the design doc.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Marketplace {
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// Marketplace URL (echoed back from the index for sanity-check).
    #[serde(default)]
    pub url: String,
    /// Plugin entries.
    #[serde(default)]
    pub plugins: Vec<MarketplaceEntry>,
}

/// One entry in the marketplace index. Either the flat single-version
/// form (`name`, `version`, `sha256`, `download_url`) or the richer
/// `versions[…]` form is accepted.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MarketplaceEntry {
    /// Plugin name.
    pub name: String,
    /// Short description.
    #[serde(default)]
    pub description: String,
    /// Source repository URL (optional).
    #[serde(default)]
    pub repository: Option<String>,
    // Flat form fields:
    /// Single-version: semver.
    #[serde(default)]
    pub version: Option<String>,
    /// Single-version: hex sha256.
    #[serde(default)]
    pub sha256: Option<String>,
    /// Single-version: tarball URL.
    #[serde(default)]
    pub download_url: Option<String>,
    // Richer form:
    /// Multi-version listing.
    #[serde(default)]
    pub versions: Vec<MarketplaceVersion>,
}

/// One version in the richer form.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MarketplaceVersion {
    /// Semver.
    pub version: String,
    /// Tarball URL (preferred field name).
    #[serde(alias = "download_url")]
    pub tarball: String,
    /// Hex sha256.
    pub sha256: String,
    /// Minimum caliban version.
    #[serde(default)]
    pub min_caliban: Option<String>,
}

impl MarketplaceEntry {
    /// Return the most-recent version (preferring `versions[]` over the
    /// flat form when both exist).
    #[must_use]
    pub fn latest_version(&self) -> Option<MarketplaceVersion> {
        if let Some(latest) = self.versions.last() {
            return Some(latest.clone());
        }
        match (&self.version, &self.sha256, &self.download_url) {
            (Some(v), Some(s), Some(u)) => Some(MarketplaceVersion {
                version: v.clone(),
                sha256: s.clone(),
                tarball: u.clone(),
                min_caliban: None,
            }),
            _ => None,
        }
    }
}

/// Settings that constrain marketplace operations.
#[derive(Debug, Clone, Default)]
pub struct MarketplaceSettings {
    /// When `Some`, only the listed marketplace URLs may be queried.
    /// Empty inner Vec disables marketplace installs entirely.
    pub strict_known: Option<Vec<String>>,
    /// Explicit block list (always rejected).
    pub blocked: Vec<String>,
}

impl MarketplaceSettings {
    /// Build settings from env vars.
    #[must_use]
    pub fn from_env() -> Self {
        let strict_known = std::env::var("CALIBAN_STRICT_KNOWN_MARKETPLACES")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(|s| {
                s.split(',')
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect()
            });
        let blocked = std::env::var("CALIBAN_BLOCKED_MARKETPLACES")
            .ok()
            .map(|s| {
                s.split(',')
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        Self {
            strict_known,
            blocked,
        }
    }

    /// Returns an error if the URL is rejected by the strict/block lists.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::BlockedMarketplace`] or
    /// [`PluginError::UnknownMarketplace`].
    pub fn check_url(&self, url: &str) -> Result<(), PluginError> {
        if self.blocked.iter().any(|u| u == url) {
            return Err(PluginError::BlockedMarketplace {
                url: url.to_string(),
            });
        }
        if let Some(allow) = self.strict_known.as_ref()
            && !allow.iter().any(|u| u == url)
        {
            return Err(PluginError::UnknownMarketplace {
                url: url.to_string(),
            });
        }
        Ok(())
    }
}

/// What the trust prompt should do. Used by the CLI to plumb interactive vs.
/// non-interactive behavior; the marketplace client itself is decision-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustDecision {
    /// Use the cached approval or, if absent, decline.
    UseCache,
    /// Force-approve (e.g. `--yes` flag).
    Approve,
}

/// Marketplace client. Holds a `reqwest::Client` and the trust + settings
/// state.
#[derive(Debug, Clone)]
pub struct MarketplaceClient {
    http: reqwest::Client,
    settings: MarketplaceSettings,
}

impl Default for MarketplaceClient {
    fn default() -> Self {
        // Untrusted plugin index/tarball downloads must run on the hardened
        // shared client (finite timeout, caliban user-agent, rustls) rather
        // than a bare `reqwest::Client::new()` with no request timeout. See #158.
        Self::new(
            caliban_common::http::default_client(),
            MarketplaceSettings::default(),
        )
    }
}

impl MarketplaceClient {
    /// Build a client over the given HTTP transport + settings.
    #[must_use]
    pub fn new(http: reqwest::Client, settings: MarketplaceSettings) -> Self {
        Self { http, settings }
    }

    /// Borrow the settings (so the CLI can query `strict_known`, etc.).
    #[must_use]
    pub fn settings(&self) -> &MarketplaceSettings {
        &self.settings
    }

    /// Fetch and parse the marketplace index at `url`.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::UnknownMarketplace`] / `BlockedMarketplace`
    /// when settings reject the URL, [`PluginError::Http`] on transport
    /// failures, and a parse error for malformed indices.
    pub async fn fetch_index(&self, url: &str) -> Result<Marketplace, PluginError> {
        self.settings.check_url(url)?;
        let resp = self.http.get(url).send().await?.error_for_status()?;
        let bytes = resp.bytes().await?;
        let parsed: Marketplace =
            serde_json::from_slice(&bytes).map_err(|source| PluginError::Parse {
                path: PathBuf::from(url),
                source,
            })?;
        Ok(parsed)
    }

    /// Install a plugin by name from a marketplace.
    ///
    /// Workflow:
    /// 1. Fetch the index and find `plugin_name`.
    /// 2. Resolve the latest version (or `desired_version` if provided).
    /// 3. Download the tarball to a tmp file; verify sha256.
    /// 4. Extract under `dest_root/<plugin_name>/`.
    /// 5. Parse the manifest and write a trust record.
    ///
    /// `decision` controls whether the operator has explicitly approved the
    /// install (e.g. CLI `--yes`); when `UseCache`, this method returns an
    /// error if the trust cache wouldn't admit the install — callers are
    /// expected to prompt the operator and retry with `Approve`.
    ///
    /// # Errors
    ///
    /// Returns variants of [`PluginError`] for marketplace / network /
    /// digest / extract / parse failures.
    #[allow(clippy::too_many_lines)]
    pub async fn install(
        &self,
        plugin_name: &str,
        marketplace_url: &str,
        desired_version: Option<&str>,
        dest_root: &Path,
        trust: &mut TrustStore,
        decision: TrustDecision,
    ) -> Result<PathBuf, PluginError> {
        let index = self.fetch_index(marketplace_url).await?;
        let entry = index
            .plugins
            .iter()
            .find(|e| e.name == plugin_name)
            .ok_or_else(|| PluginError::PluginNotFound {
                name: plugin_name.to_string(),
                url: marketplace_url.to_string(),
            })?;
        let version = match desired_version {
            Some(want) => {
                if let Some(v) = entry.versions.iter().find(|v| v.version == want) {
                    v.clone()
                } else if entry.version.as_deref() == Some(want) {
                    entry.latest_version().ok_or_else(|| PluginError::Invalid {
                        path: PathBuf::from(marketplace_url),
                        message: format!("no version metadata for plugin '{plugin_name}'"),
                    })?
                } else {
                    return Err(PluginError::Invalid {
                        path: PathBuf::from(marketplace_url),
                        message: format!(
                            "version '{want}' of plugin '{plugin_name}' not found in index"
                        ),
                    });
                }
            }
            None => entry.latest_version().ok_or_else(|| PluginError::Invalid {
                path: PathBuf::from(marketplace_url),
                message: format!("no version metadata for plugin '{plugin_name}'"),
            })?,
        };

        // 1. Download tarball.
        let resp = self
            .http
            .get(&version.tarball)
            .send()
            .await?
            .error_for_status()?;
        let body = resp.bytes().await?;

        // 2. Verify sha256.
        let mut hasher = sha2::Sha256::new();
        hasher.update(&body);
        let actual = hex::encode_lower(hasher.finalize());
        if actual != version.sha256.to_ascii_lowercase() {
            return Err(PluginError::Sha256Mismatch {
                name: plugin_name.to_string(),
                expected: version.sha256.clone(),
                actual,
            });
        }

        // 3. Extract.
        let tmp = tempfile::tempdir().map_err(|source| PluginError::Io {
            path: dest_root.to_path_buf(),
            source,
        })?;
        extract_targz(&body, tmp.path())?;
        let extracted_root = find_plugin_root(tmp.path(), plugin_name).ok_or_else(|| {
            PluginError::Extract(format!(
                "tarball for '{plugin_name}' did not contain plugin.json at <root>/{plugin_name}/plugin.json or <root>/plugin.json"
            ))
        })?;

        // Parse manifest to confirm matching name + version.
        let manifest_path = extracted_root.join("plugin.json");
        let manifest = PluginManifest::from_path(&manifest_path)?;
        manifest.check_name_matches_dir(&manifest_path)?;
        if manifest.name != plugin_name {
            return Err(PluginError::Invalid {
                path: manifest_path,
                message: format!(
                    "tarball name '{}' does not match expected '{plugin_name}'",
                    manifest.name
                ),
            });
        }
        if manifest.version != version.version {
            return Err(PluginError::Invalid {
                path: manifest_path.clone(),
                message: format!(
                    "tarball version '{}' does not match marketplace '{}'",
                    manifest.version, version.version
                ),
            });
        }

        // Manifest hash for trust record.
        let manifest_raw = std::fs::read(&manifest_path).map_err(|source| PluginError::Io {
            path: manifest_path.clone(),
            source,
        })?;
        let mut h = sha2::Sha256::new();
        h.update(&manifest_raw);
        let manifest_sha = hex::encode_lower(h.finalize());

        // 4. Trust gate.
        let needs_prompt = trust.needs_prompt(
            plugin_name,
            marketplace_url,
            &manifest.version,
            &manifest_sha,
        );
        if needs_prompt && decision == TrustDecision::UseCache {
            return Err(PluginError::Invalid {
                path: manifest_path,
                message: format!(
                    "plugin '{plugin_name}' from '{marketplace_url}' has not been approved (trust prompt required)"
                ),
            });
        }

        // 5. Copy into dest_root/<name>.
        let final_dir = dest_root.join(plugin_name);
        if final_dir.exists() {
            std::fs::remove_dir_all(&final_dir).map_err(|source| PluginError::Io {
                path: final_dir.clone(),
                source,
            })?;
        }
        copy_dir_recursive(&extracted_root, &final_dir)?;

        // 6. Record trust.
        trust.approve_marketplace(marketplace_url);
        trust.record(
            plugin_name,
            PluginTrustRecord {
                version: manifest.version.clone(),
                marketplace: marketplace_url.to_string(),
                manifest_sha256: manifest_sha,
                installed_at: now_rfc3339(),
            },
        );
        trust.save()?;

        Ok(final_dir)
    }
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn extract_targz(bytes: &[u8], dest: &Path) -> Result<(), PluginError> {
    let gz = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(gz);
    archive
        .unpack(dest)
        .map_err(|e| PluginError::Extract(format!("tar unpack: {e}")))?;
    Ok(())
}

fn find_plugin_root(tmp: &Path, plugin_name: &str) -> Option<PathBuf> {
    let direct = tmp.join("plugin.json");
    if direct.exists() {
        return Some(tmp.to_path_buf());
    }
    let nested = tmp.join(plugin_name).join("plugin.json");
    if nested.exists() {
        return Some(tmp.join(plugin_name));
    }
    // Fallback: scan one level for any subdir containing plugin.json.
    if let Ok(rd) = std::fs::read_dir(tmp) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() && p.join("plugin.json").exists() {
                return Some(p);
            }
        }
    }
    None
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), PluginError> {
    std::fs::create_dir_all(dst).map_err(|source| PluginError::Io {
        path: dst.to_path_buf(),
        source,
    })?;
    let rd = std::fs::read_dir(src).map_err(|source| PluginError::Io {
        path: src.to_path_buf(),
        source,
    })?;
    for entry in rd.flatten() {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let ft = entry.file_type().map_err(|source| PluginError::Io {
            path: from.clone(),
            source,
        })?;
        if ft.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if ft.is_file() {
            std::fs::copy(&from, &to).map_err(|source| PluginError::Io {
                path: to.clone(),
                source,
            })?;
        }
        // symlinks ignored — tarballs containing them would be a hostile-tarball risk anyway.
    }
    Ok(())
}

/// Local "hex" helper. Pulling in the `hex` crate for one function is
/// overkill; this matches `hex::encode` output for byte slices.
mod hex {
    pub(super) fn encode_lower<T: AsRef<[u8]>>(input: T) -> String {
        let bytes = input.as_ref();
        let mut out = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            out.push(nibble(b >> 4));
            out.push(nibble(b & 0x0f));
        }
        out
    }
    fn nibble(n: u8) -> char {
        match n {
            0..=9 => (b'0' + n) as char,
            10..=15 => (b'a' + (n - 10)) as char,
            _ => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_targz(plugin_name: &str, manifest: &str) -> Vec<u8> {
        // Build an in-memory .tar.gz with <plugin_name>/plugin.json.
        let mut out = Vec::new();
        {
            let enc = flate2::write::GzEncoder::new(&mut out, flate2::Compression::default());
            let mut builder = tar::Builder::new(enc);
            let mut header = tar::Header::new_gnu();
            header.set_size(manifest.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(
                    &mut header,
                    format!("{plugin_name}/plugin.json"),
                    manifest.as_bytes(),
                )
                .unwrap();
            builder.finish().unwrap();
        }
        out
    }

    fn sha256(bytes: &[u8]) -> String {
        let mut h = sha2::Sha256::new();
        h.update(bytes);
        hex::encode_lower(h.finalize())
    }

    #[test]
    fn marketplace_index_parses_minimal_entry() {
        let raw = r#"{
            "name": "test", "url": "https://m/index.json",
            "plugins": [
                { "name": "demo", "description": "d",
                  "version": "1.0.0", "sha256": "abc", "download_url": "https://m/d.tgz" }
            ]
        }"#;
        let m: Marketplace = serde_json::from_str(raw).unwrap();
        assert_eq!(m.plugins[0].name, "demo");
        let v = m.plugins[0].latest_version().unwrap();
        assert_eq!(v.version, "1.0.0");
        assert_eq!(v.sha256, "abc");
        assert_eq!(v.tarball, "https://m/d.tgz");
    }

    #[test]
    fn marketplace_index_parses_versions_form() {
        let raw = r#"{
            "name": "test", "url": "https://m/index.json",
            "plugins": [
                { "name": "demo",
                  "versions": [
                    { "version": "1.0.0", "tarball": "https://m/1.0.0.tgz", "sha256": "abc" }
                  ]
                }
            ]
        }"#;
        let m: Marketplace = serde_json::from_str(raw).unwrap();
        let v = m.plugins[0].latest_version().unwrap();
        assert_eq!(v.tarball, "https://m/1.0.0.tgz");
    }

    #[test]
    fn settings_blocks_url() {
        let s = MarketplaceSettings {
            strict_known: None,
            blocked: vec!["https://bad/idx".to_string()],
        };
        let err = s.check_url("https://bad/idx").unwrap_err();
        assert!(matches!(err, PluginError::BlockedMarketplace { .. }));
    }

    #[test]
    fn settings_rejects_unknown_when_strict() {
        let s = MarketplaceSettings {
            strict_known: Some(vec!["https://ok/idx".to_string()]),
            blocked: vec![],
        };
        let err = s.check_url("https://other/idx").unwrap_err();
        assert!(matches!(err, PluginError::UnknownMarketplace { .. }));
        assert!(s.check_url("https://ok/idx").is_ok());
    }

    #[test]
    fn settings_default_allows_any_url() {
        // Default settings (no strict list, no block list) admit any URL.
        let s = MarketplaceSettings::default();
        assert!(s.check_url("https://anything/idx").is_ok());
    }

    #[test]
    fn settings_blocked_takes_priority_over_strict_allow() {
        // A URL on both the allow list and the block list is still blocked.
        let s = MarketplaceSettings {
            strict_known: Some(vec!["https://x/idx".to_string()]),
            blocked: vec!["https://x/idx".to_string()],
        };
        let err = s.check_url("https://x/idx").unwrap_err();
        assert!(matches!(err, PluginError::BlockedMarketplace { .. }));
    }

    #[test]
    fn settings_empty_strict_list_rejects_everything() {
        // An empty strict_known Vec disables all marketplace installs.
        let s = MarketplaceSettings {
            strict_known: Some(vec![]),
            blocked: vec![],
        };
        let err = s.check_url("https://any/idx").unwrap_err();
        assert!(matches!(err, PluginError::UnknownMarketplace { .. }));
    }

    #[test]
    fn latest_version_none_when_no_metadata() {
        // Entry with neither versions[] nor a complete flat form yields None.
        let entry = MarketplaceEntry {
            name: "demo".into(),
            version: Some("1.0.0".into()),
            sha256: None,
            download_url: None,
            ..Default::default()
        };
        assert!(entry.latest_version().is_none());
    }

    #[test]
    fn latest_version_prefers_versions_list() {
        // When both forms exist, versions[] (last entry) wins over flat.
        let entry = MarketplaceEntry {
            name: "demo".into(),
            version: Some("1.0.0".into()),
            sha256: Some("flatsha".into()),
            download_url: Some("https://flat/d.tgz".into()),
            versions: vec![
                MarketplaceVersion {
                    version: "2.0.0".into(),
                    tarball: "https://m/2.0.0.tgz".into(),
                    sha256: "sha2".into(),
                    min_caliban: None,
                },
                MarketplaceVersion {
                    version: "3.0.0".into(),
                    tarball: "https://m/3.0.0.tgz".into(),
                    sha256: "sha3".into(),
                    min_caliban: Some("0.5".into()),
                },
            ],
            ..Default::default()
        };
        let v = entry.latest_version().unwrap();
        assert_eq!(v.version, "3.0.0");
        assert_eq!(v.sha256, "sha3");
    }

    #[test]
    fn marketplace_version_accepts_download_url_alias() {
        // The `tarball` field accepts a `download_url` alias in versions[].
        let raw = r#"{
            "version": "1.0.0",
            "download_url": "https://m/d.tgz",
            "sha256": "abc"
        }"#;
        let v: MarketplaceVersion = serde_json::from_str(raw).unwrap();
        assert_eq!(v.tarball, "https://m/d.tgz");
    }

    #[test]
    fn marketplace_defaults_on_empty_object() {
        // All top-level fields default; an empty object parses cleanly.
        let m: Marketplace = serde_json::from_str("{}").unwrap();
        assert!(m.name.is_empty());
        assert!(m.url.is_empty());
        assert!(m.plugins.is_empty());
    }

    #[test]
    fn marketplace_serialize_round_trips() {
        let m = Marketplace {
            name: "test".into(),
            url: "https://m/idx".into(),
            plugins: vec![MarketplaceEntry {
                name: "demo".into(),
                version: Some("1.0.0".into()),
                sha256: Some("abc".into()),
                download_url: Some("https://m/d.tgz".into()),
                ..Default::default()
            }],
        };
        let json = serde_json::to_string(&m).unwrap();
        let back: Marketplace = serde_json::from_str(&json).unwrap();
        assert_eq!(back.plugins[0].name, "demo");
        assert_eq!(back.plugins[0].latest_version().unwrap().version, "1.0.0");
    }

    #[test]
    fn trust_decision_equality() {
        assert_eq!(TrustDecision::Approve, TrustDecision::Approve);
        assert_ne!(TrustDecision::Approve, TrustDecision::UseCache);
    }

    #[test]
    fn client_default_exposes_default_settings() {
        let client = MarketplaceClient::default();
        // Default settings admit any URL (no strict list / block list).
        assert!(client.settings().check_url("https://any/idx").is_ok());
        assert!(client.settings().strict_known.is_none());
        assert!(client.settings().blocked.is_empty());
    }

    #[test]
    fn client_new_preserves_settings() {
        let settings = MarketplaceSettings {
            strict_known: Some(vec!["https://ok/idx".into()]),
            blocked: vec!["https://bad/idx".into()],
        };
        let client = MarketplaceClient::new(reqwest::Client::new(), settings);
        assert!(client.settings().check_url("https://ok/idx").is_ok());
        assert!(client.settings().check_url("https://bad/idx").is_err());
    }

    /// Regression guard for #158: the marketplace download path must enforce a
    /// finite request timeout (production `default()` now uses
    /// `caliban_common::http::default_client()`, which sets `DEFAULT_TIMEOUT`).
    /// `reqwest` doesn't expose the configured timeout for introspection, so we
    /// assert the *behavior*: an index server slower than the client timeout
    /// surfaces a transport error instead of hanging forever.
    #[tokio::test]
    async fn fetch_index_enforces_a_finite_timeout() {
        let server = wiremock::MockServer::start().await;
        let index_url = format!("{}/index.json", server.uri());
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/index.json"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_delay(std::time::Duration::from_secs(10))
                    .set_body_string("{}"),
            )
            .mount(&server)
            .await;

        // Build the client through the shared hardened factory, layering on a
        // short timeout so the test resolves quickly (mirrors how `default()`
        // gets its timeout, just shorter).
        let http = caliban_common::http::default_client_builder()
            .timeout(std::time::Duration::from_millis(150))
            .build()
            .unwrap();
        let client = MarketplaceClient::new(http, MarketplaceSettings::default());

        let err = client
            .fetch_index(&index_url)
            .await
            .expect_err("slow server should trip the request timeout");
        assert!(
            matches!(err, PluginError::Http(_)),
            "expected a transport (timeout) error, got {err:?}",
        );
    }

    #[tokio::test]
    async fn install_round_trips_and_writes_trust_record() {
        // Spin up a wiremock for both the index and the tarball.
        let manifest_body = r#"{ "name": "demo", "version": "1.0.0", "description": "test" }"#;
        let tarball = make_targz("demo", manifest_body);
        let sha = sha256(&tarball);

        let server = wiremock::MockServer::start().await;
        let tarball_url = format!("{}/demo-1.0.0.tar.gz", server.uri());
        let index_url = format!("{}/index.json", server.uri());
        let index_body = format!(
            r#"{{
                "name": "test", "url": "{index_url}",
                "plugins": [
                    {{ "name": "demo", "description": "d",
                       "version": "1.0.0", "sha256": "{sha}",
                       "download_url": "{tarball_url}" }}
                ]
            }}"#
        );

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/index.json"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(index_body))
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/demo-1.0.0.tar.gz"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_bytes(tarball.clone()))
            .mount(&server)
            .await;

        let tmp = tempfile::TempDir::new().unwrap();
        let dest_root = tmp.path().join("install");
        fs::create_dir_all(&dest_root).unwrap();
        let mut trust = TrustStore::open(
            tmp.path().join("plugins.json"),
            tmp.path().join("allow.json"),
        )
        .unwrap();
        let settings = MarketplaceSettings {
            strict_known: Some(vec![index_url.clone()]),
            blocked: vec![],
        };
        let client = MarketplaceClient::new(reqwest::Client::new(), settings);
        let path = client
            .install(
                "demo",
                &index_url,
                None,
                &dest_root,
                &mut trust,
                TrustDecision::Approve,
            )
            .await
            .unwrap();
        assert!(path.join("plugin.json").exists());
        let rec = trust.get("demo").unwrap();
        assert_eq!(rec.version, "1.0.0");
        assert_eq!(rec.marketplace, index_url);
        assert!(trust.is_marketplace_approved(&index_url));
    }

    #[tokio::test]
    async fn install_rejects_sha_mismatch() {
        let manifest_body = r#"{ "name": "demo", "version": "1.0.0", "description": "test" }"#;
        let tarball = make_targz("demo", manifest_body);

        let server = wiremock::MockServer::start().await;
        let tarball_url = format!("{}/demo-1.0.0.tar.gz", server.uri());
        let index_url = format!("{}/index.json", server.uri());
        let index_body = format!(
            r#"{{
                "name": "test", "url": "{index_url}",
                "plugins": [
                    {{ "name": "demo",
                       "version": "1.0.0",
                       "sha256": "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
                       "download_url": "{tarball_url}" }}
                ]
            }}"#
        );

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/index.json"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(index_body))
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/demo-1.0.0.tar.gz"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_bytes(tarball))
            .mount(&server)
            .await;

        let tmp = tempfile::TempDir::new().unwrap();
        let dest_root = tmp.path().join("install");
        fs::create_dir_all(&dest_root).unwrap();
        let mut trust = TrustStore::open(
            tmp.path().join("plugins.json"),
            tmp.path().join("allow.json"),
        )
        .unwrap();
        let settings = MarketplaceSettings {
            strict_known: Some(vec![index_url.clone()]),
            blocked: vec![],
        };
        let client = MarketplaceClient::new(reqwest::Client::new(), settings);
        let err = client
            .install(
                "demo",
                &index_url,
                None,
                &dest_root,
                &mut trust,
                TrustDecision::Approve,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, PluginError::Sha256Mismatch { .. }));
        assert!(trust.get("demo").is_none());
    }

    #[tokio::test]
    async fn install_blocks_marketplace() {
        let client = MarketplaceClient::new(
            reqwest::Client::new(),
            MarketplaceSettings {
                strict_known: None,
                blocked: vec!["https://evil/idx".into()],
            },
        );
        let err = client.fetch_index("https://evil/idx").await.unwrap_err();
        assert!(matches!(err, PluginError::BlockedMarketplace { .. }));
    }

    #[tokio::test]
    async fn install_caches_trust_skipping_prompt_on_reinstall() {
        let manifest_body = r#"{ "name": "demo", "version": "1.0.0", "description": "test" }"#;
        let tarball = make_targz("demo", manifest_body);
        let sha = sha256(&tarball);
        let server = wiremock::MockServer::start().await;
        let tarball_url = format!("{}/demo-1.0.0.tar.gz", server.uri());
        let index_url = format!("{}/index.json", server.uri());
        let index_body = format!(
            r#"{{
                "name": "test", "url": "{index_url}",
                "plugins": [
                    {{ "name": "demo", "version": "1.0.0", "sha256": "{sha}",
                       "download_url": "{tarball_url}" }}
                ]
            }}"#
        );
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/index.json"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(index_body))
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/demo-1.0.0.tar.gz"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_bytes(tarball.clone()))
            .mount(&server)
            .await;

        let tmp = tempfile::TempDir::new().unwrap();
        let dest_root = tmp.path().join("install");
        fs::create_dir_all(&dest_root).unwrap();
        let mut trust = TrustStore::open(
            tmp.path().join("plugins.json"),
            tmp.path().join("allow.json"),
        )
        .unwrap();
        let settings = MarketplaceSettings {
            strict_known: Some(vec![index_url.clone()]),
            blocked: vec![],
        };
        let client = MarketplaceClient::new(reqwest::Client::new(), settings);
        client
            .install(
                "demo",
                &index_url,
                None,
                &dest_root,
                &mut trust,
                TrustDecision::Approve,
            )
            .await
            .unwrap();

        // Reinstall with UseCache should succeed (no prompt needed).
        let result = client
            .install(
                "demo",
                &index_url,
                None,
                &dest_root,
                &mut trust,
                TrustDecision::UseCache,
            )
            .await;
        assert!(result.is_ok(), "expected reinstall to be admitted by cache");
    }
}
