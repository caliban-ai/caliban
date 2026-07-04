//! `Conn` — owns one MCP server's transport + running rmcp service.
//!
//! Phase B wires `Transport::Stdio`, `Transport::Http`, and `Transport::Sse`.
//! HTTP and SSE both route through rmcp 1.7's `StreamableHttpClientTransport`
//! (rmcp 1.7 folded the standalone SSE client into the streamable-http worker;
//! see the spec note in `docs/superpowers/specs/2026-05-24-mcp-v2-design.md`).
//! OAuth (auto/manual) is intentionally not wired here — it's a config-parse
//! error to ask for it, and Phase C will lift it in.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use http::{HeaderMap, HeaderName, HeaderValue};
use rmcp::ServiceExt;
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::TokioChildProcess;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::config::{ServerConfig, TransportKind};
use crate::error::McpError;

/// Selected transport for a given server. Phase B wires `Stdio`, `Http`, and
/// `Sse`; `Conn::start` dispatches on this enum.
#[derive(Debug, Clone)]
pub enum Transport {
    /// Spawn a local subprocess and speak JSON-RPC over its stdin/stdout.
    Stdio {
        /// Executable to launch.
        command: String,
        /// CLI arguments.
        args: Vec<String>,
        /// Environment variables (already `${VAR}`-expanded).
        env: BTreeMap<String, String>,
        /// Working directory; `None` inherits the caliban process cwd.
        cwd: Option<PathBuf>,
    },
    /// rmcp streamable-http client (POST + chunked + SSE).
    Http {
        /// Endpoint URL.
        url: Url,
        /// Static request headers (already env-expanded).
        headers: HeaderMap,
    },
    /// Legacy "SSE" servers. Routed through the same streamable-http transport
    /// because rmcp 1.7 merged the standalone SSE client into it; the
    /// `transport = "sse"` config value still surfaces as `sse` in the `/mcp`
    /// overlay for operator clarity.
    Sse {
        /// Endpoint URL.
        url: Url,
        /// Static request headers (already env-expanded).
        headers: HeaderMap,
    },
}

impl Transport {
    /// Build a transport from a parsed `ServerConfig`. Returns an `McpError`
    /// when a static header on an http/sse config can't be parsed as legal HTTP.
    ///
    /// # Errors
    /// [`McpError::InvalidHeader`] if a configured header name or value is
    /// not a legal HTTP header.
    pub fn from_config(server: &str, cfg: &ServerConfig) -> Result<Self, McpError> {
        match cfg.transport {
            TransportKind::Stdio => Ok(Self::Stdio {
                command: cfg.command.clone(),
                args: cfg.args.clone(),
                env: cfg.env.clone(),
                cwd: cfg.cwd.clone(),
            }),
            TransportKind::Http | TransportKind::Sse => {
                let url = cfg.url.clone().ok_or_else(|| McpError::MissingUrl {
                    server: server.to_string(),
                    transport: cfg.transport.as_str(),
                })?;
                let mut headers = HeaderMap::new();
                for (k, v) in &cfg.headers {
                    let name =
                        HeaderName::try_from(k.as_str()).map_err(|e| McpError::InvalidHeader {
                            server: server.to_string(),
                            name: k.clone(),
                            reason: e.to_string(),
                        })?;
                    let value =
                        HeaderValue::try_from(v.as_str()).map_err(|e| McpError::InvalidHeader {
                            server: server.to_string(),
                            name: k.clone(),
                            reason: e.to_string(),
                        })?;
                    headers.insert(name, value);
                }
                Ok(match cfg.transport {
                    TransportKind::Http => Self::Http { url, headers },
                    TransportKind::Sse => Self::Sse { url, headers },
                    TransportKind::Stdio => unreachable!(),
                })
            }
        }
    }

    /// Attach an OAuth `Authorization: Bearer <token>` header to an http/sse
    /// transport (a no-op for stdio, which has no request headers). Overwrites
    /// any existing `Authorization` header. Called by the manager after the
    /// OAuth authenticator resolves a token, just before the handshake.
    ///
    /// # Errors
    /// [`McpError::InvalidHeader`] if the token yields an illegal header value
    /// (e.g. it contains control characters) — should not happen for a
    /// well-formed access token.
    pub fn set_bearer(&mut self, server: &str, token: &str) -> Result<(), McpError> {
        let headers = match self {
            Self::Http { headers, .. } | Self::Sse { headers, .. } => headers,
            Self::Stdio { .. } => return Ok(()),
        };
        let value = HeaderValue::try_from(format!("Bearer {token}")).map_err(|e| {
            McpError::InvalidHeader {
                server: server.to_string(),
                name: "Authorization".to_string(),
                reason: e.to_string(),
            }
        })?;
        headers.insert(http::header::AUTHORIZATION, value);
        Ok(())
    }

    /// Short label for the `/mcp` overlay transport column.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Stdio { .. } => "stdio",
            Self::Http { .. } => "http",
            Self::Sse { .. } => "sse",
        }
    }
}

/// One live MCP connection. Holds the rmcp `RunningService` (which owns the
/// child process or HTTP worker + transport thread); `Drop`ping `Conn` shuts
/// the service down.
pub struct Conn {
    /// Server name as it appears in `mcp.toml`.
    pub server: String,
    /// Underlying rmcp client service. Use [`Conn::peer`] for hot-path calls.
    pub service: RunningService<RoleClient, ()>,
    /// Child PID for diagnostics (stdio only).
    pub child_pid: Option<u32>,
    /// Cancellation token tied to the service. Cancel to abort the loop.
    pub cancel: CancellationToken,
    /// Transport kind, captured for `/mcp` overlay + diagnostics.
    pub transport_kind: &'static str,
}

