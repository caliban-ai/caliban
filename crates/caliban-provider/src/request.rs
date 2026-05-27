//! Completion request, builder, and validation.

use serde::{Deserialize, Serialize};

use crate::effort::Effort;
use crate::error::{Error, Result};
use crate::message::{ContentBlock, Message, Role};
use crate::thinking::ThinkingConfig;
use crate::tool::{Tool, ToolChoice};

/// A provider-neutral request to generate a completion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompletionRequest {
    /// The model identifier.
    pub model: String,
    /// Ordered list of conversation messages.
    pub messages: Vec<Message>,
    /// Tools available to the model.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,
    /// How the model should select tools.
    #[serde(default)]
    pub tool_choice: ToolChoice,
    /// Maximum number of tokens to generate.
    pub max_tokens: u32,
    /// Sampling temperature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Nucleus sampling probability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// Top-k sampling cutoff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    /// Sequences that stop generation when produced.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop_sequences: Vec<String>,
    /// Extended-thinking configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
    /// Reasoning-effort level. `None` (or `Some(Effort::Auto)`) means the
    /// provider's default behavior; adapters skip writing the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
    /// Optional per-request metadata.
    #[serde(default)]
    pub metadata: RequestMetadata,
}

/// Category of a request, used by the model router (when present) to pick a
/// provider/model pair. `None` (the default) falls back to whichever route is
/// declared as the default. Round-trips through serde; non-router providers
/// simply ignore the field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestPurpose {
    /// Main conversational agent loop.
    MainLoop,
    /// Summarization / compaction.
    Summarization,
    /// Small fast-classifier calls (intent detection, routing).
    FastClassifier,
    /// Sub-agent loop (a child agent spawned by `AgentTool`).
    SubAgent,
    /// Embeddings.
    Embedding,
    /// Anything else; matches a generic "Other" route if declared.
    Other,
}

/// Optional per-request metadata passed to providers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestMetadata {
    /// An opaque user identifier forwarded to the provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    /// Logical category of this request. Consumed by the model router; other
    /// providers ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<RequestPurpose>,
}

impl CompletionRequest {
    /// Create a builder for a new request targeting `model`.
    pub fn builder(model: impl Into<String>) -> CompletionRequestBuilder {
        CompletionRequestBuilder {
            model: model.into(),
            messages: Vec::new(),
            tools: Vec::new(),
            tool_choice: ToolChoice::default(),
            max_tokens: 1024,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: Vec::new(),
            thinking: None,
            effort: None,
            metadata: RequestMetadata::default(),
        }
    }

    /// Validate the request structure.
    ///
    /// # Errors
    ///
    /// Returns `Err(Error::InvalidRequest)` if the model is empty, `max_tokens` is zero,
    /// a System message appears after a User/Assistant message, a System message contains
    /// a non-text block, or there are no User or Assistant messages.
    pub fn validate(&self) -> Result<()> {
        if self.model.is_empty() {
            return Err(Error::InvalidRequest("model is empty".into()));
        }
        if self.max_tokens == 0 {
            return Err(Error::InvalidRequest("max_tokens must be > 0".into()));
        }
        validate_messages(&self.messages)
    }
}

fn validate_messages(messages: &[Message]) -> Result<()> {
    let mut seen_non_system = false;
    let mut has_user_or_assistant = false;
    for (i, msg) in messages.iter().enumerate() {
        match msg.role {
            Role::System => {
                if seen_non_system {
                    return Err(Error::InvalidRequest(format!(
                        "Role::System message at index {i} appears after a User/Assistant \
                         message; System must lead"
                    )));
                }
                for block in &msg.content {
                    if !matches!(block, ContentBlock::Text(_)) {
                        return Err(Error::InvalidRequest(format!(
                            "Role::System message at index {i} contains a non-text block"
                        )));
                    }
                }
            }
            Role::User | Role::Assistant => {
                seen_non_system = true;
                has_user_or_assistant = true;
            }
        }
    }
    if !has_user_or_assistant {
        return Err(Error::InvalidRequest(
            "request has no User or Assistant messages".into(),
        ));
    }
    Ok(())
}

