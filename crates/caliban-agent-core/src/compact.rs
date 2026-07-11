//! Compactor trait — strategies for truncating long histories.

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use caliban_provider::{
    Capabilities, ContentBlock, Message, Provider, Role, TextBlock, ToolResultBlock, ToolUseBlock,
    Usage,
};

use crate::error::{Error, Result};

/// Outcome of a compaction pass: the rewritten history plus any provider usage
/// the compactor itself incurred. Only `SummarizingCompactor` makes a provider
/// call, so `usage` is `None` for the LLM-free strategies; threading it out
/// lets the caller fold autocompact spend into session totals (#292/#329).
#[derive(Debug, Clone, Default)]
pub struct Compaction {
    /// The rewritten message history to install.
    pub messages: Vec<Message>,
    /// Provider usage consumed producing this compaction (e.g. the
    /// summarization call), if any.
    pub usage: Option<Usage>,
}

impl Compaction {
    /// A compaction with no associated provider usage (LLM-free strategies).
    fn free(messages: Vec<Message>) -> Self {
        Self {
            messages,
            usage: None,
        }
    }
}

/// Compactor — strategy for keeping the message history under the model's
/// input window.
#[async_trait]
pub trait Compactor: Send + Sync {
    /// Decide whether to compact. Returns the new messages (plus any usage the
    /// compactor itself incurred) if compaction was applied; `None` if no-op.
    async fn compact(
        &self,
        messages: &[Message],
        capabilities: &Capabilities,
    ) -> Result<Option<Compaction>>;

    /// Strategy identifier surfaced to `PreCompact` / `PostCompact` hooks.
    /// Defaults to the type's short Rust name; impls override as desired.
    fn strategy_name(&self) -> &'static str {
        "unknown"
    }
}

/// Safety net that drops tool-call blocks which would be orphaned in
/// `messages` — a `tool_result` whose `tool_use` is absent (→ Anthropic 400)
/// or a dangling `tool_use` with no result — plus any message emptied as a
/// result. The window boundary is chosen by [`clean_tail_start`] so orphans
/// shouldn't normally arise; this guards against a mid-turn cut or a
/// malformed history. It intentionally does **not** reorder or strip by role
/// (see the note in [`clean_tail_start`]).
#[must_use]
pub fn sanitize_tool_pairs(messages: &[Message]) -> Vec<Message> {
    use std::collections::HashSet;

    let mut use_ids: HashSet<&str> = HashSet::new();
    let mut result_ids: HashSet<&str> = HashSet::new();
    for m in messages {
        for cb in &m.content {
            match cb {
                ContentBlock::ToolUse(tu) => {
                    use_ids.insert(tu.id.as_str());
                }
                ContentBlock::ToolResult(tr) => {
                    result_ids.insert(tr.tool_use_id.as_str());
                }
                _ => {}
            }
        }
    }

    let mut out: Vec<Message> = Vec::with_capacity(messages.len());
    for m in messages {
        let content: Vec<ContentBlock> = m
            .content
            .iter()
            .filter(|cb| match cb {
                // A result whose use was dropped is an orphan → drop it.
                ContentBlock::ToolResult(tr) => use_ids.contains(tr.tool_use_id.as_str()),
                // A use whose result is absent (dangling call) → drop it.
                ContentBlock::ToolUse(tu) => result_ids.contains(tu.id.as_str()),
                _ => true,
            })
            .cloned()
            .collect();
        if !content.is_empty() {
            out.push(Message {
                role: m.role.clone(),
                content,
            });
        }
    }
    out
}

/// Choose a start index into `body` that keeps roughly the last `desired`
/// messages but begins on a *genuine user turn* — a `User` message carrying no
/// `ToolResult`. A blind offset can open the retained window on a `tool_result`
/// whose `tool_use` fell into the dropped slice (Anthropic rejects the orphan)
/// or on a bare assistant message (the first message must be a user turn); a
/// clean user turn is the only boundary from which every following tool pair is
/// self-contained.
///
/// Prefers the clean boundary at or after the ideal offset (tighter budget),
/// then the nearest one before it (keeps more context), then falls back to `0`
/// — keep everything — when no clean boundary exists (a single long agentic
/// turn); the in-window truncation fallback handles any resulting overflow.
fn clean_tail_start(body: &[Message], desired: usize) -> usize {
    let ideal = body.len().saturating_sub(desired);
    let is_clean = |m: &Message| {
        m.role == Role::User
            && !m
                .content
                .iter()
                .any(|cb| matches!(cb, ContentBlock::ToolResult(_)))
    };
    if let Some(i) = (ideal..body.len()).find(|&i| is_clean(&body[i])) {
        return i;
    }
    if let Some(i) = (0..ideal).rev().find(|&i| is_clean(&body[i])) {
        return i;
    }
    0
}

