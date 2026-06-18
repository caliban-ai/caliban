//! Compactor trait — strategies for truncating long histories.

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use caliban_provider::{Capabilities, Message, Provider, Role};

use crate::error::{Error, Result};

/// Compactor — strategy for keeping the message history under the model's
/// input window.
#[async_trait]
pub trait Compactor: Send + Sync {
    /// Decide whether to compact. Returns the new messages if compaction
    /// was applied; None if no-op.
    async fn compact(
        &self,
        messages: &[Message],
        capabilities: &Capabilities,
    ) -> Result<Option<Vec<Message>>>;

    /// Strategy identifier surfaced to `PreCompact` / `PostCompact` hooks.
    /// Defaults to the type's short Rust name; impls override as desired.
    fn strategy_name(&self) -> &'static str {
        "unknown"
    }
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
    ) -> Result<Option<Vec<Message>>> {
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
        Ok(Some(new))
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

/// Noop — never compacts.
#[derive(Debug, Default)]
pub struct NoopCompactor;

#[async_trait]
impl Compactor for NoopCompactor {
    async fn compact(
        &self,
        _messages: &[Message],
        _capabilities: &Capabilities,
    ) -> Result<Option<Vec<Message>>> {
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
    ) -> Result<Option<Vec<Message>>> {
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

        // Keep the last keep_recent_turns × 2 messages of body (pairs of user+assistant).
        let keep = (self.keep_recent_turns as usize) * 2;
        let body_kept = if body.len() <= keep {
            body.to_vec()
        } else {
            body[body.len() - keep..].to_vec()
        };

        let mut new_messages = leading_systems;
        new_messages.extend(body_kept);
        if estimate_tokens(&new_messages) > capabilities.max_input_tokens {
            return Err(Error::Compaction(
                "DropOldestCompactor: kept tail still exceeds max_input_tokens".into(),
            ));
        }
        Ok(Some(new_messages))
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
    async fn compact(
        &self,
        messages: &[Message],
        capabilities: &Capabilities,
    ) -> Result<Option<Vec<Message>>> {
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
        let keep = (self.keep_recent_turns as usize) * 2;
        let (old, recent) = if body.len() <= keep {
            (&body[..0], body)
        } else {
            body.split_at(body.len() - keep)
        };

        if old.is_empty() {
            // Nothing to summarize.
            return Ok(None);
        }

        // Build a summary request.
        let summary_prompt = "Summarize the following conversation concisely, preserving any \
            tool calls, user goals, and key decisions. Output only the summary text.";

        let mut summary_messages = vec![Message::system_text(summary_prompt)];
        // Concatenate old messages into one user message.
        let mut combined = String::new();
        for m in old {
            let _ = writeln!(combined, "[{:?}]", m.role);
            for cb in &m.content {
                if let caliban_provider::ContentBlock::Text(t) = cb {
                    combined.push_str(&t.text);
                    combined.push_str("\n\n");
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

        let summary_text = resp
            .message
            .content
            .iter()
            .filter_map(|cb| match cb {
                caliban_provider::ContentBlock::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        let mut new_messages = leading_systems;
        new_messages.push(Message::system_text(format!(
            "Summary of earlier conversation:\n{summary_text}"
        )));
        new_messages.extend(recent.iter().cloned());

        if estimate_tokens(&new_messages) > capabilities.max_input_tokens {
            return Err(Error::Compaction(
                "SummarizingCompactor: result still exceeds max_input_tokens".into(),
            ));
        }
        Ok(Some(new_messages))
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
            .expect("a superseding pair exists, so compaction applies");
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
            .expect("supersession applies");
        assert!(
            result_text(&out, "a").starts_with("[superseded:"),
            "older successful Read should collapse to a placeholder"
        );
        assert_eq!(result_text(&out, "b"), "new", "newest result kept verbatim");
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