impl std::fmt::Debug for Conn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Conn")
            .field("server", &self.server)
            .field("child_pid", &self.child_pid)
            .field("transport_kind", &self.transport_kind)
            .finish_non_exhaustive()
    }
}

impl Conn {
    /// Start a connection. Drives `initialize` to completion within
    /// `startup_timeout`.
    ///
    /// Dispatches on the `Transport` variant.
    ///
    /// # Errors
    /// - [`McpError::Spawn`] if the child process fails to start (stdio).
    /// - [`McpError::HandshakeTimeout`] if `initialize` does not return within
    ///   `startup_timeout`.
    /// - [`McpError::Handshake`] if rmcp's initialize handshake itself errors.
    /// - [`McpError::Transport`] if the http/sse worker can't be brought up.
    pub async fn start(
        server: String,
        transport: Transport,
        startup_timeout: Duration,
    ) -> Result<Self, McpError> {
        match transport {
            Transport::Stdio {
                command,
                args,
                env,
                cwd,
            } => Self::start_stdio(server, command, args, env, cwd, startup_timeout).await,
            Transport::Http { url, headers } => {
                Self::start_http_like(server, url, headers, "http", startup_timeout).await
            }
            Transport::Sse { url, headers } => {
                Self::start_http_like(server, url, headers, "sse", startup_timeout).await
            }
        }
    }

    async fn start_stdio(
        server: String,
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        cwd: Option<PathBuf>,
        startup_timeout: Duration,
    ) -> Result<Self, McpError> {
        // Build the tokio::process::Command. We keep stderr piped to a no-op
        // drain so a chatty server doesn't fill the OS pipe buffer and stall.
        let mut cmd = tokio::process::Command::new(&command);
        cmd.args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (k, v) in &env {
            cmd.env(k, v);
        }
        if let Some(dir) = &cwd {
            cmd.current_dir(dir);
        }

        let (proc, stderr_opt) = TokioChildProcess::builder(cmd)
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|source| McpError::Spawn {
                server: server.clone(),
                source,
            })?;
        let child_pid = proc.id();

        if let Some(stderr) = stderr_opt {
            let server_for_log = server.clone();
            tokio::spawn(async move {
                use tokio::io::{AsyncBufReadExt, BufReader};
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::warn!(
                        target: caliban_common::tracing_targets::TARGET_MCP_STDERR,
                        server = %server_for_log,
                        "{line}",
                    );
                }
            });
        }

        let cancel = CancellationToken::new();
        let serve_future = ().serve_with_ct(proc, cancel.clone());
        let service = tokio::time::timeout(startup_timeout, serve_future)
            .await
            .map_err(|_elapsed| McpError::HandshakeTimeout {
                server: server.clone(),
                timeout: startup_timeout,
            })?
            .map_err(|source| McpError::Handshake {
                server: server.clone(),
                message: source.to_string(),
            })?;

        Ok(Self {
            server,
            service,
            child_pid,
            cancel,
            transport_kind: "stdio",
        })
    }

    /// Common code path for `Transport::Http` and `Transport::Sse` — both use
    /// rmcp's streamable-http worker. The `kind` argument is recorded for the
    /// `/mcp` overlay so the column reflects what the operator wrote in TOML.
    async fn start_http_like(
        server: String,
        url: Url,
        headers: HeaderMap,
        kind: &'static str,
        startup_timeout: Duration,
    ) -> Result<Self, McpError> {
        // Build the streamable-http transport with a default reqwest client +
        // operator-supplied custom headers. `with_uri` + `custom_headers` keeps
        // us off the `#[non_exhaustive]` struct literal path.
        let mut custom_headers: std::collections::HashMap<HeaderName, HeaderValue> =
            std::collections::HashMap::with_capacity(headers.len());
        for (name, value) in &headers {
            custom_headers.insert(name.clone(), value.clone());
        }
        let cfg = StreamableHttpClientTransportConfig::with_uri(Arc::<str>::from(url.as_str()))
            .custom_headers(custom_headers);
        let transport = StreamableHttpClientTransport::from_config(cfg);

        let cancel = CancellationToken::new();
        let serve_future = ().serve_with_ct(transport, cancel.clone());
        let service = tokio::time::timeout(startup_timeout, serve_future)
            .await
            .map_err(|_elapsed| McpError::HandshakeTimeout {
                server: server.clone(),
                timeout: startup_timeout,
            })?
            .map_err(|source| McpError::Handshake {
                server: server.clone(),
                message: source.to_string(),
            })?;

        Ok(Self {
            server,
            service,
            child_pid: None,
            cancel,
            transport_kind: kind,
        })
    }

    /// Borrow the client peer for sending requests (`list_tools` / `call_tool`).
    #[must_use]
    pub fn peer(&self) -> &rmcp::Peer<RoleClient> {
        self.service.peer()
    }

    /// Process ID of the spawned child (stdio only).
    #[must_use]
    pub fn pid(&self) -> Option<u32> {
        self.child_pid
    }

    /// Transport kind label, suitable for the `/mcp` overlay column.
    #[must_use]
    pub fn transport_kind(&self) -> &'static str {
        self.transport_kind
    }
}
