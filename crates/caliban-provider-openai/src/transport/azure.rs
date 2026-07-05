//! Azure `OpenAI` transport.
//!
//! Auth: `api-key: {key}` header (not `Authorization: Bearer`).
//! Endpoint: `{resource}.openai.azure.com/openai/deployments/{deployment}/chat/completions?api-version={ver}`.
//! Deployment routing: canonical model names are resolved via `AzureConfig.deployments`.

use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use futures::stream::BoxStream;
use secrecy::ExposeSecret;

use crate::config::AzureConfig;
use crate::error::OpenAIError;
use crate::schema::{NativeRequest, NativeResponse};
use crate::transport::Transport;

/// Sends requests to the Azure `OpenAI` Service.
#[derive(Debug)]
pub struct AzureTransport {
    client: reqwest::Client,
    /// Streaming client: connect-timeout only, no total deadline (#330).
    stream_client: reqwest::Client,
    config: AzureConfig,
}

impl AzureTransport {
    /// Create a new `AzureTransport` from an [`AzureConfig`].
    ///
    /// # Errors
    ///
    /// Returns `Err(OpenAIError::Http)` if the `reqwest` client cannot be built.
    pub fn new(config: AzureConfig) -> Result<Self, OpenAIError> {
        let client =
            caliban_common::http::build_client(config.timeout).map_err(OpenAIError::Http)?;
        let stream_client =
            caliban_common::http::build_stream_client(caliban_common::http::DEFAULT_TIMEOUT)
                .map_err(OpenAIError::Http)?;
        Ok(Self {
            client,
            stream_client,
            config,
        })
    }

    /// Build the full endpoint URL for a given deployment name.
    fn endpoint(&self, deployment: &str) -> String {
        let base = self.config.base_url.as_ref().map_or_else(
            || format!("https://{}.openai.azure.com", self.config.resource),
            |u| u.as_str().trim_end_matches('/').to_string(),
        );
        format!(
            "{base}/openai/deployments/{deployment}/chat/completions?api-version={ver}",
            ver = self.config.api_version,
        )
    }

    fn auth_headers(&self) -> Result<reqwest::header::HeaderMap, OpenAIError> {
        use reqwest::header::{HeaderMap, HeaderValue};
        let mut h = HeaderMap::new();
        h.insert(
            "api-key",
            HeaderValue::from_str(self.config.api_key.expose_secret())
                .map_err(|e| OpenAIError::Transport(Box::new(e)))?,
        );
        h.insert("content-type", HeaderValue::from_static("application/json"));
        Ok(h)
    }

    /// Resolve a canonical model name to the Azure deployment name.
    ///
    /// # Errors
    ///
    /// Returns `Err(OpenAIError::MissingConfig)` if the canonical model has no
    /// configured deployment mapping.
    fn resolve_deployment(&self, canonical: &str) -> Result<String, OpenAIError> {
        self.config
            .deployments
            .get(canonical)
            .cloned()
            .ok_or_else(|| {
                OpenAIError::MissingConfig(format!(
                    "no Azure deployment configured for model '{canonical}'"
                ))
            })
    }
}

#[async_trait]
impl Transport for AzureTransport {
    async fn send(&self, mut body: NativeRequest) -> Result<NativeResponse, OpenAIError> {
        let deployment = self.resolve_deployment(&body.model)?;
        // Azure routes by deployment URL; the body's model field is effectively ignored.
        body.model = String::new();
        body.stream = false;

        let headers = self.auth_headers()?;
        let resp = self
            .client
            .post(self.endpoint(&deployment))
            .headers(headers)
            .json(&body)
            .send()
            .await
            .map_err(OpenAIError::Http)?;
        let resp = caliban_provider::transport::check_status(resp, OpenAIError::bad_status).await?;
        Ok(resp
            .json::<NativeResponse>()
            .await
            .map_err(OpenAIError::Http)?)
    }

    async fn stream(
        &self,
        mut body: NativeRequest,
    ) -> Result<BoxStream<'static, Result<Bytes, OpenAIError>>, OpenAIError> {
        let deployment = self.resolve_deployment(&body.model)?;
        // Azure routes by deployment URL; the body's model field is effectively ignored.
        body.model = String::new();
        body.stream = true;

        let headers = self.auth_headers()?;
        let resp = self
            .stream_client
            .post(self.endpoint(&deployment))
            .headers(headers)
            .json(&body)
            .send()
            .await
            .map_err(OpenAIError::Http)?;
        let resp = caliban_provider::transport::check_status(resp, OpenAIError::bad_status).await?;
        let s = resp.bytes_stream().map(|c| c.map_err(OpenAIError::Http));
        Ok(Box::pin(s))
    }

    fn wire_model_id(&self, canonical: &str) -> String {
        // Azure routes by deployment URL; the body's model field will be cleared in
        // send()/stream().  We return the canonical name here so the Provider impl's
        // `native.model = transport.wire_model_id(canonical)` assignment gives us the
        // canonical name to look up in resolve_deployment().
        canonical.to_string()
    }
}
