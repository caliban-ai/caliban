//! Anthropic SSE event types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::request::NativeContentBlock;
use super::response::{NativeStopReason, NativeUsage};

/// Top-level SSE event envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NativeEvent {
    /// Sent once at the start of a message stream.
    MessageStart {
        /// The partial message header (id, model, initial usage).
        message: NativeMessageHeader,
    },
    /// Sent at the start of each content block.
    ContentBlockStart {
        /// Zero-based index of the content block.
        index: u32,
        /// Initial block shape (type + any static fields like tool id/name).
        content_block: NativeContentBlock,
    },
    /// Incremental delta for a content block.
    ContentBlockDelta {
        /// Zero-based index of the block being updated.
        index: u32,
        /// The incremental payload.
        delta: NativeBlockDelta,
    },
    /// Signals the end of a content block.
    ContentBlockStop {
        /// Zero-based index of the block that has finished.
        index: u32,
    },
    /// Final message metadata (stop reason, cumulative usage).
    MessageDelta {
        /// Stop reason and sequence.
        delta: NativeMessageDelta,
        /// Cumulative usage at stream end.
        usage: NativeUsage,
    },
    /// Signals end of the entire stream.
    MessageStop,
    /// Heartbeat — no payload required.
    Ping,
    /// Server-side error event.
    Error {
        /// Raw JSON error object from the API.
        error: Value,
    },
}

/// Partial message header carried by `message_start`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeMessageHeader {
    /// Unique message ID.
    pub id: String,
    /// Model that produced the stream.
    pub model: String,
    /// Initial usage snapshot (`output_tokens` == 0 at start).
    pub usage: NativeUsage,
    /// Content blocks pre-populated by the API (usually empty at stream start).
    #[serde(default)]
    pub content: Vec<NativeContentBlock>,
}

/// Incremental delta types for content blocks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NativeBlockDelta {
    /// Plain-text character delta.
    TextDelta {
        /// The incremental text fragment.
        text: String,
    },
    /// Partial JSON string for tool-use input accumulation.
    InputJsonDelta {
        /// Partial JSON fragment (must be concatenated before parsing).
        partial_json: String,
    },
    /// Incremental thinking text.
    ThinkingDelta {
        /// The incremental thinking fragment.
        thinking: String,
    },
    /// Cryptographic signature appended after thinking text.
    SignatureDelta {
        /// The signature fragment.
        signature: String,
    },
}

/// Delta payload in `message_delta` events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeMessageDelta {
    /// Why the model stopped (populated in `message_delta`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<NativeStopReason>,
    /// The stop sequence that triggered the stop (if any).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_sequence: Option<String>,
}
