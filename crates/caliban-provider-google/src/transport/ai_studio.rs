//! AI Studio transport — talks to `generativelanguage.googleapis.com`.
//!
//! Authentication is via an API key passed as the `?key=` URL query parameter.

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use secrecy::ExposeSecret;

use crate::config::AIStudioConfig;
use crate::error::GoogleError;
use crate::schema::{NativeRequest, NativeResponse};
use crate::transport::Transport;

/// Sends requests directly to the Google AI Studio HTTPS API.
pub struct AIStudioTransport {
    client: reqwest::Client,
    config: AIStudioConfig,
}

impl std::fmt::Debug for AIStudioTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AIStudioTransport").finish_non_exhaustive()
    }
}

impl AIStudioTransport {
    /// Create a new `AIStudioTransport` from an `AIStudioConfig`.
    ///
    /// # Errors
    ///
    /// Returns `Err(GoogleError::Http)` if the `reqwest` client cannot be built.
    pub fn new(config: AIStudioConfig) -> Result<Self, GoogleError> {
        let client = caliban_common::http::default_client_builder()
            .timeout(config.timeout)
            .build()
            .map_err(GoogleError::Http)?;
        Ok(Self { client, config })
    }

    fn generate_content_url(&self, model: &str) -> String {
        let base = self.config.base_url.as_str().trim_end_matches('/');
        let ver = &self.config.api_version;
        let key = self.config.api_key.expose_secret();
        format!("{base}/{ver}/models/{model}:generateContent?key={key}")
    }

    fn stream_generate_content_url(&self, model: &str) -> String {
        let base = self.config.base_url.as_str().trim_end_matches('/');
        let ver = &self.config.api_version;
        let key = self.config.api_key.expose_secret();
        format!("{base}/{ver}/models/{model}:streamGenerateContent?alt=sse&key={key}")
    }
}

#[async_trait]
impl Transport for AIStudioTransport {
    async fn send(&self, model: &str, body: &NativeRequest) -> Result<NativeResponse, GoogleError> {
        let url = self.generate_content_url(model);
        let resp = self
            .client
            .post(&url)
            .header("content-type", "application/json")
            .json(body)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(GoogleError::BadStatus {
                status: status.as_u16(),
                body: body_text,
            });
        }
        Ok(resp.json::<NativeResponse>().await?)
    }

    async fn stream(
        &self,
        model: &str,
        body: &NativeRequest,
    ) -> Result<BoxStream<'static, Result<bytes::Bytes, GoogleError>>, GoogleError> {
        let url = self.stream_generate_content_url(model);
        let resp = self
            .client
            .post(&url)
            .header("content-type", "application/json")
            .json(body)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(GoogleError::BadStatus {
                status: status.as_u16(),
                body: body_text,
            });
        }
        let s = resp
            .bytes_stream()
            .map(|chunk| chunk.map_err(GoogleError::Http));
        Ok(Box::pin(s))
    }

    fn wire_model_id(&self, canonical: &str) -> String {
        crate::models::models()
            .into_iter()
            .find(|m| m.id == canonical)
            .map_or_else(|| canonical.to_string(), |m| m.native_id)
    }
}
