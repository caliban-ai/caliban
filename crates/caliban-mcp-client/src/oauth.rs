//! `McpOAuthFlow` — PKCE + loopback OAuth flow for hosted MCP servers.
//!
//! Implements RFC 8252 (loopback redirect URI) + RFC 7636 (PKCE) + RFC
//! 8414 (`/.well-known/oauth-authorization-server`) + the MCP-flavoured
//! `/.well-known/oauth-protected-resource` discovery doc.
//!
//! The flow is structured so the moving pieces are independently
//! testable: discovery, the loopback callback server, token persistence,
//! and refresh each live behind a small trait or pure function.
//!
//! See `docs/superpowers/specs/2026-05-24-mcp-v2-design.md` (OAuth
//! section) and ADR 0023.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    Router,
    extract::{Query, State},
    response::Html,
    routing::get,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, oneshot};
use url::Url;

use crate::config::OauthMode;
use crate::error::McpError;

/// Default OAuth keyring service identifier.
pub const KEYRING_SERVICE: &str = "caliban-mcp";

/// Env-var override for the loopback callback port.
pub const PORT_ENV_VAR: &str = "CALIBAN_MCP_OAUTH_PORT";

/// Refresh window — when `expires_at - now < REFRESH_MARGIN` we refresh
/// before issuing the next request rather than risk a 401 mid-call.
pub const REFRESH_MARGIN: Duration = Duration::from_mins(1);

// ---------------------------------------------------------------------------
// Public data types
// ---------------------------------------------------------------------------

/// Manually-configured OAuth endpoints (per-server `[server.X.oauth]`
/// block). Used in `oauth = "manual"` mode; `auto` discovers these from
/// the server's well-known documents.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ManualOauthConfig {
    /// Client identifier registered with the auth server.
    #[serde(default)]
    pub client_id: Option<String>,
    /// Client secret (optional — PKCE flows are typically public).
    #[serde(default)]
    pub client_secret: Option<String>,
    /// Authorization endpoint URL.
    #[serde(default)]
    pub auth_url: Option<String>,
    /// Token endpoint URL.
    #[serde(default)]
    pub token_url: Option<String>,
    /// Scopes to request (space-joined when sent).
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Optional explicit `audience` claim (RFC 8707).
    #[serde(default)]
    pub audience: Option<String>,
}

/// Resolved OAuth endpoints, regardless of whether discovery or manual
/// config produced them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OauthEndpoints {
    /// Authorization endpoint.
    pub auth_url: Url,
    /// Token endpoint.
    pub token_url: Url,
    /// Scopes the server advertises (auto) or the operator chose (manual).
    pub scopes: Vec<String>,
    /// Resource audience (for token cache keying + the `audience` claim).
    pub audience: String,
    /// RFC 7591 dynamic-client-registration endpoint, when the authorization
    /// server advertises one. Present via discovery (`auto`); `None` for manual
    /// config and for servers that don't offer registration (e.g. GitHub).
    pub registration_endpoint: Option<Url>,
}

/// One persisted token bundle. Stored under `keyring("caliban-mcp", "<server>")`
/// as JSON (keyed by server alone — see [`account_key`] — so a warm-cache
/// lookup can precede endpoint discovery).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct OauthTokens {
    /// Access token (Bearer).
    pub access_token: String,
    /// Refresh token; `None` if the auth server didn't return one.
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Absolute expiry; `None` means "never told us, treat as long-lived".
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    /// Scopes the auth server actually granted (may be a subset).
    #[serde(default)]
    pub scopes: Vec<String>,
    /// The `client_id` these tokens were issued to. Persisted so a later
    /// refresh uses the *same* client the `refresh_token` is bound to —
    /// critical for `oauth = "auto"`, where the `client_id` came from
    /// config/registration and isn't otherwise recoverable at refresh time.
    /// `None` for tokens written before this field existed (serde default).
    #[serde(default)]
    pub client_id: Option<String>,
    /// The resolved resource `audience` these tokens were issued for. Persisted
    /// so a warm-cache reuse can return without re-running discovery (#333 M8),
    /// and carried as the `audience` claim on refresh.
    #[serde(default)]
    pub audience: String,
    /// The `client_secret`, when the client is confidential — a static manual
    /// secret or one returned by RFC 7591 dynamic registration. Persisted so a
    /// DCR-issued confidential client can silently refresh instead of forcing a
    /// fresh browser flow every expiry (#333 M9). `None` for a public client.
    #[serde(default)]
    pub client_secret: Option<String>,
    /// Absolute expiry of `client_secret` (RFC 7591 `client_secret_expires_at`),
    /// when the registration reported one. `None` = no secret, or one that does
    /// not expire. Persisted for observability / future re-registration.
    #[serde(default)]
    pub client_secret_expires_at: Option<DateTime<Utc>>,
}

impl OauthTokens {
    /// `true` when the token is missing or `< REFRESH_MARGIN` from expiry.
    #[must_use]
    pub fn needs_refresh(&self, now: DateTime<Utc>) -> bool {
        match self.expires_at {
            None => false,
            Some(exp) => {
                let margin = chrono::Duration::from_std(REFRESH_MARGIN).unwrap_or_default();
                exp - margin <= now
            }
        }
    }
}

/// PKCE pair (verifier + S256 challenge). Stored on the flow so the
/// callback handler can submit the verifier alongside the auth code.
#[derive(Debug, Clone)]
pub struct PkcePair {
    /// 43-128 char random string (RFC 7636 §4.1).
    pub verifier: String,
    /// `base64url(sha256(verifier))` — S256 challenge.
    pub challenge: String,
}

impl PkcePair {
    /// Generate a fresh PKCE pair using `OsRng`. 32 random bytes →
    /// `base64url` → 43-char verifier.
    #[must_use]
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let verifier = URL_SAFE_NO_PAD.encode(bytes);
        let mut h = Sha256::new();
        h.update(verifier.as_bytes());
        let challenge = URL_SAFE_NO_PAD.encode(h.finalize());
        Self {
            verifier,
            challenge,
        }
    }
}

// ---------------------------------------------------------------------------
// Token storage
// ---------------------------------------------------------------------------

/// Pluggable token persistence. Production uses `KeyringStore` (or
/// `FileStore` when the OS keyring is unavailable); tests use
/// `MemoryStore` for isolation.
pub trait TokenStore: Send + Sync + std::fmt::Debug {
    /// Look up tokens for `(server, audience)`. Returns `Ok(None)` when no
    /// entry exists.
    ///
    /// # Errors
    /// Implementation-defined.
    fn get(&self, server: &str, audience: &str) -> Result<Option<OauthTokens>, McpError>;
    /// Persist tokens for `(server, audience)`.
    ///
    /// # Errors
    /// Implementation-defined.
    fn put(&self, server: &str, audience: &str, tokens: &OauthTokens) -> Result<(), McpError>;
    /// Forget tokens for `(server, audience)` — called on 401.
    ///
    /// # Errors
    /// Implementation-defined.
    fn clear(&self, server: &str, audience: &str) -> Result<(), McpError>;
}

/// Cache key for a server's token bundle. Keyed by `server` **alone** (the
/// `audience` argument is retained for API compatibility but ignored): the
/// resolved audience is only knowable after endpoint discovery, so keying by it
/// would force a discovery round-trip before any cache lookup — defeating warm
/// reuse offline / when the `.well-known` host is down (#333 M8). A server maps
/// to a single MCP resource audience, so `server` is a sufficient key; the
/// audience travels inside the stored [`OauthTokens`].
fn account_key(server: &str, _audience: &str) -> String {
    server.to_string()
}

