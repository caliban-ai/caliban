//! Context-window tracker.
//!
//! Independent of telemetry export: `/usage`, `/context`, and the status-bar
//! percent indicator all work even when `CALIBAN_ENABLE_TELEMETRY=0`.
//!
//! Storage is split into a high-cardinality `Mutex<Inner>` (per-message bins
//! used by `/context`) and a lock-free `AtomicU16` of basis points (1 bp =
//! 0.01%) for the hot read path. The status bar re-reads the bp every frame,
//! so contention must stay near zero — even at 60 FPS.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU16, AtomicU32, Ordering};

use caliban_provider::{ContentBlock, Message, Role};

/// What kind of bucket a message rolls into for `/context` breakdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MessageKind {
    /// `Role::System` message at the start of the history.
    System,
    /// Auto-loaded memory prefix (top of the System slot beyond the
    /// hand-written prompt). Heuristically: any `System` message past the
    /// first.
    MemoryPrefix,
    /// User-typed text.
    UserText,
    /// Assistant text (model output).
    AssistantText,
    /// A model-issued `tool_use` block.
    ToolCall,
    /// A user-side `tool_result` block.
    ToolResult,
    /// A summarized turn (re-injected by the compactor).
    Summarized,
}

impl MessageKind {
    /// Stable label used by overlays.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System prompt",
            Self::MemoryPrefix => "Memory prefix",
            Self::UserText => "User text",
            Self::AssistantText => "Assistant text",
            Self::ToolCall => "Tool calls",
            Self::ToolResult => "Tool results",
            Self::Summarized => "Summarized",
        }
    }
}

/// Per-kind row in a `/context` breakdown.
#[derive(Debug, Clone)]
pub struct ContextBin {
    /// Which message kind this bin holds.
    pub kind: MessageKind,
    /// Tokens (estimate) attributed to this kind.
    pub tokens: u32,
}

/// Snapshot returned by `/context` rendering.
#[derive(Debug, Clone)]
pub struct ContextBreakdown {
    /// Model max input tokens (capacity). `0` when not yet set.
    pub capacity: u32,
    /// Sum of all bin tokens.
    pub used: u32,
    /// Per-kind rows in display order.
    pub bins: Vec<ContextBin>,
}

#[derive(Debug, Default)]
struct ContextInner {
    bins: [u32; 7],
}

/// Live context-window tracker. Read by the status bar every frame; written
/// once per turn after we know the latest message-history snapshot.
///
/// The hot path (`utilization`) is lock-free.
#[derive(Debug)]
pub struct ContextWindow {
    capacity: AtomicU32,
    /// Basis points (10_000 = 100%). Lock-free read.
    bp: AtomicU16,
    inner: Mutex<ContextInner>,
}

impl Default for ContextWindow {
    fn default() -> Self {
        Self::new()
    }
}

impl ContextWindow {
    /// Empty window with no capacity set.
    #[must_use]
    pub fn new() -> Self {
        Self {
            capacity: AtomicU32::new(0),
            bp: AtomicU16::new(0),
            inner: Mutex::new(ContextInner::default()),
        }
    }

    /// Set the model's max-input-tokens capacity. Called once after
    /// `Provider::capabilities` resolves.
    pub fn set_capacity(&self, max_input_tokens: u32) {
        self.capacity.store(max_input_tokens, Ordering::Relaxed);
        self.recompute_bp();
    }

    /// Capacity in tokens; `0` until `set_capacity` is called.
    #[must_use]
    pub fn capacity(&self) -> u32 {
        self.capacity.load(Ordering::Relaxed)
    }

    /// Utilization fraction (0.0..=1.0). Returns `0.0` until `set_capacity`
    /// is called.
    #[must_use]
    pub fn utilization(&self) -> f32 {
        f32::from(self.bp.load(Ordering::Relaxed)) / 10_000.0
    }

    /// Basis points (0..=10_000). The status bar uses this directly.
    #[must_use]
    pub fn utilization_bp(&self) -> u16 {
        self.bp.load(Ordering::Relaxed)
    }

    /// Replace the in-memory bins from a fresh message history. Re-categorizes
    /// every message using the heuristic in [`classify_message`].
    pub fn record_history(&self, messages: &[Message]) {
        let mut bins: [u32; 7] = [0; 7];
        let mut seen_system = false;
        for m in messages {
            for cb in &m.content {
                let (kind, tokens) = classify_content(&m.role, cb, &mut seen_system);
                bins[kind_index(kind)] = bins[kind_index(kind)].saturating_add(tokens);
            }
        }
        let mut guard = self.inner.lock().expect("context mutex poisoned");
        guard.bins = bins;
        drop(guard);
        self.recompute_bp();
    }

