//! Driveable server surfaces (ADR 0055).
//!
//! Home of the protocol adapters that let an external client drive caliban over
//! the shared [`caliban_drive`] core — MCP-server (#526), ACP (#530), and
//! headless HTTP serve (#531) — plus the cross-cutting pieces they share: the
//! [`auth`] gate and (forthcoming) the permission-elicitation bridge.
//!
//! Like the sibling `headless` and `worker` surfaces, these live in the binary
//! and consume the `caliban-drive` crate; the transport-agnostic core stays a
//! crate with no transport dependency (ADR 0055).

// Forward-facing scaffolding: the shared auth gate lands ahead of its first
// consumer (the MCP-server adapter, #526), mirroring how `headless` surfaced its
// protocol types before every code path exercised them. Allow dead_code
// module-wide so each item need not be individually gated.
#![allow(dead_code)]

pub(crate) mod auth;
pub(crate) mod permissions;
