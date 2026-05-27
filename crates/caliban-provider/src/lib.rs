//! Provider-neutral message IR and the `Provider` trait for the caliban
//! agent harness. Adapter crates (`caliban-provider-anthropic`, etc.)
//! implement this trait for specific schema-family/transport pairs.

pub mod cache;
pub mod capabilities;
pub mod effort;
pub mod error;
pub mod message;
pub mod provider;
pub mod request;
pub mod response;
pub mod stream;
pub mod thinking;
pub mod tool;

#[cfg(feature = "mock")]
pub mod mock;

pub use cache::CacheControl;
pub use capabilities::{
    Capabilities, ModelInfo, PromptCachingCapability, SystemPromptCapability, ToolUseCapability,
};
pub use effort::Effort;
pub use error::{Error, Result};
pub use message::{ContentBlock, ImageBlock, ImageSource, Message, Role, TextBlock};
pub use provider::Provider;
pub use request::{CompletionRequest, CompletionRequestBuilder, RequestMetadata, RequestPurpose};
pub use response::{CompletionResponse, StopReason, Usage};
pub use stream::{
    MessageStream, StreamEvent, StreamingContentType, StreamingDelta, collect_message,
};
pub use thinking::{ThinkingBlock, ThinkingConfig};
pub use tool::{Tool, ToolChoice, ToolResultBlock, ToolUseBlock};

#[cfg(feature = "mock")]
pub use mock::{MockProvider, MockProviderBuilder};