/// In-memory store — used by tests and as a fallback when nothing else
/// works.
#[derive(Debug, Default)]
pub struct MemoryStore {
    inner: Mutex<std::collections::BTreeMap<String, OauthTokens>>,
}

impl TokenStore for MemoryStore {
    fn get(&self, server: &str, audience: &str) -> Result<Option<OauthTokens>, McpError> {
        let inner = self
            .inner
            .try_lock()
            .map_err(|e| McpError::TokenStore(e.to_string()))?;
        Ok(inner.get(&account_key(server, audience)).cloned())
    }
    fn put(&self, server: &str, audience: &str, tokens: &OauthTokens) -> Result<(), McpError> {
        let mut inner = self
            .inner
            .try_lock()
            .map_err(|e| McpError::TokenStore(e.to_string()))?;
        inner.insert(account_key(server, audience), tokens.clone());
        Ok(())
    }
    fn clear(&self, server: &str, audience: &str) -> Result<(), McpError> {
        let mut inner = self
            .inner
            .try_lock()
            .map_err(|e| McpError::TokenStore(e.to_string()))?;
        inner.remove(&account_key(server, audience));
        Ok(())
    }
}

/// Keyring-backed store (`keyring::Entry` per server+audience).
#[derive(Debug, Default)]
pub struct KeyringStore;

impl TokenStore for KeyringStore {
    fn get(&self, server: &str, audience: &str) -> Result<Option<OauthTokens>, McpError> {
        let entry =
            keyring::Entry::new(KEYRING_SERVICE, &account_key(server, audience)).map_err(|e| {
                McpError::Keyring {
                    server: server.to_string(),
                    source: e,
                }
            })?;
        match entry.get_password() {
            Ok(s) => Ok(Some(serde_json::from_str(&s).map_err(|e| {
                McpError::TokenStore(format!("malformed keyring entry: {e}"))
            })?)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(McpError::Keyring {
                server: server.to_string(),
                source: e,
            }),
        }
    }
    fn put(&self, server: &str, audience: &str, tokens: &OauthTokens) -> Result<(), McpError> {
        let entry =
            keyring::Entry::new(KEYRING_SERVICE, &account_key(server, audience)).map_err(|e| {
                McpError::Keyring {
                    server: server.to_string(),
                    source: e,
                }
            })?;
        let json = serde_json::to_string(tokens)
            .map_err(|e| McpError::TokenStore(format!("serialize tokens: {e}")))?;
        entry.set_password(&json).map_err(|e| McpError::Keyring {
            server: server.to_string(),
            source: e,
        })
    }
    fn clear(&self, server: &str, audience: &str) -> Result<(), McpError> {
        let entry =
            keyring::Entry::new(KEYRING_SERVICE, &account_key(server, audience)).map_err(|e| {
                McpError::Keyring {
                    server: server.to_string(),
                    source: e,
                }
            })?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(McpError::Keyring {
                server: server.to_string(),
                source: e,
            }),
        }
    }
}

/// File-based fallback (`$XDG_DATA_HOME/caliban/mcp-tokens.json` mode 0600).
/// Used on systems where `keyring` returns `PlatformFailure` (CI containers,
/// servers without a logged-in user, etc.).
#[derive(Debug, Clone)]
pub struct FileStore {
    path: std::path::PathBuf,
}

impl FileStore {
    /// Build a store rooted at `path`. The parent directory is created
    /// lazily on first `put`.
    #[must_use]
    pub fn new(path: std::path::PathBuf) -> Self {
        Self { path }
    }

    /// Default location: `$XDG_DATA_HOME/caliban/mcp-tokens.json`, or
    /// `~/.local/share/caliban/mcp-tokens.json` if XDG isn't set.
    #[must_use]
    pub fn default_path() -> std::path::PathBuf {
        if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
            std::path::PathBuf::from(xdg)
                .join("caliban")
                .join("mcp-tokens.json")
        } else if let Some(home) = dirs::home_dir() {
            home.join(".local")
                .join("share")
                .join("caliban")
                .join("mcp-tokens.json")
        } else {
            std::path::PathBuf::from("./mcp-tokens.json")
        }
    }

    fn load_all(&self) -> Result<std::collections::BTreeMap<String, OauthTokens>, McpError> {
        match std::fs::read_to_string(&self.path) {
            Ok(s) if s.trim().is_empty() => Ok(std::collections::BTreeMap::new()),
            Ok(s) => serde_json::from_str(&s).map_err(|e| {
                McpError::TokenStore(format!("malformed token file {}: {e}", self.path.display()))
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok(std::collections::BTreeMap::new())
            }
            Err(e) => Err(McpError::TokenStore(format!(
                "read {}: {e}",
                self.path.display()
            ))),
        }
    }

    fn save_all(
        &self,
        all: &std::collections::BTreeMap<String, OauthTokens>,
    ) -> Result<(), McpError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| McpError::TokenStore(format!("mkdir {}: {e}", parent.display())))?;
        }
        let body = serde_json::to_string_pretty(all)
            .map_err(|e| McpError::TokenStore(format!("serialize: {e}")))?;
        write_mode_0600(&self.path, &body)
    }
}

#[cfg(unix)]
fn write_mode_0600(path: &std::path::Path, body: &str) -> Result<(), McpError> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| McpError::TokenStore(format!("open {}: {e}", path.display())))?;
    f.write_all(body.as_bytes())
        .map_err(|e| McpError::TokenStore(format!("write {}: {e}", path.display())))?;
    Ok(())
}

#[cfg(not(unix))]
fn write_mode_0600(path: &std::path::Path, body: &str) -> Result<(), McpError> {
    std::fs::write(path, body)
        .map_err(|e| McpError::TokenStore(format!("write {}: {e}", path.display())))
}

impl TokenStore for FileStore {
    fn get(&self, server: &str, audience: &str) -> Result<Option<OauthTokens>, McpError> {
        Ok(self
            .load_all()?
            .get(&account_key(server, audience))
            .cloned())
    }
    fn put(&self, server: &str, audience: &str, tokens: &OauthTokens) -> Result<(), McpError> {
        let mut all = self.load_all()?;
        all.insert(account_key(server, audience), tokens.clone());
        self.save_all(&all)
    }
    fn clear(&self, server: &str, audience: &str) -> Result<(), McpError> {
        let mut all = self.load_all()?;
        all.remove(&account_key(server, audience));
        self.save_all(&all)
    }
}

/// Build the production store: `KeyringStore` if functional, else
/// `FileStore` (and log a warning).
#[must_use]
pub fn default_store() -> Arc<dyn TokenStore> {
    // Probe the keyring with a dummy lookup; on any platform failure fall
    // back to the file store.
    let probe = keyring::Entry::new(KEYRING_SERVICE, "__probe__");
    match probe.and_then(|e| match e.get_password() {
        Ok(_) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(other) => Err(other),
    }) {
        Ok(()) => Arc::new(KeyringStore),
        Err(e) => {
            tracing::warn!(
                target: caliban_common::tracing_targets::TARGET_MCP_OAUTH,
                error = %e,
                "OS keyring unavailable; falling back to file-based token store",
            );
            Arc::new(FileStore::new(FileStore::default_path()))
        }
    }
}

