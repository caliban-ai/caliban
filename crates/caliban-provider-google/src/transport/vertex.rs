//! Google Vertex AI transport for Gemini models.
//!
//! Authenticates via `gcp_auth` (Application Default Credentials) and sends
//! requests to `{region}-aiplatform.googleapis.com` using the Gemini API shape.
//!
//! Unlike AI Studio, Vertex AI supports `fileData` URI parts for image inputs.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::{BoxStream, StreamExt};
use gcp_auth::TokenProvider;

use crate::config::VertexConfig;
use crate::error::GoogleError;
use crate::schema::{NativeRequest, NativeResponse};
use crate::transport::Transport;

const GCP_SCOPE: &[&str] = &["https://www.googleapis.com/auth/cloud-platform"];

/// `Transport` impl using Google Vertex AI for the Gemini schema family.
pub struct VertexTransport {
    client: reqwest::Client,
    token_provider: Arc<dyn TokenProvider>,
    project: String,
    region: String,
}

impl std::fmt::Debug for VertexTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VertexTransport")
            .field("project", &self.project)
            .field("region", &self.region)
            .finish_non_exhaustive()
    }
}

impl VertexTransport {
    /// Construct a new transport from a [`VertexConfig`].
    ///
    /// # Errors
    ///
    /// Returns `Err(GoogleError::Http)` if the `reqwest` client cannot be built.
    pub fn new(config: VertexConfig) -> Result<Self, GoogleError> {
        let client = caliban_common::http::default_client_builder()
            .timeout(config.timeout)
            .build()
            .map_err(GoogleError::Http)?;
        Ok(Self {
            client,
            token_provider: config.token_provider,
            project: config.project,
            region: config.region,
        })
    }

    fn endpoint(&self, model: &str, streaming: bool) -> String {
        let op = if streaming {
            "streamGenerateContent?alt=sse"
        } else {
            "generateContent"
        };
        format!(
            "https://{region}-aiplatform.googleapis.com/v1/projects/{project}/locations/{region}/publishers/google/models/{model}:{op}",
            region = self.region,
            project = self.project,
            model = model,
            op = op,
        )
    }

    async fn auth_headers(&self) -> Result<reqwest::header::HeaderMap, GoogleError> {
        use reqwest::header::{HeaderMap, HeaderValue};

        let token = self
            .token_provider
            .token(GCP_SCOPE)
            .await
            .map_err(|e| GoogleError::Transport(Box::new(e)))?;
        let bearer = format!("Bearer {}", token.as_str());
        let mut headers = HeaderMap::new();
        headers.insert(
            "Authorization",
            HeaderValue::from_str(&bearer).map_err(|e| GoogleError::Transport(Box::new(e)))?,
        );
        headers.insert("Content-Type", HeaderValue::from_static("application/json"));
        Ok(headers)
    }
}

#[async_trait]
impl Transport for VertexTransport {
    async fn send(&self, model: &str, body: &NativeRequest) -> Result<NativeResponse, GoogleError> {
        let headers = self.auth_headers().await?;
        let resp = self
            .client
            .post(self.endpoint(model, false))
            .headers(headers)
            .json(body)
            .send()
            .await
            .map_err(GoogleError::Http)?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(GoogleError::BadStatus {
                status: status.as_u16(),
                body: body_text,
            });
        }
        resp.json::<NativeResponse>()
            .await
            .map_err(GoogleError::Http)
    }

    async fn stream(
        &self,
        model: &str,
        body: &NativeRequest,
    ) -> Result<BoxStream<'static, Result<Bytes, GoogleError>>, GoogleError> {
        let headers = self.auth_headers().await?;
        let resp = self
            .client
            .post(self.endpoint(model, true))
            .headers(headers)
            .json(body)
            .send()
            .await
            .map_err(GoogleError::Http)?;

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
        // Gemini canonical IDs are identical to Vertex wire IDs.
        crate::models::models()
            .into_iter()
            .find(|m| m.id == canonical)
            .map_or_else(|| canonical.to_string(), |m| m.native_id)
    }

    fn supports_url_images(&self) -> bool {
        // Vertex AI accepts fileData URI parts; AI Studio does not.
        true
    }
}
