//! Direct transport — talks to `api.openai.com` (`OpenAI`).

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use secrecy::ExposeSecret;

use crate::config::DirectConfig;
use crate::error::OpenAIError;
use crate::schema::{NativeRequest, NativeResponse};
use crate::transport::Transport;

/// Sends requests directly to the `OpenAI` HTTPS API.
#[derive(Debug)]
pub struct DirectTransport {
    client: reqwest::Client,
    stream_client: reqwest::Client,
    config: DirectConfig,
}

impl DirectTransport {
    /// Create a new `DirectTransport` from a `DirectConfig`.
    ///
    /// # Errors
    ///
    /// Returns `Err(OpenAIError::Http)` if the `reqwest` client cannot be built.
    pub fn new(config: DirectConfig) -> Result<Self, OpenAIError> {
        let client =
            caliban_common::http::build_client(config.timeout).map_err(OpenAIError::Http)?;
        // Streaming path: no total timeout by default (#254); rely on the
        // connect timeout + the agent-core WatchedStream watchdog. Honor an
        // operator-set stream total timeout if present.
        let stream_client = match config.stream_total_timeout {
            Some(total) => caliban_common::http::build_client(total),
            None => {
                caliban_common::http::build_stream_client(caliban_common::http::DEFAULT_TIMEOUT)
            }
        }
        .map_err(OpenAIError::Http)?;
        Ok(Self {
            client,
            stream_client,
            config,
        })
    }

    fn endpoint(&self) -> String {
        let mut base = self.config.base_url.clone();
        // Append /chat/completions to the configured base URL path.
        let path = format!("{}/chat/completions", base.path().trim_end_matches('/'));
        base.set_path(&path);
        base.into()
    }

    /// The `/v1/models` discovery endpoint, derived from the configured base URL
    /// the same way [`Self::endpoint`] derives `/chat/completions`.
    fn models_endpoint(&self) -> String {
        let mut base = self.config.base_url.clone();
        let path = format!("{}/models", base.path().trim_end_matches('/'));
        base.set_path(&path);
        base.into()
    }

    fn auth_headers(&self) -> Result<reqwest::header::HeaderMap, OpenAIError> {
        use reqwest::header::{HeaderMap, HeaderValue};
        let mut h = HeaderMap::new();
        let bearer = format!("Bearer {}", self.config.api_key.expose_secret());
        h.insert(
            "authorization",
            HeaderValue::from_str(&bearer).map_err(|e| OpenAIError::Transport(Box::new(e)))?,
        );
        h.insert("content-type", HeaderValue::from_static("application/json"));
        if let Some(ref org) = self.config.organization {
            h.insert(
                "openai-organization",
                HeaderValue::from_str(org).map_err(|e| OpenAIError::Transport(Box::new(e)))?,
            );
        }
        if let Some(ref project) = self.config.project {
            h.insert(
                "openai-project",
                HeaderValue::from_str(project).map_err(|e| OpenAIError::Transport(Box::new(e)))?,
            );
        }
        Ok(h)
    }
}

#[async_trait]
impl Transport for DirectTransport {
    async fn send(&self, body: NativeRequest) -> Result<NativeResponse, OpenAIError> {
        let headers = self.auth_headers()?;
        let resp = self
            .client
            .post(self.endpoint())
            .headers(headers)
            .json(&body)
            .send()
            .await?;
        let resp = caliban_provider::transport::check_status(resp, OpenAIError::bad_status).await?;
        Ok(resp.json::<NativeResponse>().await?)
    }

    async fn stream(
        &self,
        body: NativeRequest,
    ) -> Result<BoxStream<'static, Result<bytes::Bytes, OpenAIError>>, OpenAIError> {
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
        let resp = caliban_provider::transport::check_status(resp, OpenAIError::bad_status).await?;
        let s = resp
            .bytes_stream()
            .map(|chunk| chunk.map_err(OpenAIError::Http));
        Ok(Box::pin(s))
    }

    fn wire_model_id(&self, canonical: &str) -> String {
        crate::models::models()
            .into_iter()
            .find(|m| m.id == canonical)
            .map_or_else(|| canonical.to_string(), |m| m.native_id)
    }

    async fn discover_models_json(&self) -> Result<Option<serde_json::Value>, OpenAIError> {
        let headers = self.auth_headers()?;
        let resp = self
            .client
            .get(self.models_endpoint())
            .headers(headers)
            .send()
            .await?;
        // Endpoint unavailable (e.g. a server without /v1/models) is not an
        // error — the caller falls back to the static table.
        if !resp.status().is_success() {
            return Ok(None);
        }
        Ok(Some(resp.json::<serde_json::Value>().await?))
    }
}