// ---------------------------------------------------------------------------
// Discovery — `/.well-known/oauth-protected-resource` + RFC 8414
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ProtectedResourceDoc {
    #[serde(default)]
    authorization_servers: Vec<String>,
    #[serde(default)]
    resource: Option<String>,
    /// Scopes required to access this resource (RFC 9728). For many hosted
    /// servers (GitHub included) this is the authoritative scope list — the
    /// authorization server's own metadata leaves `scopes_supported` null.
    /// `Option` because servers emit an explicit `null` (not just omission),
    /// which a bare `Vec` would fail to deserialize.
    #[serde(default)]
    scopes_supported: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct AuthServerDoc {
    authorization_endpoint: String,
    token_endpoint: String,
    /// `Option` because auth servers (GitHub) emit an explicit `null` here,
    /// which a bare `Vec` would reject.
    #[serde(default)]
    scopes_supported: Option<Vec<String>>,
    /// RFC 7591 registration endpoint. `null` on servers without DCR (GitHub).
    #[serde(default)]
    registration_endpoint: Option<String>,
}

/// Discover endpoints for `server_url`. Hits the MCP-flavoured
/// `/.well-known/oauth-protected-resource` first, follows
/// `authorization_servers[0]`, then GETs that AS's RFC 8414 doc.
///
/// `audience` falls back to `server_url` minus path when the resource
/// metadata doesn't supply one explicitly.
///
/// # Errors
/// [`McpError::OauthDiscovery`] on any HTTP / JSON / URL-shape failure.
pub async fn discover_endpoints(
    server: &str,
    server_url: &Url,
    client: &reqwest::Client,
) -> Result<OauthEndpoints, McpError> {
    let prs_url = join_wellknown(server_url, "oauth-protected-resource");
    let prs: ProtectedResourceDoc = client
        .get(prs_url.clone())
        .send()
        .await
        .map_err(|e| McpError::OauthDiscovery {
            server: server.to_string(),
            message: format!("GET {prs_url}: {e}"),
        })?
        .error_for_status()
        .map_err(|e| McpError::OauthDiscovery {
            server: server.to_string(),
            message: format!("GET {prs_url}: {e}"),
        })?
        .json()
        .await
        .map_err(|e| McpError::OauthDiscovery {
            server: server.to_string(),
            message: format!("parse {prs_url}: {e}"),
        })?;
    let audience = prs.resource.unwrap_or_else(|| {
        let mut u = server_url.clone();
        u.set_path("");
        u.to_string()
    });
    // The protected-resource doc's scopes (RFC 9728) are the scopes the
    // *resource* needs — the right thing to request. The AS metadata's
    // `scopes_supported` is often null (GitHub), so prefer the resource doc and
    // fall back to the AS list only if the resource didn't advertise any.
    let resource_scopes = prs.scopes_supported.unwrap_or_default();
    let as_url_raw = prs
        .authorization_servers
        .into_iter()
        .next()
        .ok_or_else(|| McpError::OauthDiscovery {
            server: server.to_string(),
            message: "oauth-protected-resource has no authorization_servers".to_string(),
        })?;
    let auth_server_url = Url::parse(&as_url_raw).map_err(|e| McpError::OauthDiscovery {
        server: server.to_string(),
        message: format!("invalid authorization_servers entry '{as_url_raw}': {e}"),
    })?;
    let asd_url = join_wellknown(&auth_server_url, "oauth-authorization-server");
    let asd: AuthServerDoc = client
        .get(asd_url.clone())
        .send()
        .await
        .map_err(|e| McpError::OauthDiscovery {
            server: server.to_string(),
            message: format!("GET {asd_url}: {e}"),
        })?
        .error_for_status()
        .map_err(|e| McpError::OauthDiscovery {
            server: server.to_string(),
            message: format!("GET {asd_url}: {e}"),
        })?
        .json()
        .await
        .map_err(|e| McpError::OauthDiscovery {
            server: server.to_string(),
            message: format!("parse {asd_url}: {e}"),
        })?;
    Ok(OauthEndpoints {
        auth_url: Url::parse(&asd.authorization_endpoint).map_err(|e| {
            McpError::OauthDiscovery {
                server: server.to_string(),
                message: format!("invalid authorization_endpoint: {e}"),
            }
        })?,
        token_url: Url::parse(&asd.token_endpoint).map_err(|e| McpError::OauthDiscovery {
            server: server.to_string(),
            message: format!("invalid token_endpoint: {e}"),
        })?,
        scopes: if resource_scopes.is_empty() {
            asd.scopes_supported.unwrap_or_default()
        } else {
            resource_scopes
        },
        audience,
        // RFC 7591: carry the registration endpoint so `auto` can dynamically
        // register a client when no static `client_id` was configured. A
        // malformed URL here is non-fatal — treat it as "no DCR available".
        registration_endpoint: asd
            .registration_endpoint
            .as_deref()
            .and_then(|s| Url::parse(s).ok()),
    })
}

/// Build a `.well-known` metadata URL from an issuer/resource URL.
///
/// RFC 8414 §3.1 (auth-server metadata) and RFC 9728 §3.1 (protected-resource
/// metadata) both insert `/.well-known/{suffix}` **between the host and the
/// issuer path** — the path is preserved, not replaced. So
/// `https://github.com/login/oauth` → `https://github.com/.well-known/
/// oauth-authorization-server/login/oauth`, and a root-path issuer
/// `https://example.com/` → `https://example.com/.well-known/{suffix}`.
///
/// (The earlier implementation replaced the whole path, which 404s against any
/// issuer/resource served under a sub-path — e.g. GitHub's
/// `github.com/login/oauth` and `api.githubcopilot.com/mcp/`.)
fn join_wellknown(base: &Url, suffix: &str) -> Url {
    let mut u = base.clone();
    // Preserve the issuer/resource path segment(s), dropping only a trailing
    // slash so we don't emit `//` or a dangling separator.
    let issuer_path = base.path().trim_end_matches('/');
    if issuer_path.is_empty() {
        u.set_path(&format!("/.well-known/{suffix}"));
    } else {
        u.set_path(&format!("/.well-known/{suffix}{issuer_path}"));
    }
    u.set_query(None);
    u.set_fragment(None);
    u
}

/// Build endpoints from a `ManualOauthConfig` block. Validates that the
/// required fields are present.
///
/// # Errors
/// [`McpError::OauthManualIncomplete`] if `client_id`, `auth_url`, or
/// `token_url` is missing.
pub fn endpoints_from_manual(
    server: &str,
    cfg: &ManualOauthConfig,
    server_url: &Url,
) -> Result<OauthEndpoints, McpError> {
    let auth_url_raw = cfg
        .auth_url
        .as_deref()
        .ok_or_else(|| McpError::OauthManualIncomplete {
            server: server.to_string(),
            field: "auth_url",
        })?;
    let token_url_raw =
        cfg.token_url
            .as_deref()
            .ok_or_else(|| McpError::OauthManualIncomplete {
                server: server.to_string(),
                field: "token_url",
            })?;
    if cfg.client_id.is_none() {
        return Err(McpError::OauthManualIncomplete {
            server: server.to_string(),
            field: "client_id",
        });
    }
    let audience = cfg.audience.clone().unwrap_or_else(|| {
        let mut u = server_url.clone();
        u.set_path("");
        u.to_string()
    });
    Ok(OauthEndpoints {
        auth_url: Url::parse(auth_url_raw).map_err(|e| McpError::OauthDiscovery {
            server: server.to_string(),
            message: format!("invalid manual auth_url: {e}"),
        })?,
        token_url: Url::parse(token_url_raw).map_err(|e| McpError::OauthDiscovery {
            server: server.to_string(),
            message: format!("invalid manual token_url: {e}"),
        })?,
        scopes: cfg.scopes.clone(),
        audience,
        registration_endpoint: None,
    })
}

// ---------------------------------------------------------------------------
// Dynamic Client Registration (RFC 7591)
// ---------------------------------------------------------------------------

/// A client registered dynamically with an authorization server (RFC 7591).
/// caliban registers a public PKCE client, so `client_secret` is usually
/// `None`; a server that issues a confidential client returns one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredClient {
    /// The issued client identifier.
    pub client_id: String,
    /// Client secret, when the server issued a confidential client.
    pub client_secret: Option<String>,
    /// Absolute expiry of `client_secret` (RFC 7591 `client_secret_expires_at`),
    /// when the server reported a non-zero one. `None` = no secret or a secret
    /// that does not expire.
    pub client_secret_expires_at: Option<DateTime<Utc>>,
}

