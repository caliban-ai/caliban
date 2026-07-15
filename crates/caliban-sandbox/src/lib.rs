//! OS-level sandbox for caliban subprocess tools.
//!
//! Implements ADR 0032 / `docs/superpowers/specs/2026-05-24-os-sandbox-design.md`.
//!
//! Two backends, one config surface:
//!
//! - **macOS** uses Apple's Seatbelt via `sandbox-exec` with a generated
//!   `.sb` profile.
//! - **Linux / WSL** uses bubblewrap (`bwrap`) with `--bind` / `--ro-bind`
//!   / `--tmpfs` / `--unshare-*` flags.
//! - **Windows native** is a no-op + warning (Job Objects + `AppContainer`
//!   deferred to v2).
//!
//! Usage:
//!
//! ```no_run
//! use caliban_sandbox::{Policy, SandboxedShim};
//!
//! let policy = Policy::default(); // disabled by default
//! let shim = SandboxedShim::new(policy).unwrap();
//! let mut cmd = tokio::process::Command::new("/bin/sh");
//! cmd.arg("-c").arg("echo hi");
//! shim.wrap_command(&mut cmd, "echo hi").unwrap();
//! // Now `cmd` is either unchanged (disabled / bypass) or replaced with
//! // a sandbox-exec / bwrap invocation that wraps the original command.
//! ```

pub mod bwrap;
pub mod config;
pub mod detect;
pub mod error;
pub mod seatbelt;
pub mod shim;

pub use config::{EnvAcl, FilesystemAcl, NetworkAcl, Policy};
pub use detect::{Backend, detect};
pub use error::SandboxError;
pub use shim::SandboxedShim;
