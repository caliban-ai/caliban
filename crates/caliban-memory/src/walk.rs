//! Ancestor-walk-up file discovery.
//!
//! Moved to [`caliban_common::paths::walk_up_for_file`] as part of the
//! cleanup sprint (PR-T1-A). This module is preserved as a thin re-export
//! so existing call sites keep compiling.

#[deprecated(
    since = "0.0.0",
    note = "use `caliban_common::paths::walk_up_for_file` instead; this re-export will be removed in a future cleanup-sprint PR"
)]
pub use caliban_common::paths::walk_up_for_file;