/// Register a public (PKCE) client with an RFC 7591 `registration_endpoint`.
///
/// Used by `oauth = "auto"` when no static `client_id` was configured and the
/// authorization server advertises dynamic registration. Registers `client_name`
/// with the loopback `redirect_uri` the flow will use, requesting a public
/// client (`token_endpoint_auth_method: "none"`) for `authorization_code` +
/// `refresh_token`.
///
/// # Errors
/// [`McpError::OauthRegistration`] on any HTTP / JSON failure or a non-success
/// status from the registration endpoint.
pub async fn register_client(
    server: &str,
    registration_endpoint: &Url,
    redirect_uri: &Url,
    client_name: &str,
    http: &reqwest::Client,
) -> Result<RegisteredClient, McpError> {
    let body = serde_json::json!({
        "client_name": client_name,
        "redirect_uris": [redirect_uri.as_str()],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        // Public client — PKCE only, no client secret.
        "token_endpoint_auth_method": "none",
    });
    let response = http
        .post(registration_endpoint.clone())
        .header(reqwest::header::ACCEPT, "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| McpError::OauthRegistration {
            server: server.to_string(),
            message: format!("POST {registration_endpoint}: {e}"),
        })?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(McpError::OauthRegistration {
            server: server.to_string(),
            message: format!("registration endpoint returned {status}: {text}"),
        });
    }
    let doc: ClientRegistrationResponse =
        response
            .json()
            .await
            .map_err(|e| McpError::OauthRegistration {
                server: server.to_string(),
                message: format!("malformed registration response: {e}"),
            })?;
    // RFC 7591: `client_secret_expires_at` is Unix seconds; 0 means "never".
    let client_secret_expires_at = doc
        .client_secret_expires_at
        .filter(|&secs| secs > 0)
        .and_then(|secs| DateTime::<Utc>::from_timestamp(secs, 0));
    Ok(RegisteredClient {
        client_id: doc.client_id,
        client_secret: doc.client_secret,
        client_secret_expires_at,
    })
}

#[derive(Debug, Deserialize)]
#[allow(clippy::struct_field_names)] // RFC 7591 field names are all `client_*`.
struct ClientRegistrationResponse {
    client_id: String,
    #[serde(default)]
    client_secret: Option<String>,
    /// RFC 7591: absolute expiry of the secret as Unix seconds (0 = never).
    #[serde(default)]
    client_secret_expires_at: Option<i64>,
}

// ---------------------------------------------------------------------------
// Authorization-code + PKCE flow over loopback
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// Sender half of the callback's "deliver the authorization code" channel.
/// Wrapped in `Mutex<Option<_>>` because axum handlers hold a non-`!Send`
/// reference and `oneshot::Sender::send` consumes `self`, so we move it
/// out of the mutex on first call.
type CallbackSender = Arc<Mutex<Option<oneshot::Sender<Result<String, McpError>>>>>;

#[derive(Clone)]
struct CallbackState {
    expected_state: String,
    tx: CallbackSender,
    server: String,
}

async fn callback_handler(
    State(state): State<CallbackState>,
    Query(params): Query<CallbackParams>,
) -> Html<&'static str> {
    let result = if let Some(err) = params.error {
        let desc = params.error_description.unwrap_or_default();
        Err(McpError::OauthFlow {
            server: state.server.clone(),
            message: format!("auth server returned error '{err}': {desc}"),
        })
    } else if params.state.as_deref() != Some(state.expected_state.as_str()) {
        Err(McpError::OauthFlow {
            server: state.server.clone(),
            message: "callback state mismatch".to_string(),
        })
    } else if let Some(code) = params.code {
        Ok(code)
    } else {
        Err(McpError::OauthFlow {
            server: state.server.clone(),
            message: "callback missing both code and error".to_string(),
        })
    };
    if let Some(tx) = state.tx.lock().await.take() {
        let _ = tx.send(result);
    }
    Html(
        "<html><body><h1>caliban</h1>\
         <p>Authorization complete. You can close this tab and return to the terminal.</p>\
         </body></html>",
    )
}

/// Options driving one OAuth run. `port = None` picks an ephemeral port
/// (random); env var [`PORT_ENV_VAR`] overrides when set; the CLI flag
/// `--mcp-oauth-port` should override the env var (caller's job).
#[derive(Debug, Clone)]
pub struct OauthFlowOptions {
    /// Server name (matches `mcp.toml` table key).
    pub server: String,
    /// Endpoints — discovered or from manual config.
    pub endpoints: OauthEndpoints,
    /// Public client identifier.
    pub client_id: String,
    /// Optional client secret. Most native flows are public (PKCE only).
    pub client_secret: Option<String>,
    /// Loopback port; `None` → ephemeral (`127.0.0.1:0`).
    pub port: Option<u16>,
    /// Hard cap on time we'll wait for the callback.
    pub callback_timeout: Duration,
}

impl OauthFlowOptions {
    /// Default 5-minute callback timeout, ephemeral port.
    #[must_use]
    pub fn new(server: String, endpoints: OauthEndpoints, client_id: String) -> Self {
        Self {
            server,
            endpoints,
            client_id,
            client_secret: None,
            port: None,
            callback_timeout: Duration::from_mins(5),
        }
    }
}

/// One in-progress authorization run. The caller is responsible for
/// presenting `auth_url` to the user (the TUI prints it; tests just open
/// it via `reqwest`).
pub struct OauthFlow {
    /// URL the user should visit in their browser.
    pub auth_url: Url,
    /// Server side state — the caller awaits `await_callback` to get the
    /// tokens once the user finishes authenticating.
    inner: OauthFlowInner,
}

struct OauthFlowInner {
    server: String,
    endpoints: OauthEndpoints,
    client_id: String,
    client_secret: Option<String>,
    redirect_uri: Url,
    pkce: PkcePair,
    #[allow(dead_code, reason = "preserved for diagnostics + future re-binding")]
    expected_state: String,
    code_rx: oneshot::Receiver<Result<String, McpError>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    callback_timeout: Duration,
}

impl std::fmt::Debug for OauthFlow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OauthFlow")
            .field("auth_url", &self.auth_url)
            .field("server", &self.inner.server)
            .finish_non_exhaustive()
    }
}

