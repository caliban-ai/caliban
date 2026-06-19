//! Ollama schema family for the caliban agent harness.
//!
//! Provides [`OllamaProvider<T: Transport>`] generic over its transport.
//! Default `DirectTransport` talks to a local Ollama instance at
//! `http://localhost:11434`. No authentication is required.

#![allow(clippy::missing_errors_doc)]
// Transitive dependencies pull in multiple versions of some crates.
#![allow(clippy::multiple_crate_versions)]

pub mod config;
pub mod error;
pub mod ir_convert;
pub mod models;
pub mod schema;
pub mod transport;

mod stream_parse;

use std::collections::HashMap;
use std::sync::RwLock;

use async_trait::async_trait;
use caliban_provider::{
    Capabilities, CompletionRequest, CompletionResponse, Error, MessageStream, ModelInfo, Provider,
    Result,
};

use crate::config::DirectConfig;
use crate::transport::Transport;
use crate::transport::direct::DirectTransport;

/// Ollama `/api/chat` provider, generic over its transport.
///
/// Holds a small interior-mutable cache of server-detected context-window
/// sizes (issue #60). The sync [`Provider::capabilities`] reader cannot make
/// HTTP calls, so the async [`Provider::complete`]/[`Provider::stream`] paths
/// refresh the cache via `/api/ps` and `/api/show`, and `capabilities` overlays
/// the cached value onto the static table.
#[derive(Debug)]
pub struct OllamaProvider<T: Transport> {
    transport: T,
    /// Canonical model id → server-detected context length (input-token cap).
    ctx_cache: RwLock<HashMap<String, u32>>,
}

impl OllamaProvider<DirectTransport> {
    /// Construct an `OllamaProvider` using the direct HTTP transport.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the underlying `reqwest` client cannot be built.
    pub fn direct(cfg: DirectConfig) -> Result<Self> {
        DirectTransport::new(cfg)
            .map(Self::from_transport)
            .map_err(Error::adapter)
    }

    /// Construct an `OllamaProvider` targeting a local Ollama instance with default settings.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the underlying `reqwest` client cannot be built.
    pub fn local() -> Result<Self> {
        Self::direct(DirectConfig::local())
    }
}

impl<T: Transport> OllamaProvider<T> {
    /// Construct an `OllamaProvider` from an arbitrary `Transport`.
    pub fn from_transport(transport: T) -> Self {
        Self {
            transport,
            ctx_cache: RwLock::new(HashMap::new()),
        }
    }

    /// Read the cached context length for a canonical model id, if resolved.
    fn cached_ctx(&self, canonical: &str) -> Option<u32> {
        self.ctx_cache.read().ok()?.get(canonical).copied()
    }

    fn store_ctx(&self, canonical: &str, ctx: u32) {
        if let Ok(mut cache) = self.ctx_cache.write() {
            cache.insert(canonical.to_string(), ctx);
        }
    }

    /// Resolve a model's real context window from the server and cache it,
    /// keyed by canonical id. Resolution order (issue #60):
    ///
    /// 1. `/api/ps` `context_length` for the loaded model — the live value.
    /// 2. an already-cached value — keep it (don't downgrade, don't re-hit
    ///    `/api/show` once we have a number).
    /// 3. `/api/show` `model_info[*.context_length]` — the model maximum,
    ///    available even when the model is not loaded.
    /// 4. nothing — leave unset so `capabilities` uses the static table.
    ///
    /// Every transport error is treated as "no data" and falls through, so a
    /// server that lacks the endpoints (older Ollama) or is briefly unreachable
    /// degrades gracefully rather than failing the turn.
    pub(crate) async fn resolve_and_cache(&self, canonical: &str, wire: &str) {
        if let Ok(running) = self.transport.running_models().await
            && let Some(ctx) = running
                .iter()
                .find(|m| m.matches(wire))
                .and_then(|m| m.context_length)
            && ctx > 0
        {
            self.store_ctx(canonical, ctx);
            return;
        }

        if self.cached_ctx(canonical).is_some() {
            return;
        }

        if let Ok(Some(show)) = self.transport.show_model(wire).await
            && let Some(ctx) = show.context_length()
            && ctx > 0
        {
            self.store_ctx(canonical, ctx);
        }
    }
}

#[async_trait]
impl<T: Transport> Provider for OllamaProvider<T> {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse> {
        req.validate()?;
        let canonical_model = req.model.clone();
        let mut native = ir_convert::ir_to_native_request(req, false)?;
        native.model = self.transport.wire_model_id(&canonical_model);
        self.resolve_and_cache(&canonical_model, &native.model)
            .await;
        self.transport.finalize_request(&mut native);
        let native_resp = self.transport.send(native).await.map_err(Error::from)?;
        ir_convert::native_response_to_ir(native_resp)
    }

