//! `Conn` — owns one MCP server's transport + running rmcp service.
//!
//! Phase A wires only `Transport::Stdio` (over `rmcp::transport::child_process`).
//! `Transport::Http` and `Transport::Sse` are intentionally type-stubbed so
//! Phase B can fill them in without churning callers.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use rmcp::ServiceExt;
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::TokioChildProcess;
use tokio_util::sync::CancellationToken;

use crate::config::ServerConfig;
use crate::error::McpError;

/// Selected transport for a given server. Phase A wires only `Stdio`.
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
    /// HTTP transport — Phase B. Currently a stub that returns
    /// [`McpError::TransportNotYetImplemented`] from [`Conn::start`].
    Http {
        /// Endpoint URL.
        url: String,
    },
    /// SSE transport — Phase B. Same stub semantics as [`Transport::Http`].
    Sse {
        /// Endpoint URL.
        url: String,
    },
}

impl Transport {
    /// Build a `Transport::Stdio` from a parsed `ServerConfig`. Phase B will
    /// dispatch on `ServerConfig::transport` to pick HTTP/SSE.
    pub fn from_config(cfg: &ServerConfig) -> Self {
        Self::Stdio {
            command: cfg.command.clone(),
            args: cfg.args.clone(),
            env: cfg.env.clone(),
            cwd: cfg.cwd.clone(),
        }
    }
}

/// One live MCP connection. Holds the rmcp `RunningService` (which owns the
/// child process + transport thread); `Drop`ping `Conn` shuts the service down.
pub struct Conn {
    /// Server name as it appears in `mcp.toml`.
    pub server: String,
    /// Underlying rmcp client service. Use [`Conn::peer`] for hot-path calls.
    pub service: RunningService<RoleClient, ()>,
    /// Child PID for diagnostics (stdio only).
    pub child_pid: Option<u32>,
    /// Cancellation token tied to the service. Cancel to abort the loop.
    pub cancel: CancellationToken,
}

impl std::fmt::Debug for Conn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Conn")
            .field("server", &self.server)
            .field("child_pid", &self.child_pid)
            .finish_non_exhaustive()
    }
}

impl Conn {
    /// Start a connection. Drives `initialize` to completion within
    /// `startup_timeout`.
    ///
    /// Phase A only supports `Transport::Stdio`. HTTP and SSE return
    /// [`McpError::TransportNotYetImplemented`].
    ///
    /// # Errors
    ///
    /// - [`McpError::Spawn`] if the child process fails to start (stdio).
    /// - [`McpError::HandshakeTimeout`] if `initialize` does not return within
    ///   `startup_timeout`.
    /// - [`McpError::Handshake`] if rmcp's initialize handshake itself errors.
    /// - [`McpError::TransportNotYetImplemented`] for non-stdio variants.
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
            Transport::Http { .. } | Transport::Sse { .. } => {
                Err(McpError::TransportNotYetImplemented {
                    server,
                    kind: match transport {
                        Transport::Http { .. } => "http",
                        Transport::Sse { .. } => "sse",
                        Transport::Stdio { .. } => unreachable!(),
                    },
                })
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

        // Drain stderr to tracing::warn on a background task so the pipe never
        // backs up. Each line is logged with the server name.
        if let Some(stderr) = stderr_opt {
            let server_for_log = server.clone();
            tokio::spawn(async move {
                use tokio::io::{AsyncBufReadExt, BufReader};
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::warn!(
                        target: "caliban::mcp::stderr",
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
}
