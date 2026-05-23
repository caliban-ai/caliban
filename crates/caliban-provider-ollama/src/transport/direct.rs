//! Direct transport — talks to a local Ollama instance.

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;

use crate::config::DirectConfig;
use crate::error::OllamaError;
use crate::schema::{NativeRequest, NativeResponse};
use crate::transport::Transport;

/// Sends requests directly to the Ollama HTTP API (no auth required).
#[derive(Debug)]
pub struct DirectTransport {
    client: reqwest::Client,
    config: DirectConfig,
}

impl DirectTransport {
    /// Create a new `DirectTransport` from a `DirectConfig`.
    ///
    /// # Errors
    ///
    /// Returns `Err(OllamaError::Http)` if the `reqwest` client cannot be built.
    pub fn new(config: DirectConfig) -> Result<Self, OllamaError> {
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(OllamaError::Http)?;
        Ok(Self { client, config })
    }

    fn endpoint(&self) -> String {
        let mut base = self.config.base_url.clone();
        let path = format!("{}/api/chat", base.path().trim_end_matches('/'));
        base.set_path(&path);
        base.into()
    }
}

#[async_trait]
impl Transport for DirectTransport {
    async fn send(&self, body: NativeRequest) -> Result<NativeResponse, OllamaError> {
        let resp = self
            .client
            .post(self.endpoint())
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(OllamaError::BadStatus {
                status: status.as_u16(),
                body: body_text,
            });
        }
        Ok(resp.json::<NativeResponse>().await?)
    }

    async fn stream(
        &self,
        body: NativeRequest,
    ) -> Result<BoxStream<'static, Result<bytes::Bytes, OllamaError>>, OllamaError> {
        let resp = self
            .client
            .post(self.endpoint())
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(OllamaError::BadStatus {
                status: status.as_u16(),
                body: body_text,
            });
        }
        let s = resp
            .bytes_stream()
            .map(|chunk| chunk.map_err(OllamaError::Http));
        Ok(Box::pin(s))
    }

    fn wire_model_id(&self, canonical: &str) -> String {
        crate::models::models()
            .into_iter()
            .find(|m| m.id == canonical)
            .map_or_else(|| canonical.to_string(), |m| m.native_id)
    }
}
