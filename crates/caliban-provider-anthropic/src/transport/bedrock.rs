//! AWS Bedrock transport for Anthropic Claude models.

use async_stream::try_stream;
use async_trait::async_trait;
use aws_sdk_bedrockruntime::Client as BedrockClient;
use aws_sdk_bedrockruntime::primitives::Blob;
use aws_sdk_bedrockruntime::types::ResponseStream;
use bytes::Bytes;
use futures::stream::BoxStream;

use crate::config::BedrockConfig;
use crate::error::AnthropicError;
use crate::schema::{NativeRequest, NativeResponse};
use crate::transport::Transport;

/// `Transport` impl using AWS Bedrock for the Anthropic schema family.
#[derive(Debug)]
pub struct BedrockTransport {
    client: BedrockClient,
    anthropic_version: String,
}

impl BedrockTransport {
    /// Construct a new transport from a [`BedrockConfig`].
    pub fn new(config: BedrockConfig) -> Self {
        let client = BedrockClient::new(&config.sdk_config);
        Self {
            client,
            anthropic_version: config.anthropic_version,
        }
    }
}

#[async_trait]
impl Transport for BedrockTransport {
    async fn send(&self, mut body: NativeRequest) -> Result<NativeResponse, AnthropicError> {
        let model_id = body.model.clone();
        // Bedrock: model is in the URL path, not the request body.
        // Clear the field to avoid sending a non-empty model value.
        body.model = String::new();
        body.anthropic_version = Some(self.anthropic_version.clone());
        body.stream = false;

        let body_json = serde_json::to_vec(&body)?;

        let resp = self
            .client
            .invoke_model()
            .model_id(&model_id)
            .body(Blob::new(body_json))
            .content_type("application/json")
            .accept("application/json")
            .send()
            .await
            .map_err(|e| AnthropicError::Transport(Box::new(e.into_service_error())))?;

        let body_bytes = resp.body.into_inner();
        let native: NativeResponse = serde_json::from_slice(&body_bytes)?;
        Ok(native)
    }

    async fn stream(
        &self,
        mut body: NativeRequest,
    ) -> Result<BoxStream<'static, Result<Bytes, AnthropicError>>, AnthropicError> {
        let model_id = body.model.clone();
        body.model = String::new();
        body.anthropic_version = Some(self.anthropic_version.clone());
        body.stream = true;

        let body_json = serde_json::to_vec(&body)?;

        let resp = self
            .client
            .invoke_model_with_response_stream()
            .model_id(&model_id)
            .body(Blob::new(body_json))
            .content_type("application/json")
            .accept("application/json")
            .send()
            .await
            .map_err(|e| AnthropicError::Transport(Box::new(e.into_service_error())))?;

        let mut event_receiver = resp.body;

        let stream = try_stream! {
            loop {
                let event = event_receiver.recv().await.map_err(|e| {
                    AnthropicError::Transport(Box::new(e.into_service_error()))
                })?;
                let Some(event) = event else { break };
                if let ResponseStream::Chunk(part) = event
                    && let Some(blob) = part.bytes
                {
                    let json_bytes = blob.into_inner();
                    // Reframe each Bedrock chunk as an SSE data line so the
                    // existing `stream_parse::map_sse_to_events` handles it.
                    let mut reframed = Vec::with_capacity(json_bytes.len() + 8);
                    reframed.extend_from_slice(b"data: ");
                    reframed.extend_from_slice(&json_bytes);
                    reframed.extend_from_slice(b"\n\n");
                    yield Bytes::from(reframed);
                }
                // Unknown or future variant types are skipped so the stream
                // remains usable when the SDK adds new event kinds.
            }
        };

        Ok(Box::pin(stream))
    }

    fn wire_model_id(&self, canonical: &str) -> String {
        // Translate canonical ID (e.g. "claude-3-5-sonnet") to Bedrock wire format
        // (e.g. "anthropic.claude-3-5-sonnet-20241022-v2:0").
        let native_id = crate::models::models()
            .into_iter()
            .find(|m| m.id == canonical)
            .map_or_else(|| canonical.to_string(), |m| m.native_id);

        // If already in Bedrock format, return as-is.
        if native_id.starts_with("anthropic.") {
            return native_id;
        }

        // Determine the Bedrock version suffix:
        // Sonnet 3.5 v2 and 3.7 use "v2:0"; everything else uses "v1:0".
        let version_suffix =
            if native_id.contains("3-5-sonnet-20241022") || native_id.contains("3-7-sonnet") {
                "v2:0"
            } else {
                "v1:0"
            };

        format!("anthropic.{native_id}-{version_suffix}")
    }
}