/// Bind the loopback callback listener and derive its `redirect_uri`.
///
/// `port = Some(p)` pins the port (required when the `redirect_uri` must match
/// a fixed registration — GitHub OAuth Apps, or a DCR registration we're about
/// to submit); `None` binds an ephemeral port. Returning the bound listener
/// (rather than just the port) lets a caller learn the `redirect_uri` — e.g. to
/// dynamically register a client for it — before handing the *same* listener to
/// [`OauthFlow::start_on_listener`], with no unbind/rebind race.
///
/// # Errors
/// [`McpError::OauthFlow`] if the listener can't bind.
pub(crate) async fn bind_loopback(
    server: &str,
    port: Option<u16>,
) -> Result<(tokio::net::TcpListener, Url), McpError> {
    let bind_addr = SocketAddr::from(([127, 0, 0, 1], port.unwrap_or(0)));
    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .map_err(|e| McpError::OauthFlow {
            server: server.to_string(),
            message: format!("bind loopback {bind_addr}: {e}"),
        })?;
    let local = listener.local_addr().map_err(|e| McpError::OauthFlow {
        server: server.to_string(),
        message: format!("local_addr: {e}"),
    })?;
    let redirect_uri =
        Url::parse(&format!("http://127.0.0.1:{}/callback", local.port())).map_err(|e| {
            McpError::OauthFlow {
                server: server.to_string(),
                message: format!("redirect uri parse: {e}"),
            }
        })?;
    Ok((listener, redirect_uri))
}

impl OauthFlow {
    /// Spawn the loopback callback server and build the auth URL, binding the
    /// port from `opts.port` (ephemeral when `None`).
    ///
    /// # Errors
    /// [`McpError::OauthFlow`] if the loopback listener fails to bind.
    pub async fn start(opts: OauthFlowOptions) -> Result<Self, McpError> {
        let (listener, redirect_uri) = bind_loopback(&opts.server, opts.port).await?;
        Ok(Self::start_on_listener(opts, listener, redirect_uri))
    }

    /// Like [`Self::start`] but drives an already-bound loopback `listener` +
    /// its `redirect_uri`. Used when the caller needed the `redirect_uri` before
    /// starting the flow (e.g. to dynamically register a client for it). Must be
    /// called from within a Tokio runtime (it spawns the callback server).
    #[must_use]
    pub(crate) fn start_on_listener(
        opts: OauthFlowOptions,
        listener: tokio::net::TcpListener,
        redirect_uri: Url,
    ) -> Self {
        let pkce = PkcePair::generate();
        let mut state_bytes = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut state_bytes);
        let expected_state = URL_SAFE_NO_PAD.encode(state_bytes);

        let (code_tx, code_rx) = oneshot::channel::<Result<String, McpError>>();
        let cb_state = CallbackState {
            expected_state: expected_state.clone(),
            tx: Arc::new(Mutex::new(Some(code_tx))),
            server: opts.server.clone(),
        };
        let app = Router::new()
            .route("/callback", get(callback_handler))
            .with_state(cb_state);
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        tokio::spawn(async move {
            let server = axum::serve(listener, app);
            tokio::select! {
                res = server => {
                    if let Err(e) = res {
                        tracing::warn!(target: caliban_common::tracing_targets::TARGET_MCP_OAUTH, error = %e, "callback server error");
                    }
                }
                _ = shutdown_rx => {}
            }
        });

        let mut auth_url = opts.endpoints.auth_url.clone();
        {
            let scopes_joined = opts.endpoints.scopes.join(" ");
            let mut qp = auth_url.query_pairs_mut();
            qp.append_pair("response_type", "code");
            qp.append_pair("client_id", &opts.client_id);
            qp.append_pair("redirect_uri", redirect_uri.as_str());
            qp.append_pair("code_challenge", &pkce.challenge);
            qp.append_pair("code_challenge_method", "S256");
            qp.append_pair("state", &expected_state);
            if !scopes_joined.is_empty() {
                qp.append_pair("scope", &scopes_joined);
            }
            if !opts.endpoints.audience.is_empty() {
                qp.append_pair("audience", &opts.endpoints.audience);
            }
        }

        Self {
            auth_url,
            inner: OauthFlowInner {
                server: opts.server,
                endpoints: opts.endpoints,
                client_id: opts.client_id,
                client_secret: opts.client_secret,
                redirect_uri,
                pkce,
                expected_state,
                code_rx,
                shutdown_tx: Some(shutdown_tx),
                callback_timeout: opts.callback_timeout,
            },
        }
    }

    /// Wait for the user's browser callback, then exchange the
    /// authorization code for tokens.
    ///
    /// # Errors
    /// - [`McpError::OauthFlow`] on cancellation, timeout, or callback
    ///   error.
    /// - [`McpError::OauthExchange`] if the token-endpoint POST fails.
    pub async fn await_callback(self, http: &reqwest::Client) -> Result<OauthTokens, McpError> {
        let OauthFlowInner {
            server,
            endpoints,
            client_id,
            client_secret,
            redirect_uri,
            pkce,
            expected_state: _,
            code_rx,
            shutdown_tx,
            callback_timeout,
        } = self.inner;
        let mut shutdown_tx = shutdown_tx;
        let code = match tokio::time::timeout(callback_timeout, code_rx).await {
            Ok(Ok(Ok(code))) => code,
            Ok(Ok(Err(e))) => {
                if let Some(tx) = shutdown_tx.take() {
                    let _ = tx.send(());
                }
                return Err(e);
            }
            Ok(Err(_)) => {
                return Err(McpError::OauthFlow {
                    server,
                    message: "callback channel dropped".to_string(),
                });
            }
            Err(_) => {
                if let Some(tx) = shutdown_tx.take() {
                    let _ = tx.send(());
                }
                return Err(McpError::OauthFlow {
                    server,
                    message: format!("callback timed out after {callback_timeout:?}"),
                });
            }
        };
        if let Some(tx) = shutdown_tx.take() {
            let _ = tx.send(());
        }
        exchange_code(
            http,
            ExchangeArgs {
                server: &server,
                endpoints: &endpoints,
                client_id: &client_id,
                client_secret: client_secret.as_deref(),
                redirect_uri: &redirect_uri,
                pkce: &pkce,
            },
            code,
        )
        .await
    }

    /// Cancel the flow (user hit Esc / closed the modal). Shuts the
    /// callback server down.
    pub fn cancel(mut self) {
        if let Some(tx) = self.inner.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

struct ExchangeArgs<'a> {
    server: &'a str,
    endpoints: &'a OauthEndpoints,
    client_id: &'a str,
    client_secret: Option<&'a str>,
    redirect_uri: &'a Url,
    pkce: &'a PkcePair,
}

async fn exchange_code(
    http: &reqwest::Client,
    args: ExchangeArgs<'_>,
    code: String,
) -> Result<OauthTokens, McpError> {
    let mut form: Vec<(&str, String)> = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code),
        ("redirect_uri", args.redirect_uri.to_string()),
        ("client_id", args.client_id.to_string()),
        ("code_verifier", args.pkce.verifier.clone()),
    ];
    if let Some(secret) = args.client_secret {
        form.push(("client_secret", secret.to_string()));
    }
    parse_token_response(
        http.post(args.endpoints.token_url.clone())
            // GitHub's token endpoint returns form-encoded by default; force
            // JSON so `parse_token_response` can decode it. Spec-compliant
            // servers already return JSON, so this is harmless there.
            .header(reqwest::header::ACCEPT, "application/json")
            .form(&form)
            .send()
            .await
            .map_err(|e| McpError::OauthExchange {
                server: args.server.to_string(),
                message: e.to_string(),
            })?,
        args.server,
    )
    .await
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    /// Optional so we can distinguish a real error body (which omits it) from a
    /// genuine success — GitHub returns HTTP 200 even for token errors.
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    scope: Option<String>,
    // OAuth error fields (RFC 6749 §5.2). GitHub reports token-exchange
    // failures as HTTP 200 with these set, not a 4xx.
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