    async fn stream(&self, req: CompletionRequest) -> Result<MessageStream> {
        req.validate()?;
        let canonical_model = req.model.clone();
        let mut native = ir_convert::ir_to_native_request(req, true)?;
        native.model = self.transport.wire_model_id(&canonical_model);
        self.resolve_and_cache(&canonical_model, &native.model)
            .await;
        self.transport.finalize_request(&mut native);
        let bytes_stream = self.transport.stream(native).await.map_err(Error::from)?;
        Ok(stream_parse::map_ndjson_to_events(bytes_stream))
    }

    fn capabilities(&self, model: &str) -> Capabilities {
        let mut caps = models::capabilities_for(model);
        // Overlay a server-detected context window when one has been resolved
        // (issue #60); otherwise the static-table value stands.
        if let Some(ctx) = self.cached_ctx(model) {
            caps.max_input_tokens = ctx;
        }
        caps
    }

    fn list_models(&self) -> Vec<ModelInfo> {
        models::models()
    }

    async fn refresh_models(&self) -> Result<Vec<ModelInfo>> {
        // Probe the server's loaded models (`/api/ps`) and overlay each live
        // context window onto the static catalog, so the trait returns
        // server-detected data instead of the static table. Also seeds the
        // ctx cache the sync `capabilities` reader overlays (#60) — folding the
        // probe that used to be a `complete`/`stream` side effect into the
        // trait surface (the #34 refresh hook).
        let running = self.transport.running_models().await.map_err(Error::from)?;
        let mut models = models::models();
        for info in &mut models {
            if let Some(ctx) = running
                .iter()
                .find(|m| m.matches(&info.native_id))
                .and_then(|m| m.context_length)
                && ctx > 0
            {
                info.capabilities.max_input_tokens = ctx;
                self.store_ctx(&info.id, ctx);
            }
        }
        Ok(models)
    }

    fn name(&self) -> &'static str {
        "ollama"
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use bytes::Bytes;
    use futures::stream::BoxStream;

    use super::*;
    use crate::error::OllamaError;
    use crate::schema::{ModelShow, NativeRequest, NativeResponse, RunningModel};

    const MODEL: &str = "qwen3.6:27b-mlx";

    /// Transport whose `/api/ps` answer depends on how many times it has been
    /// asked, so a test can simulate "model not loaded yet → loaded → unloaded
    /// again" across turns without any HTTP. `/api/show` always reports a
    /// smaller (configured) window than `/api/ps` so we can tell which source
    /// won. `send`/`stream` are never called by `resolve_and_cache`.
    struct SequencedProbe {
        ps_calls: AtomicUsize,
    }

    #[async_trait]
    impl Transport for SequencedProbe {
        async fn send(&self, _: NativeRequest) -> std::result::Result<NativeResponse, OllamaError> {
            unimplemented!("send is not exercised by resolve_and_cache")
        }

        async fn stream(
            &self,
            _: NativeRequest,
        ) -> std::result::Result<
            BoxStream<'static, std::result::Result<Bytes, OllamaError>>,
            OllamaError,
        > {
            unimplemented!("stream is not exercised by resolve_and_cache")
        }

        async fn running_models(&self) -> std::result::Result<Vec<RunningModel>, OllamaError> {
            // Loaded only on the 2nd probe (index 1); empty otherwise.
            if self.ps_calls.fetch_add(1, Ordering::SeqCst) == 1 {
                Ok(vec![RunningModel {
                    name: MODEL.into(),
                    model: MODEL.into(),
                    context_length: Some(262_144),
                }])
            } else {
                Ok(Vec::new())
            }
        }

        async fn show_model(&self, _: &str) -> std::result::Result<Option<ModelShow>, OllamaError> {
            let body = r#"{"model_info":{"general.architecture":"qwen3_5","qwen3_5.context_length":131072}}"#;
            Ok(Some(serde_json::from_str(body).unwrap()))
        }
    }

    #[tokio::test]
    async fn ps_overrides_show_and_value_is_not_downgraded() {
        let provider = OllamaProvider::from_transport(SequencedProbe {
            ps_calls: AtomicUsize::new(0),
        });

        // Turn 1: not loaded → /api/show max (131072).
        provider.resolve_and_cache(MODEL, MODEL).await;
        assert_eq!(provider.capabilities(MODEL).max_input_tokens, 131_072);

        // Turn 2: loaded → live /api/ps value (262144) overrides the show value.
        provider.resolve_and_cache(MODEL, MODEL).await;
        assert_eq!(provider.capabilities(MODEL).max_input_tokens, 262_144);

        // Turn 3: not loaded again → keep the known-good value rather than
        // downgrading back to the smaller /api/show number.
        provider.resolve_and_cache(MODEL, MODEL).await;
        assert_eq!(provider.capabilities(MODEL).max_input_tokens, 262_144);
    }
}
