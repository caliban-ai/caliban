//! Client used by the `caliban` CLI (and the parent `AgentTool`) to talk
//! to a running supervisor daemon.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::UnixStream;

use crate::proto::{CtlReply, CtlRequest, SupervisorError};

/// Errors talking to the supervisor.
#[derive(thiserror::Error, Debug)]
pub enum ClientError {
    /// IO error talking over the socket.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// JSON (de)serialization error.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// The daemon returned an error reply.
    #[error("daemon: {0}")]
    Daemon(#[from] SupervisorError),
    /// The daemon's reply was not the variant the call expected.
    #[error("unexpected reply: {0}")]
    Unexpected(String),
    /// The daemon socket was not found.
    #[error("daemon not running at {0}")]
    NotRunning(PathBuf),
}

/// Thin client around a Unix socket. One client = one connection.
pub struct SupervisorClient {
    socket_path: PathBuf,
}

impl SupervisorClient {
    /// Build a client targeting the given socket path.
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    /// Connect and send a single request, returning the matching reply.
    pub async fn request(&self, req: &CtlRequest) -> Result<CtlReply, ClientError> {
        if !self.socket_path.exists() {
            return Err(ClientError::NotRunning(self.socket_path.clone()));
        }
        let stream = UnixStream::connect(&self.socket_path).await?;
        let (read_half, mut write_half) = stream.into_split();
        let mut body = serde_json::to_vec(req)?;
        body.push(b'\n');
        write_half.write_all(&body).await?;
        write_half.flush().await?;
        let mut reader = BufReader::new(read_half);
        let mut line = String::new();
        let read = tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut line)).await;
        match read {
            Ok(Ok(0)) => Err(ClientError::Unexpected("daemon closed connection".into())),
            Ok(Ok(_)) => Ok(serde_json::from_str(line.trim_end())?),
            Ok(Err(e)) => Err(ClientError::Io(e)),
            Err(_) => Err(ClientError::Unexpected("daemon timeout".into())),
        }
    }

    /// Convenience: send `List` and unwrap the agents.
    pub async fn list(&self) -> Result<Vec<crate::proto::AgentRecord>, ClientError> {
        match self.request(&CtlRequest::List).await? {
            CtlReply::Listed { agents } => Ok(agents),
            CtlReply::Error { error } => Err(error.into()),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// Convenience: send `Status`.
    pub async fn status(&self) -> Result<crate::proto::DaemonStatus, ClientError> {
        match self.request(&CtlRequest::Status).await? {
            CtlReply::Status(s) => Ok(s),
            CtlReply::Error { error } => Err(error.into()),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// Convenience: spawn a new agent.
    pub async fn spawn(
        &self,
        spec: crate::proto::SpawnSpec,
    ) -> Result<(crate::proto::AgentId, PathBuf), ClientError> {
        match self.request(&CtlRequest::Spawn { spec }).await? {
            CtlReply::Spawned { id, socket_path } => Ok((id, socket_path)),
            CtlReply::Error { error } => Err(error.into()),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// Convenience: kill an agent.
    pub async fn kill(&self, id: impl Into<String>) -> Result<(), ClientError> {
        match self.request(&CtlRequest::Kill { id: id.into() }).await? {
            CtlReply::Killed => Ok(()),
            CtlReply::Error { error } => Err(error.into()),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// Convenience: respawn an agent.
    pub async fn respawn(
        &self,
        id: impl Into<String>,
    ) -> Result<crate::proto::AgentId, ClientError> {
        match self.request(&CtlRequest::Respawn { id: id.into() }).await? {
            CtlReply::Respawned { id } => Ok(id),
            CtlReply::Error { error } => Err(error.into()),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// Convenience: rm an agent.
    pub async fn rm(&self, id: impl Into<String>, force: bool) -> Result<(), ClientError> {
        match self
            .request(&CtlRequest::Rm {
                id: id.into(),
                force,
            })
            .await?
        {
            CtlReply::Removed => Ok(()),
            CtlReply::Error { error } => Err(error.into()),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// Convenience: attach to an existing agent. Returns the per-agent
    /// socket path the caller can stream from.
    pub async fn attach(&self, id: impl Into<String>) -> Result<PathBuf, ClientError> {
        match self.request(&CtlRequest::Attach { id: id.into() }).await? {
            CtlReply::AttachAck { socket_path } => Ok(socket_path),
            CtlReply::Error { error } => Err(error.into()),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// Convenience: ask the daemon to shut down.
    pub async fn shutdown(&self) -> Result<(), ClientError> {
        match self.request(&CtlRequest::Shutdown).await? {
            CtlReply::ShutdownAck => Ok(()),
            CtlReply::Error { error } => Err(error.into()),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// Convenience: report a Running<->Idle transition for an agent.
    pub async fn report_status(
        &self,
        id: impl Into<String>,
        status: crate::proto::AgentStatus,
    ) -> Result<(), ClientError> {
        match self
            .request(&CtlRequest::ReportStatus {
                id: id.into(),
                status,
            })
            .await?
        {
            CtlReply::StatusReported => Ok(()),
            CtlReply::Error { error } => Err(error.into()),
            other => Err(ClientError::Unexpected(format!("{other:?}"))),
        }
    }

    /// Path the client targets.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}
