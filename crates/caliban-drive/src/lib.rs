//! # caliban-drive
//!
//! The transport-agnostic **drive core** for caliban (ADR 0055).
//!
//! caliban is a first-class MCP *client* but exposes no server surface. The
//! driveable-server-surface epic ([#503]) closes that gap with three protocol
//! adapters — MCP-server, ACP, and headless HTTP serve — built over one shared
//! core so the agent lifecycle is implemented once. This crate is that core.
//!
//! It exposes the four operations every driveable surface needs, with no
//! protocol or transport in scope:
//!
//! | Operation | Entry point |
//! |-----------|-------------|
//! | **run**    | [`DriveSession::spawn`] |
//! | **stream** | [`DriveSession::subscribe`] |
//! | **status** | [`DriveSession::status`] / [`DriveSession::wait_done`] |
//! | **input**  | [`DriveSession::send_input`] |
//!
//! A [`DriveSession`] drives a [`caliban_agent_core::Agent`] run on a background
//! task, fanning its `TurnEvent`s out to any number of subscribers (with replay,
//! so a late subscriber still sees the whole run) and, for interactive sessions,
//! feeding follow-up input into the loop's end-of-turn boundary (ADR 0047).
//!
//! The intended consumers are the caliban binary's headless `-p` driver and
//! caliband worker (which today hand-wire their own run/stream/input loops) and
//! the forthcoming MCP-server / ACP / HTTP-serve adapters. This crate depends
//! only on `caliban-agent-core` and `caliban-provider` — never on a transport
//! crate — so those adapters layer cleanly on top.
//!
//! [#503]: https://github.com/caliban-ai/caliban/issues/503

mod error;
mod inbound;
mod session;
mod status;

pub use error::DriveError;
pub use inbound::DriveInbound;
pub use session::{DriveOptions, DriveSession, new_run_id};
pub use status::DriveStatus;