/// Janitor compactor: replaces older `ToolResult` blocks with a one-line
/// placeholder when a newer invocation of the same logical action exists.
/// LLM-free; O(n) per call.
#[derive(Debug, Default)]
pub struct MicroCompactor;

impl MicroCompactor {
    /// Construct a new [`MicroCompactor`].
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Compactor for MicroCompactor {
    async fn compact(
        &self,
        messages: &[Message],
        _capabilities: &Capabilities,
    ) -> Result<Option<Compaction>> {
        // Map tool_use_id → is_error from the ToolResult blocks, so a failed
        // result can't be chosen as the surviving "latest" (#170).
        let mut errored: std::collections::HashMap<&str, bool> = std::collections::HashMap::new();
        for m in messages {
            for cb in &m.content {
                if let caliban_provider::ContentBlock::ToolResult(tr) = cb {
                    errored.insert(tr.tool_use_id.as_str(), tr.is_error);
                }
            }
        }
        // First pass: find the latest *successful* tool_use_id for each
        // (tool, key). Only a result that exists and did not error may
        // supersede earlier ones — otherwise a later failed Read would
        // destroy the earlier good content.
        let mut latest: std::collections::HashMap<(String, String), String> =
            std::collections::HashMap::new();
        for m in messages {
            for cb in &m.content {
                if let caliban_provider::ContentBlock::ToolUse(tu) = cb
                    && let Some(k) = supersession_key(&tu.name, &tu.input)
                    && errored.get(tu.id.as_str()) == Some(&false)
                {
                    latest.insert((tu.name.clone(), k), tu.id.clone());
                }
            }
        }
        // Build a map tool_use_id → (tool_name, key) for older invocations.
        let mut superseded: std::collections::HashMap<String, (String, String)> =
            std::collections::HashMap::new();
        for m in messages {
            for cb in &m.content {
                if let caliban_provider::ContentBlock::ToolUse(tu) = cb
                    && let Some(k) = supersession_key(&tu.name, &tu.input)
                    && let Some(latest_id) = latest.get(&(tu.name.clone(), k.clone()))
                    && latest_id != &tu.id
                {
                    superseded.insert(tu.id.clone(), (tu.name.clone(), k));
                }
            }
        }
        if superseded.is_empty() {
            return Ok(None);
        }
        // Second pass: rewrite ToolResult blocks whose id is superseded.
        let new: Vec<Message> = messages
            .iter()
            .map(|m| {
                let new_content: Vec<_> = m
                    .content
                    .iter()
                    .map(|cb| match cb {
                        caliban_provider::ContentBlock::ToolResult(tr) => {
                            if let Some((tool, key)) = superseded.get(&tr.tool_use_id) {
                                let placeholder = format!("[superseded: {tool}({key})]");
                                caliban_provider::ContentBlock::ToolResult(
                                    caliban_provider::ToolResultBlock {
                                        tool_use_id: tr.tool_use_id.clone(),
                                        content: vec![caliban_provider::ContentBlock::Text(
                                            caliban_provider::TextBlock {
                                                text: placeholder,
                                                cache_control: None,
                                            },
                                        )],
                                        is_error: tr.is_error,
                                    },
                                )
                            } else {
                                cb.clone()
                            }
                        }
                        _ => cb.clone(),
                    })
                    .collect();
                caliban_provider::Message {
                    role: m.role.clone(),
                    content: new_content,
                }
            })
            .collect();
        Ok(Some(Compaction::free(new)))
    }

    fn strategy_name(&self) -> &'static str {
        "MicroCompactor"
    }
}

/// Per-tool predicate for "newer invocation of this same logical action".
/// Returns the supersession key; `None` means this tool is never supersedable.
pub(crate) fn supersession_key(tool_name: &str, input: &serde_json::Value) -> Option<String> {
    match tool_name {
        "Read" => input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(String::from),
        "Grep" | "Glob" => Some(input.to_string()),
        "WebFetch" => input.get("url").and_then(|v| v.as_str()).map(String::from),
        _ => None,
    }
}

/// Estimate token count using a chars/4 heuristic.
#[must_use]
pub fn estimate_tokens(messages: &[Message]) -> u32 {
    let mut chars: usize = 0;
    for m in messages {
        for cb in &m.content {
            if let caliban_provider::ContentBlock::Text(t) = cb {
                chars += t.text.len();
            }
            if let caliban_provider::ContentBlock::ToolResult(tr) = cb {
                for inner in &tr.content {
                    if let caliban_provider::ContentBlock::Text(t) = inner {
                        chars += t.text.len();
                    }
                }
            }
            if let caliban_provider::ContentBlock::Thinking(t) = cb {
                chars += t.thinking.len();
            }
            if let caliban_provider::ContentBlock::ToolUse(tu) = cb {
                chars += tu.input.to_string().len();
                chars += tu.name.len();
            }
        }
    }
    u32::try_from(chars / 4).unwrap_or(u32::MAX)
}

