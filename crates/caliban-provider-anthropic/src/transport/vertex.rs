//! Google Vertex AI transport for Anthropic Claude models.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::{BoxStream, StreamExt};
use gcp_auth::TokenProvider;

use crate::config::VertexConfig;
use crate::error::AnthropicError;
use crate::schema::{NativeRequest, NativeResponse};
use crate::transport::Transport;

const GCP_SCOPE: &[&str] = &["https://www.googleapis.com/auth/cloud-platform"];

/// `Transport` impl using Google Vertex AI for the Anthropic schema family.
pub struct VertexTransport {
    client: reqwest::Client,
    /// Streaming client: connect-timeout only, no total deadline (#330).
    stream_client: reqwest::Client,
    token_provider: Arc<dyn TokenProvider>,
    project: String,
    region: String,
    anthropic_version: String,
}

impl std::fmt::Debug for VertexTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VertexTransport")
            .field("project", &self.project)
            .field("region", &self.region)
            .field("anthropic_version", &self.anthropic_version)
            .finish_non_exhaustive()
    }
}

impl VertexTransport {
    /// Construct a new transport from a [`VertexConfig`].
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying HTTP client cannot be constructed.
    pub fn new(config: VertexConfig) -> Result<Self, AnthropicError> {
        let client =
            caliban_common::http::build_client(config.timeout).map_err(AnthropicError::Http)?;
        let stream_client =
            caliban_common::http::build_stream_client(caliban_common::http::DEFAULT_TIMEOUT)
                .map_err(AnthropicError::Http)?;
        Ok(Self {
            client,
            stream_client,
            token_provider: config.token_provider,
            project: config.project,
            region: config.region,
            anthropic_version: config.anthropic_version,
        })
    }

    fn endpoint(&self, model: &str, streaming: bool) -> String {
        let op = if streaming {
            "streamRawPredict"
        } else {
            "rawPredict"
        };
        format!(
            "https://{region}-aiplatform.googleapis.com/v1/projects/{project}/locations/{region}/publishers/anthropic/models/{model}:{op}",
            region = self.region,
            project = self.project,
            model = model,
            op = op,
        )
    }

    async fn auth_headers(&self) -> Result<reqwest::header::HeaderMap, AnthropicError> {
        use reqwest::header::{HeaderMap, HeaderValue};

        let token = self
            .token_provider
            .token(GCP_SCOPE)
            .await
            .map_err(|e| AnthropicError::Transport(Box::new(e)))?;
        let bearer = format!("Bearer {}", token.as_str());
        let mut headers = HeaderMap::new();
        headers.insert(
            "Authorization",
            HeaderValue::from_str(&bearer).map_err(|e| AnthropicError::Transport(Box::new(e)))?,
        );
        headers.insert("Content-Type", HeaderValue::from_static("application/json"));
        Ok(headers)
    }
}

#[async_trait]
impl Transport for VertexTransport {
    async fn send(&self, mut body: NativeRequest) -> Result<NativeResponse, AnthropicError> {
        let model = body.model.clone();
        body.model = String::new(); // model is in the URL, not the body
        body.anthropic_version = Some(self.anthropic_version.clone());
        body.stream = false;

        let headers = self.auth_headers().await?;
        let resp = self
            .client
            .post(self.endpoint(&model, false))
            .headers(headers)
            .json(&body)
            .send()
            .await
            .map_err(AnthropicError::Http)?;

        let resp =
            caliban_provider::transport::check_status(resp, AnthropicError::bad_status).await?;
        resp.json::<NativeResponse>()
            .await
            .map_err(AnthropicError::Http)
    }

    async fn stream(
        &self,
        mut body: NativeRequest,
    ) -> Result<BoxStream<'static, Result<Bytes, AnthropicError>>, AnthropicError> {
        let model = body.model.clone();
        body.model = String::new(); // model is in the URL, not the body
        body.anthropic_version = Some(self.anthropic_version.clone());
        body.stream = true;

        let headers = self.auth_headers().await?;
        let resp = self
            .stream_client
            .post(self.endpoint(&model, true))
            .headers(headers)
            .json(&body)
            .send()
            .await
            .map_err(AnthropicError::Http)?;

        let resp =
            caliban_provider::transport::check_status(resp, AnthropicError::bad_status).await?;
        let s = resp.bytes_stream().map(|c| c.map_err(AnthropicError::Http));
        Ok(Box::pin(s))
    }

    fn wire_model_id(&self, canonical: &str) -> String {
        // Translate canonical ID (e.g. "claude-3-5-sonnet") to Vertex format
        // (e.g. "claude-3-5-sonnet@20241022") by replacing the last hyphen
        // before the 8-digit date with '@'.
        let native_id = crate::models::models()
            .into_iter()
            .find(|m| m.id == canonical)
            .map_or_else(|| canonical.to_string(), |m| m.native_id);

        // If already in Vertex format (contains '@'), return as-is.
        if native_id.contains('@') {
            return native_id;
        }

        // Find the last hyphen before what looks like an 8-digit date (YYYYMMDD).
        if let Some(dash_pos) = native_id.rfind('-') {
            let (prefix, suffix) = native_id.split_at(dash_pos);
            let after_dash = &suffix[1..]; // skip the '-'
            if after_dash.len() == 8 && after_dash.chars().all(|c| c.is_ascii_digit()) {
                return format!("{prefix}@{after_dash}");
            }
        }

        // Fallback: return as-is (the API will reject invalid IDs).
        native_id
    }
}