    /// Manually bump a kind by `tokens`. Used by `/compact` to attribute the
    /// summary back into the `Summarized` bucket.
    pub fn add(&self, kind: MessageKind, tokens: u32) {
        let mut guard = self.inner.lock().expect("context mutex poisoned");
        let idx = kind_index(kind);
        guard.bins[idx] = guard.bins[idx].saturating_add(tokens);
        drop(guard);
        self.recompute_bp();
    }

    /// Returns the current snapshot.
    #[must_use]
    pub fn breakdown(&self) -> ContextBreakdown {
        let bins = self.inner.lock().expect("context mutex poisoned").bins;
        let bins_out = ALL_KINDS
            .iter()
            .map(|k| ContextBin {
                kind: *k,
                tokens: bins[kind_index(*k)],
            })
            .collect::<Vec<_>>();
        let used = bins.iter().copied().sum::<u32>();
        ContextBreakdown {
            capacity: self.capacity(),
            used,
            bins: bins_out,
        }
    }

    fn recompute_bp(&self) {
        let cap = self.capacity.load(Ordering::Relaxed);
        if cap == 0 {
            self.bp.store(0, Ordering::Relaxed);
            return;
        }
        let bins = self.inner.lock().expect("context mutex poisoned").bins;
        let used: u32 = bins.iter().copied().sum();
        let bp = u32::from(u16::MAX).min((used.saturating_mul(10_000)) / cap.max(1));
        let bp = u16::try_from(bp).unwrap_or(u16::MAX);
        self.bp.store(bp, Ordering::Relaxed);
    }
}

const ALL_KINDS: [MessageKind; 7] = [
    MessageKind::System,
    MessageKind::MemoryPrefix,
    MessageKind::UserText,
    MessageKind::AssistantText,
    MessageKind::ToolCall,
    MessageKind::ToolResult,
    MessageKind::Summarized,
];

const fn kind_index(k: MessageKind) -> usize {
    match k {
        MessageKind::System => 0,
        MessageKind::MemoryPrefix => 1,
        MessageKind::UserText => 2,
        MessageKind::AssistantText => 3,
        MessageKind::ToolCall => 4,
        MessageKind::ToolResult => 5,
        MessageKind::Summarized => 6,
    }
}

/// Char-count → token estimate (chars/4 heuristic, matching agent-core's
/// `compact::estimate_tokens`).
fn est_tokens(s: &str) -> u32 {
    u32::try_from(s.len() / 4).unwrap_or(u32::MAX)
}

/// Classify one content block. The `seen_system` flag flips on the first
/// `System` message; subsequent system rows are treated as the auto-loaded
/// memory prefix.
fn classify_content(role: &Role, cb: &ContentBlock, seen_system: &mut bool) -> (MessageKind, u32) {
    match (role, cb) {
        (&Role::System, ContentBlock::Text(t)) => {
            let kind = if *seen_system {
                MessageKind::MemoryPrefix
            } else {
                *seen_system = true;
                MessageKind::System
            };
            (kind, est_tokens(&t.text))
        }
        (&Role::User, ContentBlock::Text(t)) => {
            // Heuristic: text containing the canonical summary prefix lands in
            // the Summarized bucket so /compact totals can show through.
            let bucket = if t.text.starts_with("Summary of earlier conversation:") {
                MessageKind::Summarized
            } else {
                MessageKind::UserText
            };
            (bucket, est_tokens(&t.text))
        }
        (&Role::Assistant, ContentBlock::Text(t)) => {
            (MessageKind::AssistantText, est_tokens(&t.text))
        }
        (_, ContentBlock::ToolUse(tu)) => {
            let len = tu.input.to_string().len() + tu.name.len();
            (
                MessageKind::ToolCall,
                u32::try_from(len / 4).unwrap_or(u32::MAX),
            )
        }
        (_, ContentBlock::ToolResult(tr)) => {
            let mut n: u32 = 0;
            for inner in &tr.content {
                if let ContentBlock::Text(t) = inner {
                    n = n.saturating_add(est_tokens(&t.text));
                }
            }
            (MessageKind::ToolResult, n)
        }
        (_, ContentBlock::Thinking(t)) => (MessageKind::AssistantText, est_tokens(&t.thinking)),
        _ => (MessageKind::UserText, 0),
    }
}