/// Builder for [`CompletionRequest`].
#[must_use = "builder has no effect until .build() is called"]
pub struct CompletionRequestBuilder {
    model: String,
    messages: Vec<Message>,
    tools: Vec<Tool>,
    tool_choice: ToolChoice,
    max_tokens: u32,
    temperature: Option<f32>,
    top_p: Option<f32>,
    top_k: Option<u32>,
    stop_sequences: Vec<String>,
    thinking: Option<ThinkingConfig>,
    effort: Option<Effort>,
    metadata: RequestMetadata,
}

impl CompletionRequestBuilder {
    /// Append a system message after any existing leading System messages.
    ///
    /// Multiple calls to `.system()` preserve call order: the second call
    /// inserts after the first, not before it.
    pub fn system(mut self, text: impl Into<String>) -> Self {
        // Insert after any existing leading System messages, before the first non-System.
        let insertion_index = self
            .messages
            .iter()
            .position(|m| m.role != Role::System)
            .unwrap_or(self.messages.len());
        self.messages
            .insert(insertion_index, Message::system_text(text));
        self
    }

    /// Append a user text message.
    pub fn user_text(mut self, text: impl Into<String>) -> Self {
        self.messages.push(Message::user_text(text));
        self
    }

    /// Append an assistant text message.
    pub fn assistant_text(mut self, text: impl Into<String>) -> Self {
        self.messages.push(Message::assistant_text(text));
        self
    }

    /// Append an arbitrary message.
    pub fn message(mut self, m: Message) -> Self {
        self.messages.push(m);
        self
    }

    /// Add a tool declaration.
    pub fn tool(mut self, t: Tool) -> Self {
        self.tools.push(t);
        self
    }

    /// Set the tool-choice policy.
    pub fn tool_choice(mut self, choice: ToolChoice) -> Self {
        self.tool_choice = choice;
        self
    }

    /// Set the maximum number of output tokens.
    pub fn max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = n;
        self
    }

    /// Set the sampling temperature.
    pub fn temperature(mut self, t: f32) -> Self {
        self.temperature = Some(t);
        self
    }

    /// Set the nucleus-sampling probability.
    pub fn top_p(mut self, p: f32) -> Self {
        self.top_p = Some(p);
        self
    }

    /// Set the top-k sampling cutoff.
    pub fn top_k(mut self, k: u32) -> Self {
        self.top_k = Some(k);
        self
    }

    /// Add a stop sequence.
    pub fn stop_sequence(mut self, s: impl Into<String>) -> Self {
        self.stop_sequences.push(s.into());
        self
    }

    /// Enable extended thinking with the given configuration.
    pub fn thinking(mut self, cfg: ThinkingConfig) -> Self {
        self.thinking = Some(cfg);
        self
    }

    /// Set the reasoning-effort level. Passing `Effort::Auto` keeps the
    /// field non-`None`; adapters still treat `Auto` as "omit from the
    /// wire request".
    pub fn effort(mut self, e: Effort) -> Self {
        self.effort = Some(e);
        self
    }

    /// Attach an opaque user identifier.
    pub fn user_id(mut self, id: impl Into<String>) -> Self {
        self.metadata.user_id = Some(id.into());
        self
    }

    /// Validate and build the [`CompletionRequest`].
    ///
    /// # Errors
    ///
    /// Returns `Err(Error::InvalidRequest)` if any validation rule is violated.
    #[must_use = "discarding the Result silently ignores validation errors"]
    pub fn build(self) -> Result<CompletionRequest> {
        let req = CompletionRequest {
            model: self.model,
            messages: self.messages,
            tools: self.tools,
            tool_choice: self.tool_choice,
            max_tokens: self.max_tokens,
            temperature: self.temperature,
            top_p: self.top_p,
            top_k: self.top_k,
            stop_sequences: self.stop_sequences,
            thinking: self.thinking,
            effort: self.effort,
            metadata: self.metadata,
        };
        req.validate()?;
        Ok(req)
    }
}
