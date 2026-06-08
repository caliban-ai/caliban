//! Direct transport — talks to a local Ollama instance.

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;

use crate::config::DirectConfig;
use crate::error::OllamaError;
use crate::schema::{ModelShow, NativeRequest, NativeResponse, RunningModel, RunningModelList};
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
        let client = caliban_common::http::default_client_builder()
            .timeout(config.timeout)
            .build()
            .map_err(OllamaError::Http)?;
        Ok(Self { client, config })
    }

    fn endpoint(&self) -> String {
        self.api_url("/api/chat")
    }

    /// Build the absolute URL for an Ollama API path, preserving any base
    /// path the operator configured (e.g. a reverse-proxy prefix).
    fn api_url(&self, suffix: &str) -> String {
        let mut base = self.config.base_url.clone();
        let path = format!("{}{suffix}", base.path().trim_end_matches('/'));
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

    async fn running_models(&self) -> Result<Vec<RunningModel>, OllamaError> {
        let resp = self.client.get(self.api_url("/api/ps")).send().await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(OllamaError::BadStatus {
                status: status.as_u16(),
                body: resp.text().await.unwrap_or_default(),
            });
        }
        Ok(resp.json::<RunningModelList>().await?.models)
    }

    async fn show_model(&self, model: &str) -> Result<Option<ModelShow>, OllamaError> {
        let resp = self
            .client
            .post(self.api_url("/api/show"))
            .header("content-type", "application/json")
            .json(&serde_json::json!({ "model": model }))
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(OllamaError::BadStatus {
                status: status.as_u16(),
                body: resp.text().await.unwrap_or_default(),
            });
        }
        Ok(Some(resp.json::<ModelShow>().await?))
    }
}
