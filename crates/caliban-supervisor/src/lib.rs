//! `caliban-supervisor` — per-repo daemon owning background sub-agents.
//!
//! See `docs/superpowers/specs/2026-05-24-subagent-worktree-and-fleet-design.md`
//! and ADR 0037.
//!
//! The daemon (`caliband` binary) listens on a Unix domain socket at
//! `$XDG_RUNTIME_DIR/caliban/<hash(repo_root)>.sock` and serves a small
//! line-delimited JSON IPC protocol: `list`, `spawn`, `attach`, `kill`,
//! `respawn`, `rm`, `status`.
//!
//! For testability, the daemon's accept loop is driven by [`Supervisor::serve`]
//! which can bind any path the caller wants; the `caliband` binary picks
//! the per-repo runtime-dir path. The [`SupervisorClient`] talks to a
//! supervisor over the same socket and is what the `caliban` CLI uses.

#![allow(clippy::missing_errors_doc)]

pub mod client;
pub mod proc;
pub mod proto;
pub mod registry;
pub mod runtime;
pub mod server;
pub mod store;
pub mod transport;

pub use client::{ClientError, SupervisorClient};
pub use proc::{ExecWorkerLauncher, OsSignaller, Signaller, WorkerHandle, WorkerLauncher};
pub use proto::{
    AgentId, AgentRecord, AgentStatus, CtlReply, CtlRequest, DaemonStatus, SpawnSpec,
    SupervisorError,
};
pub use registry::Registry;
pub use runtime::{workspace_socket_path, workspace_socket_path_in};
pub use server::{NetworkConfig, Supervisor};
pub use store::AgentStore;
pub use transport::{BindSpec, BoxConn, ConnectSpec, Endpoint, Listener, connect};