async fn parse_token_response(
    response: reqwest::Response,
    server: &str,
) -> Result<OauthTokens, McpError> {
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(McpError::OauthExchange {
            server: server.to_string(),
            message: format!("token endpoint returned {status}: {body}"),
        });
    }
    let body: TokenResponse = response.json().await.map_err(|e| McpError::OauthExchange {
        server: server.to_string(),
        message: format!("malformed token response: {e}"),
    })?;
    // GitHub (and RFC 6749 §5.2) can report failure with HTTP 200 + an `error`
    // field. Surface it clearly instead of a confusing "missing access_token".
    if let Some(err) = body.error {
        let desc = body.error_description.unwrap_or_default();
        return Err(McpError::OauthExchange {
            server: server.to_string(),
            message: format!("token endpoint error '{err}': {desc}"),
        });
    }
    let access_token = body.access_token.ok_or_else(|| McpError::OauthExchange {
        server: server.to_string(),
        message: "token response had neither access_token nor error".to_string(),
    })?;
    let expires_at = body
        .expires_in
        .map(|secs| Utc::now() + chrono::Duration::seconds(secs));
    let scopes = body
        .scope
        .map(|s| s.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();
    Ok(OauthTokens {
        access_token,
        refresh_token: body.refresh_token,
        expires_at,
        scopes,
        // Set by the caller (`await_callback` / `refresh_tokens` / the
        // authenticator) which knows the client identity + audience; the token
        // endpoint doesn't echo them back.
        client_id: None,
        audience: String::new(),
        client_secret: None,
        client_secret_expires_at: None,
    })
}

/// Refresh an existing token bundle. Returns the *new* tokens (the
/// caller should persist them).
///
/// # Errors
/// [`McpError::OauthExchange`] if the refresh POST fails, or if the
/// bundle has no `refresh_token`.
pub async fn refresh_tokens(
    http: &reqwest::Client,
    server: &str,
    endpoints: &OauthEndpoints,
    client_id: &str,
    client_secret: Option<&str>,
    tokens: &OauthTokens,
) -> Result<OauthTokens, McpError> {
    let refresh = tokens
        .refresh_token
        .as_deref()
        .ok_or_else(|| McpError::OauthExchange {
            server: server.to_string(),
            message: "no refresh_token available; full re-auth required".to_string(),
        })?;
    let mut form: Vec<(&str, String)> = vec![
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", refresh.to_string()),
        ("client_id", client_id.to_string()),
    ];
    if let Some(s) = client_secret {
        form.push(("client_secret", s.to_string()));
    }
    if !endpoints.scopes.is_empty() {
        form.push(("scope", endpoints.scopes.join(" ")));
    }
    let response = http
        .post(endpoints.token_url.clone())
        // Force JSON — GitHub's token endpoint otherwise returns form-encoded.
        .header(reqwest::header::ACCEPT, "application/json")
        .form(&form)
        .send()
        .await
        .map_err(|e| McpError::OauthExchange {
            server: server.to_string(),
            message: e.to_string(),
        })?;
    let mut new = parse_token_response(response, server).await?;
    if new.refresh_token.is_none() {
        // Auth servers often omit the refresh_token from refresh
        // responses; preserve the original so subsequent refreshes
        // still work.
        new.refresh_token.clone_from(&tokens.refresh_token);
    }
    // Preserve the client identity + audience so the *next* refresh still
    // targets the right client with the right secret (the token endpoint never
    // echoes these back). #333: without carrying the client_secret forward, a
    // confidential DCR client would lose it after the first refresh.
    new.client_id = Some(client_id.to_string());
    new.client_secret = client_secret.map(str::to_string);
    new.client_secret_expires_at = tokens.client_secret_expires_at;
    new.audience.clone_from(&endpoints.audience);
    Ok(new)
}

// ---------------------------------------------------------------------------
// Connect-path orchestrator
// ---------------------------------------------------------------------------

/// Resolves a Bearer access token for one remote MCP server, driving the full
/// OAuth lifecycle: reuse a cached token, silently refresh a near-expiry one,
/// or — on a cold cache — run the interactive PKCE browser flow and persist the
/// result. Shared across every server in a single startup pass so the reqwest
/// client and token store are built once.
///
/// This is the piece that was missing from ADR 0023 Phase C: the flow building
/// blocks above (`discover_endpoints`, `OauthFlow`, `refresh_tokens`, the token
/// store) all existed but nothing wired them into the connect path, so
/// `oauth = "auto"`/`"manual"` servers silently failed to authenticate.
#[derive(Clone)]
pub struct OauthAuthenticator {
    http: reqwest::Client,
    store: Arc<dyn TokenStore>,
    /// Whether we may open a browser and block on the loopback callback. False
    /// in headless/`--print`/non-TTY runs — a cold cache then errors with
    /// [`McpError::OauthInteractiveRequired`] instead of hanging forever.
    interactive: bool,
    /// Fixed loopback callback port (`--mcp-oauth-port` / [`PORT_ENV_VAR`]).
    /// `None` → ephemeral (`127.0.0.1:0`). A fixed port is required for
    /// authorization servers that match the `redirect_uri` exactly (e.g. GitHub
    /// OAuth Apps, whose registered callback URL pins host + port).
    callback_port: Option<u16>,
}

impl std::fmt::Debug for OauthAuthenticator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OauthAuthenticator")
            .field("interactive", &self.interactive)
            .field("callback_port", &self.callback_port)
            .finish_non_exhaustive()
    }
}

impl OauthAuthenticator {
    /// Build an authenticator over an HTTP client, a token store, the
    /// interactivity of the current run, and an optional fixed callback port.
    #[must_use]
    pub fn new(
        http: reqwest::Client,
        store: Arc<dyn TokenStore>,
        interactive: bool,
        callback_port: Option<u16>,
    ) -> Self {
        Self {
            http,
            store,
            interactive,
            callback_port,
        }
    }

