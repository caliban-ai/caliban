//! Direct transport — talks to `api.anthropic.com`.

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use secrecy::ExposeSecret;

use crate::config::DirectConfig;
use crate::error::AnthropicError;
use crate::schema::{NativeRequest, NativeResponse};
use crate::transport::Transport;

/// Sends requests directly to the Anthropic HTTPS API.
#[derive(Debug)]
pub struct DirectTransport {
    client: reqwest::Client,
    /// Streaming client: connect-timeout only, **no** total deadline (#330).
    /// The total-timeout `client` would otherwise cap a long stream mid-flow;
    /// the first-byte + idle bounds live in agent-core.
    stream_client: reqwest::Client,
    config: DirectConfig,
}

impl DirectTransport {
    /// Create a new `DirectTransport` from a `DirectConfig`.
    ///
    /// # Errors
    ///
    /// Returns `Err(AnthropicError::Http)` if the `reqwest` client cannot be built.
    pub fn new(config: DirectConfig) -> Result<Self, AnthropicError> {
        let client =
            caliban_common::http::build_client(config.timeout).map_err(AnthropicError::Http)?;
        let stream_client =
            caliban_common::http::build_stream_client(caliban_common::http::DEFAULT_TIMEOUT)
                .map_err(AnthropicError::Http)?;
        Ok(Self {
            client,
            stream_client,
            config,
        })
    }

    fn endpoint(&self) -> String {
        let mut base = self.config.base_url.clone();
        base.set_path("/v1/messages");
        base.into()
    }

    fn auth_headers(&self) -> Result<reqwest::header::HeaderMap, AnthropicError> {
        use reqwest::header::{HeaderMap, HeaderValue};
        let mut h = HeaderMap::new();
        h.insert(
            "x-api-key",
            HeaderValue::from_str(self.config.api_key.expose_secret())
                .map_err(|e| AnthropicError::Transport(Box::new(e)))?,
        );
        h.insert(
            "anthropic-version",
            HeaderValue::from_str(&self.config.anthropic_version)
                .map_err(|e| AnthropicError::Transport(Box::new(e)))?,
        );
        h.insert("content-type", HeaderValue::from_static("application/json"));
        Ok(h)
    }
}

#[async_trait]
impl Transport for DirectTransport {
    async fn send(&self, body: NativeRequest) -> Result<NativeResponse, AnthropicError> {
        let headers = self.auth_headers()?;
        let resp = self
            .client
            .post(self.endpoint())
            .headers(headers)
            .json(&body)
            .send()
            .await?;
        let resp =
            caliban_provider::transport::check_status(resp, AnthropicError::bad_status).await?;
        Ok(resp.json::<NativeResponse>().await?)
    }

    async fn stream(
        &self,
        body: NativeRequest,
    ) -> Result<BoxStream<'static, Result<bytes::Bytes, AnthropicError>>, AnthropicError> {
        let mut body = body;
        body.stream = true;

        let headers = self.auth_headers()?;
        let resp = self
            .stream_client
            .post(self.endpoint())
            .headers(headers)
            .json(&body)
            .send()
            .await?;
        let resp =
            caliban_provider::transport::check_status(resp, AnthropicError::bad_status).await?;
        let s = resp
            .bytes_stream()
            .map(|chunk| chunk.map_err(AnthropicError::Http));
        Ok(Box::pin(s))
    }

    fn wire_model_id(&self, canonical: &str) -> String {
        crate::models::models()
            .into_iter()
            .find(|m| m.id == canonical)
            .map_or_else(|| canonical.to_string(), |m| m.native_id)
    }
}