/// Last-resort in-window reducer (#329): when the retained tail alone still
/// exceeds `max_tokens` — typically a single oversized tool result inside the
/// last few turns — truncate the largest text/tool-result payloads in place
/// until it fits, instead of no-op'ing forever (`Summarizing`) or hard-erroring
/// (`DropOldest`, which would trip the consecutive-failure disable). Preserves
/// message and turn structure plus tool pairing; only shrinks block *contents*.
fn truncate_in_window(messages: &[Message], max_tokens: u32) -> Vec<Message> {
    // Lower a uniform per-text-block char cap until the whole window fits (or
    // we hit a floor). Monotone and bounded — no pathological spin.
    let mut cap = 8192usize;
    let mut out = cap_blocks(messages, cap);
    while cap > 256 && estimate_tokens(&out) > max_tokens {
        cap /= 2;
        out = cap_blocks(messages, cap);
    }
    out
}

/// Clone `messages`, truncating every oversized payload to fit `cap` characters:
/// text, tool-result text, and `tool_use` input (#421). An oversized thinking
/// block is replaced by a short text elision (it can't be truncated in place
/// without invalidating its signature). Blocks already within `cap` pass through
/// unchanged.
fn cap_blocks(messages: &[Message], cap: usize) -> Vec<Message> {
    messages
        .iter()
        .map(|m| Message {
            role: m.role.clone(),
            content: m.content.iter().map(|cb| cap_block(cb, cap)).collect(),
        })
        .collect()
}

fn cap_block(cb: &ContentBlock, cap: usize) -> ContentBlock {
    match cb {
        ContentBlock::Text(t) => ContentBlock::Text(TextBlock {
            text: cap_text(&t.text, cap),
            cache_control: t.cache_control,
        }),
        ContentBlock::ToolResult(tr) => ContentBlock::ToolResult(ToolResultBlock {
            tool_use_id: tr.tool_use_id.clone(),
            content: tr
                .content
                .iter()
                .map(|inner| cap_block(inner, cap))
                .collect(),
            is_error: tr.is_error,
        }),
        // #421: an oversized tool_use *input* (e.g. a Write/Edit carrying a
        // 200k-char body) is the common reason the retained window can't shrink
        // under budget — `estimate_tokens` counts it but the old `cap_block`
        // couldn't touch it, so `truncate_in_window` bottomed out still over
        // budget and hard-failed. Replace the already-executed call's input with
        // a compact placeholder: the tool_use/tool_result pairing (id) is
        // preserved and the value stays valid JSON.
        ContentBlock::ToolUse(tu) => {
            let s = tu.input.to_string();
            if s.len() > cap {
                ContentBlock::ToolUse(ToolUseBlock {
                    id: tu.id.clone(),
                    name: tu.name.clone(),
                    input: serde_json::json!({
                        "_elided_during_compaction": true,
                        "_original_input_chars": s.len(),
                    }),
                })
            } else {
                cb.clone()
            }
        }
        // #421: a thinking block can't be truncated in place (its signature would
        // no longer match), but leaving huge reasoning intact also blocks the
        // last-resort reducer. Replace an oversized one with a short text
        // elision — a plain text block carries no signature claim, so it's safe
        // to re-send.
        ContentBlock::Thinking(t) if t.thinking.len() > cap => ContentBlock::Text(TextBlock {
            text: format!(
                "[earlier reasoning elided during compaction: {} chars]",
                t.thinking.len()
            ),
            cache_control: None,
        }),
        other => other.clone(),
    }
}

/// Truncate `s` to about `cap` chars on a UTF-8 boundary, leaving a marker that
/// records how much was elided.
fn cap_text(s: &str, cap: usize) -> String {
    if s.len() <= cap {
        return s.to_string();
    }
    let mut end = cap;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n[… {} chars truncated by in-window compaction …]",
        &s[..end],
        s.len() - end
    )
}

/// Noop — never compacts.
#[derive(Debug, Default)]
pub struct NoopCompactor;

#[async_trait]
impl Compactor for NoopCompactor {
    async fn compact(
        &self,
        _messages: &[Message],
        _capabilities: &Capabilities,
    ) -> Result<Option<Compaction>> {
        Ok(None)
    }

    fn strategy_name(&self) -> &'static str {
        "Noop"
    }
}