    /// Resolve a Bearer token for `server`.
    ///
    /// Returns `Ok(None)` for `OauthMode::Off` (nothing to attach). Otherwise
    /// returns `Ok(Some(access_token))`, running whichever of {reuse, refresh,
    /// interactive flow} is required. Endpoints are discovered (`auto`) or read
    /// from the manual block (`manual`); the `client_id` comes from the config
    /// block when set, otherwise (`auto`) from RFC 7591 dynamic registration
    /// when the auth server advertises a `registration_endpoint`.
    ///
    /// # Errors
    /// - [`McpError::OauthDiscovery`] / [`McpError::OauthManualIncomplete`] if
    ///   endpoints can't be resolved.
    /// - [`McpError::OauthNoClientId`] on a cold cache with no configured
    ///   `client_id` and no dynamic registration available.
    /// - [`McpError::OauthRegistration`] if dynamic registration fails.
    /// - [`McpError::OauthInteractiveRequired`] on a cold cache in headless mode.
    /// - [`McpError::OauthFlow`] / [`McpError::OauthExchange`] if the flow fails.
    pub async fn bearer_for(
        &self,
        server: &str,
        mode: OauthMode,
        server_url: &Url,
        manual: &ManualOauthConfig,
    ) -> Result<Option<String>, McpError> {
        if matches!(mode, OauthMode::Off) {
            return Ok(None);
        }
        let now = Utc::now();

        // 1. Warm-cache fast path (#333 M8): return a still-valid token WITHOUT
        //    endpoint discovery. For `auto`, discovery is two round-trips whose
        //    only purpose here would be to recompute the cache key; keying by
        //    server (see `account_key`) lets a valid on-disk token be reused
        //    offline / when the `.well-known` host is down.
        let cached = self.store.get(server, "")?;
        if let Some(existing) = &cached
            && !existing.needs_refresh(now)
        {
            tracing::debug!(
                target: caliban_common::tracing_targets::TARGET_MCP_OAUTH,
                server,
                "reusing cached oauth token (no discovery)",
            );
            return Ok(Some(existing.access_token.clone()));
        }

        // 2. Need endpoints — to refresh a near-expiry token or run a cold
        //    interactive flow. Resolve them now (discovery for `auto`). Explicit
        //    `oauth_config.scopes` always wins so an operator can narrow/correct
        //    what discovery inferred.
        let mut endpoints = match mode {
            OauthMode::Off => unreachable!("Off handled above"),
            OauthMode::Auto => discover_endpoints(server, server_url, &self.http).await?,
            OauthMode::Manual => endpoints_from_manual(server, manual, server_url)?,
        };
        if !manual.scopes.is_empty() {
            endpoints.scopes = manual.scopes.clone();
        }

        // 3. Silent refresh of a near-expiry cached token, preferring the
        //    client_secret the token was issued with (#333 M9) over a manual
        //    one — a DCR-issued confidential client has no manual secret.
        if let Some(existing) = &cached
            && existing.refresh_token.is_some()
            && let Some(client_id) = existing
                .client_id
                .clone()
                .or_else(|| manual.client_id.clone())
        {
            let secret = existing
                .client_secret
                .as_deref()
                .or(manual.client_secret.as_deref());
            match refresh_tokens(&self.http, server, &endpoints, &client_id, secret, existing).await
            {
                Ok(refreshed) => {
                    self.store.put(server, "", &refreshed)?;
                    tracing::info!(
                        target: caliban_common::tracing_targets::TARGET_MCP_OAUTH,
                        server,
                        "refreshed oauth token",
                    );
                    return Ok(Some(refreshed.access_token));
                }
                Err(e) => {
                    tracing::warn!(
                        target: caliban_common::tracing_targets::TARGET_MCP_OAUTH,
                        server,
                        error = %e,
                        "oauth token refresh failed; re-authenticating interactively",
                    );
                }
            }
        }

        // 4. Cold cache — fresh interactive authorization.
        self.interactive_authorize(server, &endpoints, manual).await
    }

    /// Cold-cache path: obtain a `client_id` (static config or RFC 7591 dynamic
    /// registration), run the interactive browser flow, and cache the token.
    ///
    /// # Errors
    /// - [`McpError::OauthNoClientId`] when neither a configured `client_id` nor
    ///   a `registration_endpoint` is available (server unconfigurable — GitHub).
    /// - [`McpError::OauthInteractiveRequired`] in headless mode.
    /// - [`McpError::OauthRegistration`] / [`McpError::OauthFlow`] /
    ///   [`McpError::OauthExchange`] if registration or the flow fails.
    async fn interactive_authorize(
        &self,
        server: &str,
        endpoints: &OauthEndpoints,
        manual: &ManualOauthConfig,
    ) -> Result<Option<String>, McpError> {
        // A `client_id` can come from static config OR (RFC 7591) dynamic
        // registration when the auth server advertises a `registration_endpoint`.
        // Resolve the source up front so neither branch has to panic-unwrap.
        enum Source {
            Static(String, Option<String>),
            Dcr(Url),
        }
        let source = if let Some(cid) = &manual.client_id {
            Source::Static(cid.clone(), manual.client_secret.clone())
        } else if let Some(reg_url) = &endpoints.registration_endpoint {
            Source::Dcr(reg_url.clone())
        } else {
            // Neither configured nor registrable (e.g. GitHub, no DCR).
            return Err(McpError::OauthNoClientId {
                server: server.to_string(),
            });
        };
        // Refuse to open a browser we can't complete in headless mode — check
        // BEFORE spending a dynamic registration.
        if !self.interactive {
            return Err(McpError::OauthInteractiveRequired {
                server: server.to_string(),
            });
        }

        // Bind the callback listener first so we know the exact `redirect_uri`
        // to register (DCR) and to present in the auth URL. Holding the bound
        // listener across registration avoids any unbind/rebind race.
        let (listener, redirect_uri) = bind_loopback(server, self.callback_port).await?;
        let (client_id, client_secret, secret_expires_at) = match source {
            Source::Static(cid, secret) => (cid, secret, None),
            Source::Dcr(reg_url) => {
                let registered =
                    register_client(server, &reg_url, &redirect_uri, "caliban", &self.http).await?;
                tracing::info!(
                    target: caliban_common::tracing_targets::TARGET_MCP_OAUTH,
                    server,
                    client_id = registered.client_id.as_str(),
                    "registered oauth client dynamically (RFC 7591)",
                );
                (
                    registered.client_id,
                    registered.client_secret,
                    registered.client_secret_expires_at,
                )
            }
        };

        let mut opts =
            OauthFlowOptions::new(server.to_string(), endpoints.clone(), client_id.clone());
        opts.client_secret = client_secret.clone();
        let flow = OauthFlow::start_on_listener(opts, listener, redirect_uri);
        present_auth_url(server, &flow.auth_url);
        let mut tokens = flow.await_callback(&self.http).await?;
        tokens.client_id = Some(client_id);
        // Persist the client identity + audience so a later refresh needs no
        // discovery and a confidential DCR client can refresh silently (#333).
        tokens.client_secret = client_secret;
        tokens.client_secret_expires_at = secret_expires_at;
        tokens.audience.clone_from(&endpoints.audience);
        self.store.put(server, "", &tokens)?;
        tracing::info!(
            target: caliban_common::tracing_targets::TARGET_MCP_OAUTH,
            server,
            "oauth authorization complete; token cached",
        );
        Ok(Some(tokens.access_token))
    }
}

/// Present the authorization URL to the operator: print it prominently (this
/// runs during startup, before the TUI takes the terminal) and make a
/// best-effort attempt to open the system browser. Printing is the source of
/// truth — the loopback callback captures the code regardless of whether the
/// auto-open succeeded.
fn present_auth_url(server: &str, auth_url: &Url) {
    // `eprintln!` (not the TUI): start_mcp runs before the alternate screen is
    // entered, so this lands on the normal terminal.
    eprintln!(
        "\n\u{1f510} caliban: MCP server '{server}' needs authorization.\n   \
         Opening your browser. If it doesn't open, visit:\n\n   {auth_url}\n"
    );
    let opened = open_in_browser(auth_url.as_str());
    if !opened {
        tracing::debug!(
            target: caliban_common::tracing_targets::TARGET_MCP_OAUTH,
            server,
            "could not auto-open a browser; user must open the printed URL manually",
        );
    }
}

/// Best-effort system-browser open, dependency-free. Returns `true` if the
/// launcher process spawned (not that the page actually rendered).
fn open_in_browser(url: &str) -> bool {
    let (cmd, args) = browser_argv(std::env::consts::OS, url);
    std::process::Command::new(cmd)
        .args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}

