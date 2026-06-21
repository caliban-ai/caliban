//! State assembly for the `caliban` binary.
//!
//! Split into two submodules:
//!
//! - [`compose`] — composition-root helpers (provider/registry/permissions/
//!   settings construction).
//! - [`drivers`] — run-driver functions (single-prompt + headless agent loops).
//!
//! Both submodules are glob-re-exported so call sites refer to every helper as
//! `crate::startup::<name>` regardless of which file it lives in.

pub(crate) mod compose;
pub(crate) mod drivers;

pub(crate) use compose::*;
pub(crate) use drivers::*;