/// Drops messages from the front (preserving leading System messages) until
/// estimated tokens drop below `target_fraction * max_input_tokens`. Always
/// keeps the last `keep_recent_turns` (User+Assistant pairs).
#[derive(Debug)]
pub struct DropOldestCompactor {
    /// Fraction of `max_input_tokens` to target after compaction (e.g. 0.7 = 70%).
    pub target_fraction: f32,
    /// Minimum number of User+Assistant turn pairs to preserve at the tail.
    pub keep_recent_turns: u32,
}

impl Default for DropOldestCompactor {
    fn default() -> Self {
        Self {
            target_fraction: 0.7,
            keep_recent_turns: 4,
        }
    }
}

#[async_trait]
impl Compactor for DropOldestCompactor {
    async fn compact(
        &self,
        messages: &[Message],
        capabilities: &Capabilities,
    ) -> Result<Option<Compaction>> {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let target =
            (f64::from(capabilities.max_input_tokens) * f64::from(self.target_fraction)) as u32;
        if estimate_tokens(messages) <= target {
            return Ok(None);
        }
        // Find leading System messages — preserved verbatim.
        let leading_system_count = messages
            .iter()
            .take_while(|m| m.role == Role::System)
            .count();
        let leading_systems = messages[..leading_system_count].to_vec();
        let body = &messages[leading_system_count..];

        // Keep the last keep_recent_turns × 2 messages of body (pairs of
        // user+assistant), but #329: snap the boundary to a clean user turn so
        // the window can't open on an orphaned tool_result / bare assistant.
        let keep = (self.keep_recent_turns as usize) * 2;
        let start = clean_tail_start(body, keep);
        let mut tail = sanitize_tool_pairs(&body[start..]);

        // #329: if the retained tail alone still exceeds the hard window (one
        // oversized tool result within the last few turns), shrink payloads in
        // place rather than hard-erroring — which would trip the consecutive-
        // failure disable and leave the run permanently overflowing.
        let systems_tokens = estimate_tokens(&leading_systems);
        let tail_budget = capabilities.max_input_tokens.saturating_sub(systems_tokens);
        if estimate_tokens(&tail) > tail_budget {
            tail = truncate_in_window(&tail, tail_budget);
        }

        let mut new_messages = leading_systems;
        new_messages.extend(tail);
        if estimate_tokens(&new_messages) > capabilities.max_input_tokens {
            return Err(Error::Compaction(
                "DropOldestCompactor: kept tail still exceeds max_input_tokens after truncation"
                    .into(),
            ));
        }
        Ok(Some(Compaction::free(new_messages)))
    }

    fn strategy_name(&self) -> &'static str {
        "DropOldest"
    }
}

/// Summarizes older turns into a single System message using the given provider.
#[derive(Clone)]
pub struct SummarizingCompactor {
    /// The provider used to generate the summary.
    pub provider: Arc<dyn Provider + Send + Sync>,
    /// Model identifier passed to the provider for the summarization call.
    pub summarizer_model: String,
    /// Fraction of `max_input_tokens` to target after compaction (e.g. 0.7 = 70%).
    pub target_fraction: f32,
    /// Minimum number of User+Assistant turn pairs to preserve at the tail.
    pub keep_recent_turns: u32,
}