/// Build the launcher argv for opening `url` in the default browser on `os`
/// (pass `std::env::consts::OS`). The URL is always its own argv entry — never
/// interpolated into a shell command string.
///
/// On Windows this uses `rundll32 url.dll,FileProtocolHandler <url>`, **not**
/// `cmd /C start "" <url>`: `cmd.exe` treats the `&` query separators in an
/// OAuth authorization URL as command separators and truncates it (#338).
/// `rundll32` receives the URL directly, so there is no shell to re-parse it.
fn browser_argv(os: &str, url: &str) -> (&'static str, Vec<String>) {
    match os {
        "macos" => ("open", vec![url.to_string()]),
        "windows" => (
            "rundll32.exe",
            vec!["url.dll,FileProtocolHandler".to_string(), url.to_string()],
        ),
        // Linux / *BSD / other unix.
        _ => ("xdg-open", vec![url.to_string()]),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_argv_passes_url_as_single_arg_intact() {
        // The auth URL has multiple `&` separators — it must survive as one
        // argv entry on every platform (#338).
        let url = "https://auth.example/authorize?client_id=abc&scope=read+write&state=xyz&code_challenge=q";
        for os in ["macos", "windows", "linux", "freebsd"] {
            let (_cmd, args) = browser_argv(os, url);
            assert_eq!(
                args.last().map(String::as_str),
                Some(url),
                "url must be the final, intact arg on {os}: {args:?}"
            );
        }
    }

    #[test]
    fn browser_argv_windows_avoids_cmd_shell() {
        // rundll32 receives the URL directly; `cmd /C start` would let cmd.exe
        // parse the `&` separators and truncate the URL (#338).
        let url = "https://auth.example/authorize?a=1&b=2";
        let (cmd, args) = browser_argv("windows", url);
        assert_eq!(cmd, "rundll32.exe");
        assert!(
            !args.iter().any(|a| a == "start" || a == "/C"),
            "must not route through cmd.exe: {args:?}"
        );
        assert_eq!(args.last().unwrap(), url);
    }

    #[test]
    fn pkce_pair_length_and_charset() {
        let p = PkcePair::generate();
        assert_eq!(p.verifier.len(), 43, "verifier should be 43 chars");
        for c in p.verifier.chars() {
            assert!(
                c.is_ascii_alphanumeric() || c == '-' || c == '_',
                "non-base64url char in verifier: {c}",
            );
        }
        // Recompute challenge for sanity.
        let mut h = sha2::Sha256::new();
        h.update(p.verifier.as_bytes());
        let expected = URL_SAFE_NO_PAD.encode(h.finalize());
        assert_eq!(p.challenge, expected);
    }

    #[test]
    fn needs_refresh_no_expiry_returns_false() {
        let t = OauthTokens {
            access_token: "x".to_string(),
            refresh_token: None,
            expires_at: None,
            scopes: vec![],
            client_id: None,
            ..Default::default()
        };
        assert!(!t.needs_refresh(Utc::now()));
    }

    #[test]
    fn needs_refresh_near_expiry_true() {
        let now = Utc::now();
        let t = OauthTokens {
            access_token: "x".to_string(),
            refresh_token: Some("r".to_string()),
            expires_at: Some(now + chrono::Duration::seconds(10)),
            scopes: vec![],
            client_id: None,
            ..Default::default()
        };
        assert!(t.needs_refresh(now));
    }

    #[test]
    fn needs_refresh_far_from_expiry_false() {
        let now = Utc::now();
        let t = OauthTokens {
            access_token: "x".to_string(),
            refresh_token: Some("r".to_string()),
            expires_at: Some(now + chrono::Duration::seconds(3600)),
            scopes: vec![],
            client_id: None,
            ..Default::default()
        };
        assert!(!t.needs_refresh(now));
    }

    #[test]
    fn memory_store_round_trip() {
        let store = MemoryStore::default();
        let tokens = OauthTokens {
            access_token: "a".to_string(),
            refresh_token: Some("r".to_string()),
            expires_at: None,
            scopes: vec!["read".to_string()],
            client_id: None,
            ..Default::default()
        };
        store.put("svc", "aud", &tokens).expect("put");
        let got = store.get("svc", "aud").expect("get").expect("some");
        assert_eq!(got.access_token, "a");
        assert_eq!(got.refresh_token.as_deref(), Some("r"));
        assert_eq!(got.scopes, vec!["read".to_string()]);
        store.clear("svc", "aud").expect("clear");
        assert!(store.get("svc", "aud").expect("get").is_none());
    }

    #[test]
    fn file_store_round_trip() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("tokens.json");
        let store = FileStore::new(path.clone());
        let tokens = OauthTokens {
            access_token: "atok".to_string(),
            refresh_token: None,
            expires_at: None,
            scopes: vec![],
            client_id: None,
            ..Default::default()
        };
        store.put("svc", "aud", &tokens).expect("put");
        // File exists and is non-empty.
        let meta = std::fs::metadata(&path).expect("metadata");
        assert!(meta.len() > 0);
        let got = store.get("svc", "aud").expect("get").expect("some");
        assert_eq!(got.access_token, "atok");
        store.clear("svc", "aud").expect("clear");
        assert!(store.get("svc", "aud").expect("get").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn file_store_writes_mode_0600() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("tokens.json");
        let store = FileStore::new(path.clone());
        store
            .put(
                "svc",
                "aud",
                &OauthTokens {
                    access_token: "x".to_string(),
                    refresh_token: None,
                    expires_at: None,
                    scopes: vec![],
                    client_id: None,
                    ..Default::default()
                },
            )
            .expect("put");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600, got {mode:o}");
    }

    #[test]
    fn join_wellknown_preserves_issuer_path() {
        // RFC 8414 / RFC 9728: the issuer/resource path is preserved after the
        // well-known suffix (path-preserving), NOT replaced. This is the case
        // GitHub relies on — a stripped path 404s.
        let base = Url::parse("https://github.com/login/oauth").unwrap();
        let u = join_wellknown(&base, "oauth-authorization-server");
        assert_eq!(
            u.to_string(),
            "https://github.com/.well-known/oauth-authorization-server/login/oauth"
        );
    }

    #[test]
    fn join_wellknown_trims_trailing_slash() {
        // A trailing slash on the resource URL must not leak into the metadata
        // path (would yield a `//` or dangling separator).
        let base = Url::parse("https://api.githubcopilot.com/mcp/").unwrap();
        let u = join_wellknown(&base, "oauth-protected-resource");
        assert_eq!(
            u.to_string(),
            "https://api.githubcopilot.com/.well-known/oauth-protected-resource/mcp"
        );
    }

    #[test]
    fn join_wellknown_root_path_has_no_suffix_path() {
        // A root-path issuer collapses to the bare well-known URL.
        let base = Url::parse("https://example.com/").unwrap();
        let u = join_wellknown(&base, "oauth-authorization-server");
        assert_eq!(
            u.to_string(),
            "https://example.com/.well-known/oauth-authorization-server"
        );
    }

    #[test]
    fn manual_requires_client_id() {
        let cfg = ManualOauthConfig {
            client_id: None,
            client_secret: None,
            auth_url: Some("https://x/auth".to_string()),
            token_url: Some("https://x/token".to_string()),
            scopes: vec![],
            audience: None,
        };
        let server_url = Url::parse("https://x/mcp").unwrap();
        let err = endpoints_from_manual("s", &cfg, &server_url).unwrap_err();
        assert!(matches!(
            err,
            McpError::OauthManualIncomplete {
                field: "client_id",
                ..
            }
        ));
    }

    #[test]
    fn manual_endpoints_built() {
        let cfg = ManualOauthConfig {
            client_id: Some("cid".to_string()),
            client_secret: None,
            auth_url: Some("https://auth/x".to_string()),
            token_url: Some("https://auth/t".to_string()),
            scopes: vec!["read".to_string()],
            audience: Some("aud".to_string()),
        };
        let server_url = Url::parse("https://x/mcp").unwrap();
        let endpoints = endpoints_from_manual("s", &cfg, &server_url).unwrap();
        assert_eq!(endpoints.audience, "aud");
        assert_eq!(endpoints.scopes, vec!["read".to_string()]);
    }
}
