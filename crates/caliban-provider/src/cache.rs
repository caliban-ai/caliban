//! Prompt-cache markers in the IR.

use serde::{Deserialize, Serialize};

/// Cache-control marker that can be attached to content blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CacheControl {
    /// Mark this block as an ephemeral cache breakpoint.
    Ephemeral,
}