// ---------------------------------------------------------------------------
// Status-bar formatting helpers
// ---------------------------------------------------------------------------

/// Format a token count for the status bar: 1024 → "1K", 200000 → "200K".
#[must_use]
pub fn format_capacity_short(tokens: u32) -> String {
    if tokens >= 1_000_000 {
        format!("{}M", tokens / 1_000_000)
    } else if tokens >= 1_000 {
        format!("{}K", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

/// Format the status-bar segment for the context-window utilization indicator.
/// Returns `None` when capacity is `0` (status bar omits the segment).
///
/// Examples: `"12% of 200K"`, `"3% of 1M"`.
#[must_use]
pub fn format_status_segment(window: &ContextWindow) -> Option<String> {
    let cap = window.capacity();
    if cap == 0 {
        return None;
    }
    let bp = window.utilization_bp();
    let percent = bp / 100;
    Some(format!("{percent}% of {}", format_capacity_short(cap)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use caliban_provider::{Message, TextBlock};

    fn sys_msg(text: &str) -> Message {
        Message {
            role: Role::System,
            content: vec![ContentBlock::Text(TextBlock {
                text: text.into(),
                cache_control: None,
            })],
        }
    }
    fn user_msg(text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text(TextBlock {
                text: text.into(),
                cache_control: None,
            })],
        }
    }
    fn assistant_msg(text: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text(TextBlock {
                text: text.into(),
                cache_control: None,
            })],
        }
    }

    #[test]
    fn utilization_zero_before_set_capacity() {
        let w = ContextWindow::new();
        w.record_history(&[user_msg(&"x".repeat(4_000))]);
        // capacity unset → 0%.
        assert!((w.utilization() - 0.0).abs() < f32::EPSILON);
        assert_eq!(format_status_segment(&w), None);
    }

    #[test]
    fn utilization_50_percent_at_half_capacity() {
        let w = ContextWindow::new();
        w.set_capacity(200_000);
        // 100K tokens × 4 chars = 400K chars → est ~100K tokens.
        w.record_history(&[user_msg(&"x".repeat(400_000))]);
        let u = w.utilization();
        // Char heuristic is ~ chars/4 so this should be very close to 50%.
        assert!(u > 0.49 && u < 0.51, "utilization was {u}");
    }

    #[test]
    fn breakdown_segregates_system_and_memory_prefix() {
        let w = ContextWindow::new();
        w.set_capacity(200_000);
        let history = vec![
            sys_msg(&"a".repeat(400)),
            sys_msg(&"b".repeat(800)),
            user_msg(&"u".repeat(40)),
            assistant_msg(&"a".repeat(60)),
        ];
        w.record_history(&history);
        let bd = w.breakdown();
        let by_kind: std::collections::BTreeMap<_, _> =
            bd.bins.iter().map(|b| (b.kind, b.tokens)).collect();
        assert_eq!(by_kind[&MessageKind::System], 100, "400 chars / 4");
        assert_eq!(by_kind[&MessageKind::MemoryPrefix], 200, "800 chars / 4");
        assert_eq!(by_kind[&MessageKind::UserText], 10, "40 chars / 4");
        assert_eq!(by_kind[&MessageKind::AssistantText], 15, "60 chars / 4");
    }

    #[test]
    fn status_segment_formats_as_n_percent_of_k() {
        let w = ContextWindow::new();
        w.set_capacity(200_000);
        w.record_history(&[user_msg(&"x".repeat(96_000))]);
        // 24_000 tokens of 200_000 → 12%.
        let seg = format_status_segment(&w).expect("capacity is set");
        assert_eq!(seg, "12% of 200K");
    }

    #[test]
    fn capacity_short_formats() {
        assert_eq!(format_capacity_short(1_000), "1K");
        assert_eq!(format_capacity_short(200_000), "200K");
        assert_eq!(format_capacity_short(1_000_000), "1M");
        assert_eq!(format_capacity_short(800), "800");
    }

    #[test]
    fn add_bumps_summarized_bucket() {
        let w = ContextWindow::new();
        w.set_capacity(100_000);
        w.add(MessageKind::Summarized, 5_000);
        let bd = w.breakdown();
        let summ = bd
            .bins
            .iter()
            .find(|b| b.kind == MessageKind::Summarized)
            .unwrap();
        assert_eq!(summ.tokens, 5_000);
        // 5% utilization.
        let bp = w.utilization_bp();
        assert!((480..=520).contains(&bp), "bp was {bp}");
    }
}
