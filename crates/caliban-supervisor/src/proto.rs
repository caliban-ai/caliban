//! IPC wire types for the supervisor protocol.
//!
//! These types now live in the thin, dependency-light [`caliban_contract::wire`]
//! crate so out-of-tree drivers (prospero) can depend on the contract without
//! pulling in the whole daemon (#656). This module re-exports them unchanged, so
//! `caliban_supervisor::proto::*` (and the `caliban_supervisor::` root re-exports)
//! keep resolving — the wire types have a single definition, in the contract.
//!
//! The wire format is newline-delimited JSON: each request is one JSON object
//! terminated by `\n`; each reply is one JSON object terminated by `\n`.

pub use caliban_contract::wire::{
    AgentId, AgentRecord, AgentStatus, CtlReply, CtlRequest, DaemonStatus, DrainedAgent,
    DriveProtocol, PermissionPosture, SpawnSpec, SupervisorError,
};
