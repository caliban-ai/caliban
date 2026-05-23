//! Native wire-format types for Google Gemini streaming SSE chunks.
//!
//! In Gemini's SSE stream each `data:` line is a complete `NativeResponse`
//! (same shape as the non-streaming response). Chunks accumulate parts; the
//! final chunk has `finishReason` set.

// Re-export the full response type — each SSE chunk is a NativeResponse.
pub use crate::schema::response::NativeResponse;
