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
pub struct DirectTransport {
    client: reqwest::Client,
    config: DirectConfig,
}

impl DirectTransport {
    /// Create a new `DirectTransport` from a `DirectConfig`.
    ///
    /// # Errors
    ///
    /// Returns `Err(AnthropicError::Http)` if the `reqwest` client cannot be built.
    pub fn new(config: DirectConfig) -> Result<Self, AnthropicError> {
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(AnthropicError::Http)?;
        Ok(Self { client, config })
    }

    fn endpoint(&self) -> String {
        let mut base = self.config.base_url.clone();
        base.set_path("/v1/messages");
        base.into()
    }

    fn auth_headers(&self) -> reqwest::header::HeaderMap {
        use reqwest::header::{HeaderMap, HeaderValue};
        let mut h = HeaderMap::new();
        h.insert(
            "x-api-key",
            HeaderValue::from_str(self.config.api_key.expose_secret()).expect("api key header"),
        );
        h.insert(
            "anthropic-version",
            HeaderValue::from_str(&self.config.anthropic_version).expect("version header"),
        );
        h.insert("content-type", HeaderValue::from_static("application/json"));
        h
    }
}

#[async_trait]
impl Transport for DirectTransport {
    async fn send(&self, body: NativeRequest) -> Result<NativeResponse, AnthropicError> {
        let resp = self
            .client
            .post(self.endpoint())
            .headers(self.auth_headers())
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AnthropicError::BadStatus {
                status: status.as_u16(),
                body,
            });
        }
        Ok(resp.json::<NativeResponse>().await?)
    }

    async fn stream(
        &self,
        body: NativeRequest,
    ) -> Result<BoxStream<'static, Result<bytes::Bytes, AnthropicError>>, AnthropicError> {
        let mut body = body;
        body.stream = true;

        let resp = self
            .client
            .post(self.endpoint())
            .headers(self.auth_headers())
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AnthropicError::BadStatus {
                status: status.as_u16(),
                body,
            });
        }
        let s = resp
            .bytes_stream()
            .map(|chunk| chunk.map_err(AnthropicError::Http));
        Ok(Box::pin(s))
    }
}
