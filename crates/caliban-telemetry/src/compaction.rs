//! Metric names for context-management compaction + cache markers.
//!
//! Names match the OpenTelemetry convention `caliban.<domain>.<metric>`.
//! Each constant is the string passed to the meter when emitting counters.

/// Counter: fired when autocompact crosses its threshold and the strategy
/// is invoked (regardless of whether it actually compacted).
pub const AUTO_TRIGGERED: &str = "caliban.compaction.auto_triggered";

/// Counter: fired when autocompact has been disabled for the remainder of
/// the run after `MAX_CONSECUTIVE_COMPACT_FAILURES` (=2) failures.
pub const AUTO_DISABLED: &str = "caliban.compaction.auto_disabled_after_failures";

/// Histogram: tokens freed by a `MicroCompactor` pass (zero is common).
pub const MICRO_FREED: &str = "caliban.compaction.micro_freed_tokens";

/// Counter: fired once per `ToolResult` block that overflowed the global
/// per-tool-result size cap and was persisted to the overflow directory.
pub const TOOL_OVERFLOW: &str = "caliban.compaction.tool_result_overflowed";

/// Counter: fired when `apply_prompt_cache` set the conversation-level
/// marker on the last user message (i.e. the message met the
/// `min_cache_block_tokens` threshold).
pub const CACHE_MARKED: &str = "caliban.cache.conversation_marked";
