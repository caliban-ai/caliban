//! Errors emitted by the MCP client crate.

use std::path::PathBuf;
use std::time::Duration;

/// Errors emitted by the MCP client crate.
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum McpError {
    /// IO failure reading a config file (other than `NotFound`, which is
    /// silently treated as "no config").
    #[error("mcp: io error reading {path}: {source}")]
    Io {
        /// Path that failed to read.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// TOML parse error.
    #[error("mcp: config parse error in {path}: {source}")]
    ConfigParse {
        /// Path that failed to parse.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: toml::de::Error,
    },
    /// Server key doesn't match `[a-z0-9_-]{1,32}`.
    #[error("mcp: invalid server name '{0}' (must match [a-z0-9_-]{{1,32}})")]
    InvalidServerName(String),
    /// `${VAR}` substitution found no value in the process env.
    #[error("mcp: env var '{var}' referenced by server '{server}' is not set")]
    MissingEnv {
        /// Server whose env table referenced the missing variable.
        server: String,
        /// Variable name that was missing.
        var: String,
    },
    /// `${VAR}` was used inline (e.g. `"prefix-${VAR}-suffix"`). v1 only
    /// supports full-value substitution.
    #[error(
        "mcp: server '{server}' env['{key}'] uses unsupported inline interpolation; only \"${{VAR}}\" full-value substitution is allowed in v1"
    )]
    InlineInterpolation {
        /// Server whose env value was malformed.
        server: String,
        /// Env-table key whose value was malformed.
        key: String,
    },
    /// Spawning a server's command failed.
    #[error("mcp: server '{server}' failed to spawn: {source}")]
    Spawn {
        /// Server that failed.
        server: String,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },
    /// `initialize` handshake did not complete within `timeout`.
    #[error("mcp: server '{server}' handshake timed out after {timeout:?}")]
    HandshakeTimeout {
        /// Server that timed out.
        server: String,
        /// Configured timeout.
        timeout: Duration,
    },
    /// rmcp's initialize handshake returned an error (transport closed, peer
    /// returned a JSON-RPC error, malformed init response, etc.).
    #[error("mcp: server '{server}' handshake failed: {message}")]
    Handshake {
        /// Server that failed.
        server: String,
        /// Stringified rmcp error.
        message: String,
    },
    /// rmcp `Peer::list_tools` / `call_tool` returned an error.
    #[error("mcp: server '{server}' rpc error: {message}")]
    Rpc {
        /// Server name.
        server: String,
        /// Stringified rmcp service error.
        message: String,
    },
    /// An in-flight tool call was cancelled by the agent's cancellation token.
    #[error("mcp: server '{server}' tool '{tool}' cancelled")]
    Cancelled {
        /// Server name.
        server: String,
        /// Tool name.
        tool: String,
    },
    /// Tool result exceeded the per-server output cap.
    #[error("mcp: server '{server}' tool '{tool}' output {bytes}B exceeds limit {limit}B")]
    OutputTooLarge {
        /// Server name.
        server: String,
        /// Tool name.
        tool: String,
        /// Actual size in bytes.
        bytes: usize,
        /// Configured cap.
        limit: usize,
    },
    /// Selected `Transport` variant is not wired yet. Phase B retains this
    /// variant in the public surface for forward compatibility with Phase C
    /// (which lights up resources/elicitation transports), but Phase B itself
    /// wires `Transport::Http` and `Transport::Sse`.
    #[error("mcp: server '{server}' transport '{kind}' not yet implemented")]
    TransportNotYetImplemented {
        /// Server name.
        server: String,
        /// Transport kind.
        kind: &'static str,
    },
    /// HTTP/SSE transport error from rmcp's streamable-http client.
    #[error("mcp: server '{server}' http transport error: {message}")]
    Transport {
        /// Server name.
        server: String,
        /// Stringified rmcp error.
        message: String,
    },
    /// `${VAR}` substitution requires a variable that isn't set.
    #[error("mcp: server '{server}' field '{field}' references unset env var '{var}'")]
    MissingEnvField {
        /// Server name.
        server: String,
        /// Field whose value referenced the missing variable.
        field: String,
        /// Variable name.
        var: String,
    },
    /// `url` was missing, not an absolute http/https URL, or unparseable.
    #[error("mcp: server '{server}' invalid url '{url}': {reason}")]
    InvalidUrl {
        /// Server name.
        server: String,
        /// Raw URL value as provided.
        url: String,
        /// Human-readable reason (parse error / non-absolute / wrong scheme / etc.).
        reason: String,
    },
    /// HTTP/SSE transport requires `url`.
    #[error("mcp: server '{server}' transport='{transport}' requires a 'url' field; none provided")]
    MissingUrl {
        /// Server name.
        server: String,
        /// Transport kind that was selected (`"http"` or `"sse"`).
        transport: &'static str,
    },
    /// stdio transport doesn't accept `url`/`headers`/`oauth` fields.
    #[error("mcp: server '{server}' field '{field}' is not valid for transport='stdio'")]
    StdioFieldMismatch {
        /// Server name.
        server: String,
        /// Field that was misplaced.
        field: &'static str,
    },
    /// Legacy Phase B variant — superseded by `OauthDiscovery`/`OauthFlow`
    /// in Phase C. Retained for binary compatibility; never constructed.
    #[deprecated(
        since = "0.0.0",
        note = "Phase C wires real OAuth — this variant is no longer produced"
    )]
    #[error("mcp: server '{server}' oauth='{mode}' is not yet supported (legacy Phase B error)")]
    OauthPhaseC {
        /// Server name.
        server: String,
        /// Mode the operator requested.
        mode: String,
    },
    /// `oauth = "<garbage>"` — not one of `"off"|"auto"|"manual"`.
    #[error(
        "mcp: server '{server}' oauth='{value}' is invalid; expected 'off', 'auto', or 'manual'"
    )]
    InvalidOauthMode {
        /// Server name.
        server: String,
        /// Value the operator wrote.
        value: String,
    },
    /// `transport = "<garbage>"` — not one of the recognized variants.
    #[error(
        "mcp: server '{server}' transport='{value}' is invalid; expected 'stdio', 'http', or 'sse'"
    )]
    InvalidTransport {
        /// Server name.
        server: String,
        /// Value the operator wrote.
        value: String,
    },
    /// A static HTTP header name or value isn't legal HTTP.
    #[error("mcp: server '{server}' header '{name}' is invalid: {reason}")]
    InvalidHeader {
        /// Server name.
        server: String,
        /// Header name as written.
        name: String,
        /// Reason from `http::HeaderName`/`HeaderValue` parsing.
        reason: String,
    },
    // -------- Phase C: OAuth + elicitation + resources --------
    /// `[server.X.oauth]` discovery (well-known docs) failed.
    #[error("mcp: server '{server}' oauth discovery failed: {message}")]
    OauthDiscovery {
        /// Server name.
        server: String,
        /// Stringified underlying error.
        message: String,
    },
    /// Browser-redirect / loopback PKCE flow failed.
    #[error("mcp: server '{server}' oauth flow failed: {message}")]
    OauthFlow {
        /// Server name.
        server: String,
        /// Reason (cancelled / timed out / state mismatch / etc.).
        message: String,
    },
    /// Token-endpoint exchange or refresh failed.
    #[error("mcp: server '{server}' oauth token exchange failed: {message}")]
    OauthExchange {
        /// Server name.
        server: String,
        /// Stringified upstream error.
        message: String,
    },
    /// `oauth = "manual"` was set but a required field is missing in the
    /// `[server.X.oauth]` block.
    #[error("mcp: server '{server}' manual oauth block is missing required field '{field}'")]
    OauthManualIncomplete {
        /// Server name.
        server: String,
        /// Field that was missing (`client_id`, `auth_url`, `token_url`).
        field: &'static str,
    },
    /// A cold token cache needs an interactive browser authorization, but
    /// caliban is running headless (`--print` / stream-json / non-TTY). We
    /// refuse to hang on a loopback callback that can never complete.
    #[error(
        "mcp: server '{server}' needs interactive OAuth authorization; run caliban \
         interactively once (a browser will open) to cache a token, then re-run headless"
    )]
    OauthInteractiveRequired {
        /// Server name.
        server: String,
    },
    /// `oauth = "auto"` produced no `client_id`: there's no cached token, the
    /// config didn't supply one, and the authorization server does not offer
    /// dynamic client registration (RFC 7591). The operator must register an
    /// OAuth client and set `client_id` explicitly.
    #[error(
        "mcp: server '{server}' has no client_id and its authorization server does not \
         support dynamic client registration; register an OAuth app and set \
         [mcp_servers.{server}.oauth_config].client_id"
    )]
    OauthNoClientId {
        /// Server name.
        server: String,
    },
    /// OS keyring access failed (no backend or permission denied).
    #[error("mcp: server '{server}' keyring error: {source}")]
    Keyring {
        /// Server name.
        server: String,
        /// Stringified `keyring::Error`.
        #[source]
        source: keyring::Error,
    },
    /// Generic token-store IO/serialization issue.
    #[error("mcp: token store error: {0}")]
    TokenStore(String),
    /// Resource template arity mismatch (positional args don't match the
    /// number of `{placeholder}` slots).
    #[error("mcp: resource template '{template}' expects {expected} args but got {actual}")]
    ResourceTemplateArity {
        /// Template that was being expanded.
        template: String,
        /// Number of unique placeholders.
        expected: usize,
        /// Number of args provided.
        actual: usize,
    },
}

/// Result alias scoped to this crate.
pub type Result<T> = std::result::Result<T, McpError>;
