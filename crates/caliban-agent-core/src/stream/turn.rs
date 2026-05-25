//! Per-turn accumulator state and TTFT/TBT timing.
//!
//! These types are the building blocks consumed by the single-turn loop body
//! in `stream/mod.rs`. They're kept here so the loop driver can stay focused
//! on orchestration without intermixing low-level stream-event bookkeeping.

use std::time::{Duration, Instant};

use caliban_provider::{
    ContentBlock, Message, Role, TextBlock, ThinkingBlock, ToolUseBlock, Usage,
};

// ---------------------------------------------------------------------------
// Per-turn timing (TTFT/TBT)
// ---------------------------------------------------------------------------

/// Captures per-turn wall-clock latency markers:
/// - **TTFT** (time-to-first-token): request-sent → first delta arrived.
/// - **TBT** (time-between-tokens): mean inter-delta interval.
#[derive(Debug)]
pub(crate) struct TurnTiming {
    request_sent_at: Instant,
    first_delta_at: Option<Instant>,
    last_delta_at: Option<Instant>,
    pub(crate) delta_count: u32,
}

impl TurnTiming {
    pub(crate) fn start() -> Self {
        Self {
            request_sent_at: Instant::now(),
            first_delta_at: None,
            last_delta_at: None,
            delta_count: 0,
        }
    }

    pub(crate) fn observe_delta(&mut self) {
        let now = Instant::now();
        self.first_delta_at.get_or_insert(now);
        self.last_delta_at = Some(now);
        self.delta_count += 1;
    }

    pub(crate) fn ttft(&self) -> Option<Duration> {
        self.first_delta_at
            .map(|t| t.saturating_duration_since(self.request_sent_at))
    }

    pub(crate) fn tbt(&self) -> Option<Duration> {
        match (self.first_delta_at, self.last_delta_at, self.delta_count) {
            (Some(f), Some(l), n) if n >= 2 => Some(l.saturating_duration_since(f) / (n - 1)),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal accumulator state for one provider stream
// ---------------------------------------------------------------------------

/// In-progress content block being assembled from stream events.
pub(crate) enum ActiveBlock {
    Text {
        accumulated: String,
    },
    Thinking {
        accumulated: String,
    },
    ToolUse {
        id: String,
        name: String,
        json_buf: String,
    },
}

/// State accumulated while draining one provider `MessageStream`.
pub(crate) struct MessageAccumulator {
    pub(crate) message_id: String,
    pub(crate) model: String,
    pub(crate) blocks: Vec<ContentBlock>,
    pub(crate) active: Vec<Option<ActiveBlock>>,
    pub(crate) stop_reason: Option<caliban_provider::StopReason>,
    pub(crate) usage: Usage,
}

impl MessageAccumulator {
    pub(crate) fn new() -> Self {
        Self {
            message_id: String::new(),
            model: String::new(),
            blocks: Vec::new(),
            active: Vec::new(),
            stop_reason: None,
            usage: Usage::default(),
        }
    }

    /// Ensure the `active` and `blocks` vecs are large enough for `index`.
    pub(crate) fn ensure_index(&mut self, index: usize) {
        if self.active.len() <= index {
            self.active.resize_with(index + 1, || None);
            self.blocks.resize(
                index + 1,
                ContentBlock::Text(TextBlock {
                    text: String::new(),
                    cache_control: None,
                }),
            );
        }
    }

    /// Finalize a block at `index` after `ContentBlockStop`.
    pub(crate) fn finalize_block(&mut self, index: usize) {
        let Some(slot) = self.active.get_mut(index) else {
            return;
        };
        let Some(active) = slot.take() else {
            return;
        };
        let block = match active {
            ActiveBlock::Text { accumulated } => ContentBlock::Text(TextBlock {
                text: accumulated,
                cache_control: None,
            }),
            ActiveBlock::Thinking { accumulated } => ContentBlock::Thinking(ThinkingBlock {
                thinking: accumulated,
                signature: None,
            }),
            ActiveBlock::ToolUse { id, name, json_buf } => {
                let input = if json_buf.is_empty() {
                    serde_json::json!({})
                } else {
                    serde_json::from_str(&json_buf).unwrap_or(serde_json::json!({}))
                };
                ContentBlock::ToolUse(ToolUseBlock { id, name, input })
            }
        };
        if index < self.blocks.len() {
            self.blocks[index] = block;
        }
    }

    pub(crate) fn into_message(self) -> Message {
        Message {
            role: Role::Assistant,
            content: self.blocks,
        }
    }
}

#[cfg(test)]
mod turn_timing_tests {
    use super::TurnTiming;
    use std::thread::sleep;
    use std::time::Duration;

    #[test]
    fn no_delta_means_no_ttft_and_no_tbt() {
        let t = TurnTiming::start();
        assert!(t.ttft().is_none());
        assert!(t.tbt().is_none());
    }

    #[test]
    fn single_delta_gives_ttft_but_no_tbt() {
        let mut t = TurnTiming::start();
        sleep(Duration::from_millis(5));
        t.observe_delta();
        assert!(t.ttft().unwrap() >= Duration::from_millis(4));
        assert!(t.tbt().is_none(), "TBT needs >= 2 deltas");
    }

    #[test]
    fn multi_delta_gives_ttft_and_tbt() {
        let mut t = TurnTiming::start();
        sleep(Duration::from_millis(5));
        t.observe_delta();
        sleep(Duration::from_millis(10));
        t.observe_delta();
        sleep(Duration::from_millis(10));
        t.observe_delta();
        assert!(t.ttft().unwrap() >= Duration::from_millis(4));
        // Two intervals of ~10ms each → mean ~10ms. Wide tolerance for CI.
        let tbt = t.tbt().unwrap();
        assert!(
            tbt >= Duration::from_millis(5) && tbt <= Duration::from_millis(50),
            "tbt was {tbt:?}"
        );
    }
}