impl std::fmt::Debug for SummarizingCompactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SummarizingCompactor")
            .field("summarizer_model", &self.summarizer_model)
            .field("target_fraction", &self.target_fraction)
            .field("keep_recent_turns", &self.keep_recent_turns)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl Compactor for SummarizingCompactor {
    // Split point + old-slice rendering + summary request + tail sanitize/
    // truncate keep this over the 100-line lint; it reads as one linear flow.
    #[allow(clippy::too_many_lines)]
    async fn compact(
        &self,
        messages: &[Message],
        capabilities: &Capabilities,
    ) -> Result<Option<Compaction>> {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let target =
            (f64::from(capabilities.max_input_tokens) * f64::from(self.target_fraction)) as u32;
        if estimate_tokens(messages) <= target {
            return Ok(None);
        }
        let leading_system_count = messages
            .iter()
            .take_while(|m| m.role == Role::System)
            .count();
        let leading_systems = messages[..leading_system_count].to_vec();
        let body = &messages[leading_system_count..];
        // #329: split on a clean user turn so `recent` never opens on an
        // orphaned tool_result / bare assistant, and `old` is a whole-turn
        // prefix the summarizer can render coherently.
        let keep = (self.keep_recent_turns as usize) * 2;
        let start = clean_tail_start(body, keep);
        let (old, recent) = body.split_at(start);

        if old.is_empty() {
            // Nothing to summarize. #329: if the in-window tail is itself over
            // the *hard* limit (a huge tool result within the last few turns),
            // don't no-op forever — truncate oversized payloads in place so the
            // run stops overflowing. Otherwise leave it (we're over the soft
            // target but under the hard cap, and there's nothing to summarize).
            if estimate_tokens(messages) > capabilities.max_input_tokens {
                let tail = sanitize_tool_pairs(recent);
                let budget = capabilities
                    .max_input_tokens
                    .saturating_sub(estimate_tokens(&leading_systems));
                let mut new_messages = leading_systems;
                new_messages.extend(truncate_in_window(&tail, budget));
                return Ok(Some(Compaction::free(new_messages)));
            }
            return Ok(None);
        }

        // Build a summary request.
        let summary_prompt = "Summarize the following conversation concisely, preserving any \
            tool calls, user goals, and key decisions. Output only the summary text.";

        let mut summary_messages = vec![Message::system_text(summary_prompt)];
        // Concatenate old messages into one user message. #329: render tool
        // traffic (tool_use / tool_result), not just Text — in agentic runs the
        // old turns are almost all tool calls, so a Text-only view left the
        // summarizer near-blind and it summarized nothing of substance.
        let mut combined = String::new();
        for m in old {
            let _ = writeln!(combined, "[{:?}]", m.role);
            for cb in &m.content {
                match cb {
                    ContentBlock::Text(t) => {
                        combined.push_str(&t.text);
                        combined.push_str("\n\n");
                    }
                    ContentBlock::ToolUse(tu) => {
                        let _ = writeln!(combined, "[tool_use {}({})]", tu.name, tu.input);
                    }
                    ContentBlock::ToolResult(tr) => {
                        let tag = if tr.is_error {
                            "tool_result error"
                        } else {
                            "tool_result"
                        };
                        let _ = write!(combined, "[{tag}] ");
                        for inner in &tr.content {
                            if let ContentBlock::Text(t) = inner {
                                combined.push_str(&t.text);
                            }
                        }
                        combined.push('\n');
                    }
                    ContentBlock::Thinking(t) => {
                        let _ = writeln!(combined, "[thinking] {}", t.thinking);
                    }
                    ContentBlock::Image(_) => {}
                }
            }
        }
        summary_messages.push(Message::user_text(combined));

        let req = caliban_provider::CompletionRequest {
            model: self.summarizer_model.clone(),
            messages: summary_messages,
            tools: vec![],
            tool_choice: caliban_provider::ToolChoice::None,
            max_tokens: 1024,
            temperature: Some(0.3),
            top_p: None,
            top_k: None,
            stop_sequences: vec![],
            thinking: caliban_provider::ThinkingSetting::Auto,
            effort: None,
            metadata: caliban_provider::RequestMetadata {
                user_id: None,
                purpose: Some(caliban_provider::RequestPurpose::Summarization),
            },
        };

        let resp = self
            .provider
            .complete(req)
            .await
            .map_err(|e| Error::Compaction(format!("summarizer call failed: {e}")))?;

        // #329/#292: thread the summarization spend out so the caller can fold
        // it into session usage/cost totals — otherwise autocompact is invisible.
        let usage = Some(resp.usage);

        let summary_text = resp
            .message
            .content
            .iter()
            .filter_map(|cb| match cb {
                ContentBlock::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        let mut head = leading_systems;
        head.push(Message::system_text(format!(
            "Summary of earlier conversation:\n{summary_text}"
        )));

        // #329: sanitize the recent tail — its first message may be a
        // tool_result whose tool_use is in the summarized `old` slice.
        let mut tail = sanitize_tool_pairs(recent);
        let budget = capabilities
            .max_input_tokens
            .saturating_sub(estimate_tokens(&head));
        if estimate_tokens(&tail) > budget {
            tail = truncate_in_window(&tail, budget);
        }

        let mut new_messages = head;
        new_messages.extend(tail);

        if estimate_tokens(&new_messages) > capabilities.max_input_tokens {
            return Err(Error::Compaction(
                "SummarizingCompactor: result still exceeds max_input_tokens after truncation"
                    .into(),
            ));
        }
        Ok(Some(Compaction {
            messages: new_messages,
            usage,
        }))
    }

    fn strategy_name(&self) -> &'static str {
        "Summarizing"
    }
}

#[cfg(test)]
mod microcompactor_tests {
    use super::*;
    use caliban_provider::{ContentBlock, Message, Role, TextBlock, ToolResultBlock, ToolUseBlock};
    use serde_json::json;

    fn caps() -> Capabilities {
        Capabilities {
            max_input_tokens: 1024,
            max_output_tokens: 1024,
            vision: false,
            tool_use: caliban_provider::ToolUseCapability::None,
            thinking: false,
            prompt_caching: caliban_provider::PromptCachingCapability::None,
            json_mode: false,
            streaming: false,
            stop_sequences: false,
            top_k: false,
            system_prompt: caliban_provider::SystemPromptCapability::SeparateField,
            refusal_field: false,
        }
    }

    fn read_use(id: &str, path: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse(ToolUseBlock {
                id: id.into(),
                name: "Read".into(),
                input: json!({ "file_path": path }),
            })],
        }
    }

    fn read_result(id: &str, text: &str, is_error: bool) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult(ToolResultBlock {
                tool_use_id: id.into(),
                content: vec![ContentBlock::Text(TextBlock {
                    text: text.into(),
                    cache_control: None,
                })],
                is_error,
            })],
        }
    }

    /// Pull the text of the (single) result block carrying `tool_use_id`.
    fn result_text(messages: &[Message], id: &str) -> String {
        for m in messages {
            for cb in &m.content {
                if let ContentBlock::ToolResult(tr) = cb
                    && tr.tool_use_id == id
                    && let Some(ContentBlock::Text(t)) = tr.content.first()
                {
                    return t.text.clone();
                }
            }
        }
        panic!("no result for {id}");
    }

    #[tokio::test]
    async fn error_result_does_not_supersede_successful() {
        // #170: a later *failed* Read of the same path must not destroy the
        // earlier successful content.
        let messages = vec![
            read_use("a", "/x"),
            read_result("a", "good", false),
            read_use("b", "/x"),
            read_result("b", "boom", true),
        ];
        let out = MicroCompactor::new()
            .compact(&messages, &caps())
            .await
            .unwrap()
            .expect("a superseding pair exists, so compaction applies")
            .messages;
        assert_eq!(
            result_text(&out, "a"),
            "good",
            "the successful earlier Read must be preserved, not superseded by a failed one"
        );
    }

    #[tokio::test]
    async fn successful_supersession_still_collapses() {
        // A genuinely newer *successful* Read still collapses the older one.
        let messages = vec![
            read_use("a", "/x"),
            read_result("a", "old", false),
            read_use("b", "/x"),
            read_result("b", "new", false),
        ];
        let out = MicroCompactor::new()
            .compact(&messages, &caps())
            .await
            .unwrap()
            .expect("supersession applies")
            .messages;
        assert!(
            result_text(&out, "a").starts_with("[superseded:"),
            "older successful Read should collapse to a placeholder"
        );
        assert_eq!(result_text(&out, "b"), "new", "newest result kept verbatim");
    }
}

#[cfg(test)]
mod correctness_329_tests {
    use super::*;
    use caliban_provider::{
        Capabilities, CompletionRequest, CompletionResponse, ContentBlock, Message, MessageStream,
        ModelInfo, PromptCachingCapability, Provider, Role, StopReason, SystemPromptCapability,
        TextBlock, ToolResultBlock, ToolUseBlock, ToolUseCapability, Usage,
    };
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    fn caps(max: u32) -> Capabilities {
        Capabilities {
            max_input_tokens: max,
            max_output_tokens: 1024,
            vision: false,
            tool_use: ToolUseCapability::None,
            thinking: false,
            prompt_caching: PromptCachingCapability::None,
            json_mode: false,
            streaming: false,
            stop_sequences: false,
            top_k: false,
            system_prompt: SystemPromptCapability::SeparateField,
            refusal_field: false,
        }
    }

    fn user(t: &str) -> Message {
        Message::user_text(t)
    }
    fn tool_use(id: &str, path: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse(ToolUseBlock {
                id: id.into(),
                name: "Read".into(),
                input: json!({ "file_path": path }),
            })],
        }
    }
    fn tool_result(id: &str, text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult(ToolResultBlock {
                tool_use_id: id.into(),
                content: vec![ContentBlock::Text(TextBlock {
                    text: text.into(),
                    cache_control: None,
                })],
                is_error: false,
            })],
        }
    }

    fn has_use(ms: &[Message], id: &str) -> bool {
        ms.iter().any(|m| {
            m.content
                .iter()
                .any(|cb| matches!(cb, ContentBlock::ToolUse(tu) if tu.id == id))
        })
    }
    fn has_result(ms: &[Message], id: &str) -> bool {
        ms.iter().any(|m| {
            m.content
                .iter()
                .any(|cb| matches!(cb, ContentBlock::ToolResult(tr) if tr.tool_use_id == id))
        })
    }

    /// No `tool_result` in `ms` may reference a `tool_use` that isn't also in
    /// `ms` — the exact invariant the provider enforces (Anthropic 400).
    fn assert_no_orphans(ms: &[Message]) {
        let uses: std::collections::HashSet<&str> = ms
            .iter()
            .flat_map(|m| &m.content)
            .filter_map(|cb| match cb {
                ContentBlock::ToolUse(tu) => Some(tu.id.as_str()),
                _ => None,
            })
            .collect();
        for m in ms {
            for cb in &m.content {
                if let ContentBlock::ToolResult(tr) = cb {
                    assert!(
                        uses.contains(tr.tool_use_id.as_str()),
                        "orphaned tool_result {} survived",
                        tr.tool_use_id
                    );
                }
            }
        }
    }

    // ---- Fix 1: orphan sanitizer ----
    #[test]
    fn sanitize_drops_orphan_result_and_dangling_use() {
        let msgs = vec![
            tool_result("gone", "orphan"), // result with no use
            user("hello"),
            tool_use("keep", "/f"),
            tool_result("keep", "ok"),
            tool_use("dangling", "/g"), // use with no result
        ];
        let out = sanitize_tool_pairs(&msgs);
        assert!(!has_result(&out, "gone"), "orphan result should be dropped");
        assert!(!has_use(&out, "dangling"), "dangling use should be dropped");
        assert!(
            has_use(&out, "keep") && has_result(&out, "keep"),
            "complete pair kept"
        );
        assert_no_orphans(&out);
    }

    // ---- Boundary: clean_tail_start snaps to a genuine user turn ----
    #[test]
    fn clean_tail_start_snaps_forward_to_user_turn() {
        let body = vec![
            user("first"),         // 0 clean
            tool_use("a", "/a"),   // 1
            tool_result("a", "x"), // 2 (ideal offset for desired=4 lands here)
            user("second"),        // 3 clean
            tool_use("b", "/b"),   // 4
            tool_result("b", "y"), // 5
        ];
        // desired=4 → ideal=2 is a tool_result → must snap forward to 3.
        assert_eq!(clean_tail_start(&body, 4), 3);
    }

    // ---- Fix 1 end-to-end: DropOldest never emits an orphan ----
    #[tokio::test]
    async fn dropoldest_output_has_no_orphaned_tool_result() {
        let mut msgs = vec![Message::system_text("sys")];
        for i in 0..15 {
            msgs.push(user(&format!("ask {i}")));
            msgs.push(tool_use(&format!("t{i}"), &format!("/f{i}")));
            msgs.push(tool_result(&format!("t{i}"), &"x".repeat(300)));
        }
        let c = DropOldestCompactor {
            target_fraction: 0.5,
            keep_recent_turns: 2,
        };
        let out = c
            .compact(&msgs, &caps(2000))
            .await
            .unwrap()
            .expect("over target → compacts")
            .messages;
        let first_body = out.iter().find(|m| m.role != Role::System).expect("body");
        assert_eq!(
            first_body.role,
            Role::User,
            "window must open on a user turn"
        );
        assert_no_orphans(&out);
    }

    // ---- #421: oversized tool_use *input* is truncated, not errored ----
    #[tokio::test]
    async fn dropoldest_truncates_oversized_tool_use_input() {
        // The bulk lives in the tool_use input (e.g. a Write with a 200k body),
        // not the result. The old cap_block couldn't shrink it, so the reducer
        // bottomed out over budget and hard-failed. It must now fit.
        let msgs = vec![
            Message::system_text("sys"),
            user("q"),
            tool_use("big", &"z".repeat(200_000)),
            tool_result("big", "ok"),
        ];
        let c = DropOldestCompactor {
            target_fraction: 0.5,
            keep_recent_turns: 4,
        };
        let out = c
            .compact(&msgs, &caps(4000))
            .await
            .unwrap()
            .expect("must reduce in-window, not error")
            .messages;
        assert!(
            estimate_tokens(&out) <= 4000,
            "oversized tool_use input was not shrunk: {} tokens",
            estimate_tokens(&out)
        );
        assert!(
            has_result(&out, "big"),
            "tool_use/tool_result pairing preserved through truncation"
        );
    }

    // ---- Fix 4: oversized in-window tail is truncated, not errored ----
    #[tokio::test]
    async fn dropoldest_truncates_oversized_in_window_tail() {
        // A single huge tool result inside the last kept turn: it can't be
        // dropped without losing the turn, so it must be shrunk in place.
        let msgs = vec![
            Message::system_text("sys"),
            user("q"),
            tool_use("big", "/huge"),
            tool_result("big", &"z".repeat(200_000)),
        ];
        let c = DropOldestCompactor {
            target_fraction: 0.5,
            keep_recent_turns: 4,
        };
        let out = c
            .compact(&msgs, &caps(4000))
            .await
            .unwrap()
            .expect("must reduce in-window, not no-op or error")
            .messages;
        assert!(
            estimate_tokens(&out) <= 4000,
            "in-window truncation did not fit budget: {} tokens",
            estimate_tokens(&out)
        );
        assert!(
            has_result(&out, "big"),
            "pairing preserved through truncation"
        );
        assert_no_orphans(&out);
    }

    // A provider that records the text it was asked to summarize and returns a
    // canned summary carrying a known usage, so we can assert Fix 2 + Fix 3.
    struct CapturingProvider {
        seen: Mutex<Option<String>>,
        usage: Usage,
    }

    #[async_trait]
    impl Provider for CapturingProvider {
        async fn complete(
            &self,
            req: CompletionRequest,
        ) -> caliban_provider::Result<CompletionResponse> {
            let text: String = req
                .messages
                .iter()
                .flat_map(|m| &m.content)
                .filter_map(|cb| match cb {
                    ContentBlock::Text(t) => Some(t.text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            *self.seen.lock().unwrap() = Some(text);
            Ok(CompletionResponse {
                id: "r1".into(),
                model: req.model,
                message: Message::assistant_text("SUMMARY"),
                stop_reason: StopReason::EndTurn,
                stop_sequence: None,
                usage: self.usage,
            })
        }
        async fn stream(&self, _req: CompletionRequest) -> caliban_provider::Result<MessageStream> {
            unimplemented!("summarizer only calls complete()")
        }
        fn capabilities(&self, _model: &str) -> Capabilities {
            caps(1000)
        }
        fn list_models(&self) -> Vec<ModelInfo> {
            vec![]
        }
        fn name(&self) -> &'static str {
            "capturing"
        }
    }

    // ---- Fix 2 + Fix 3: summarizer sees tool traffic, and usage is threaded ----
    #[tokio::test]
    async fn summarizer_renders_tool_traffic_and_threads_usage() {
        let usage = Usage {
            output_tokens: 42,
            ..Usage::default()
        };
        let provider = Arc::new(CapturingProvider {
            seen: Mutex::new(None),
            usage,
        });
        let c = SummarizingCompactor {
            provider: provider.clone(),
            summarizer_model: "m".into(),
            target_fraction: 0.5,
            keep_recent_turns: 1,
        };
        // Old turns are ALL tool traffic (no assistant text) — the case that
        // starved the Text-only summarizer input.
        let mut msgs = vec![user("start goal")];
        for i in 0..10 {
            msgs.push(tool_use(&format!("t{i}"), &format!("/f{i}")));
            msgs.push(tool_result(
                &format!("t{i}"),
                // Large enough that the history clears the 0.5 * 2000 target and
                // the old slice is actually summarized.
                &format!("file {i} contents {}", "x".repeat(2000)),
            ));
        }
        msgs.push(user("recent question"));

        let out = c
            .compact(&msgs, &caps(2000))
            .await
            .unwrap()
            .expect("over target → summarizes");

        // Fix 3: the summarization spend is threaded out.
        assert_eq!(out.usage.map(|u| u.output_tokens), Some(42));

        // Fix 2: the summarizer's input carried the tool traffic, not near-blank text.
        let sent = provider
            .seen
            .lock()
            .unwrap()
            .clone()
            .expect("summarizer was called");
        assert!(
            sent.contains("[tool_use Read"),
            "no tool_use in summarizer input:\n{sent}"
        );
        assert!(
            sent.contains("[tool_result]"),
            "no tool_result in summarizer input"
        );
        assert!(
            sent.contains("file 0 contents"),
            "no tool result content in summarizer input"
        );

        // And the resulting history stays valid + within budget.
        assert_no_orphans(&out.messages);
    }
}

#[cfg(test)]
mod supersession_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn read_key_is_file_path() {
        let k = supersession_key("Read", &json!({"file_path": "/x.rs"}));
        assert_eq!(k.as_deref(), Some("/x.rs"));
    }
    #[test]
    fn grep_key_is_exact_args() {
        let a = supersession_key("Grep", &json!({"pattern": "foo", "path": "."}));
        let b = supersession_key("Grep", &json!({"pattern": "foo", "path": "."}));
        let c = supersession_key("Grep", &json!({"pattern": "bar", "path": "."}));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
    #[test]
    fn bash_is_never_supersedable() {
        assert!(supersession_key("Bash", &json!({"command": "ls"})).is_none());
    }
    #[test]
    fn webfetch_keys_by_url() {
        let k = supersession_key("WebFetch", &json!({"url": "https://x", "prompt": "…"}));
        assert_eq!(k.as_deref(), Some("https://x"));
    }
}
