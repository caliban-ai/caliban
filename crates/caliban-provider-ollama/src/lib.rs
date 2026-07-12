//! Ollama schema family for the caliban agent harness.
//!
//! Provides [`OllamaProvider<T: Transport>`] generic over its transport.
//! Default `DirectTransport` talks to a local Ollama instance at
//! `http://localhost:11434`. No authentication is required.

#![allow(clippy::missing_errors_doc)]
// Transitive dependencies pull in multiple versions of some crates.
#![allow(clippy::multiple_crate_versions)]

pub mod config;
pub mod discovery;
pub mod error;
pub mod ir_convert;
pub mod schema;
pub mod transport;

mod stream_parse;

use std::path::PathBuf;
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
/// The Ollama server is the source of truth for the model list and their
/// capabilities (#316) — there is no static model table. `discovered` holds the
/// current models, seeded synchronously at construction from the persisted
/// cache (last successful discovery) so the sync [`Provider::capabilities`] /
/// [`Provider::list_models`] readers have correct last-known-good values
/// immediately, and refreshed by the async [`Provider::refresh_models`] and the
/// per-request [`Self::resolve_and_cache`] paths (which hit `/api/tags`,
/// `/api/show`, `/api/ps`).
#[derive(Debug)]
pub struct OllamaProvider<T: Transport> {
    transport: T,
    /// The discovered models (source of truth). Seeded from the persisted cache
    /// at construction; updated by discovery.
    discovered: RwLock<Vec<ModelInfo>>,
    /// Where the discovery result is persisted. `None` disables persistence
    /// (tests, or when no cache dir is resolvable).
    cache_path: Option<PathBuf>,
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
    /// Construct an `OllamaProvider` from an arbitrary `Transport`, persisting
    /// discovery to the default cache path for this transport's server.
    pub fn from_transport(transport: T) -> Self {
        let cache_path = discovery::default_cache_path(&transport.server_id());
        Self::from_transport_with_cache(transport, cache_path)
    }

    /// Like [`Self::from_transport`] but with an explicit cache path. `None`
    /// disables persistence (used by tests to avoid touching the real cache).
    /// The provider seeds its model list from `cache_path` if it exists.
    pub fn from_transport_with_cache(transport: T, cache_path: Option<PathBuf>) -> Self {
        let seeded = cache_path
            .as_deref()
            .map(discovery::load_cache)
            .unwrap_or_default();
        Self {
            transport,
            discovered: RwLock::new(seeded),
            cache_path,
        }
    }

    /// Look up a discovered model's capabilities by id (exact, then tag-strip).
    fn find_caps(&self, model: &str) -> Option<Capabilities> {
        let d = self.discovered.read().ok()?;
        if let Some(m) = d.iter().find(|m| m.id == model || m.native_id == model) {
            return Some(m.capabilities);
        }
        // Tag-strip: `qwen3.6:27b-mlx` → base `qwen3.6`.
        let base = model.split_once(':').map(|(b, _)| b)?;
        d.iter()
            .find(|m| m.id == base || m.native_id == base)
            .map(|m| m.capabilities)
    }

    /// Insert or replace a discovered model, keyed by id.
    fn upsert(&self, info: ModelInfo) {
        if let Ok(mut d) = self.discovered.write() {
            if let Some(existing) = d.iter_mut().find(|m| m.id == info.id) {
                *existing = info;
            } else {
                d.push(info);
            }
        }
    }

    /// Persist the current discovered set to the cache (best-effort).
    fn persist(&self) {
        if let Some(path) = &self.cache_path
            && let Ok(d) = self.discovered.read()
        {
            discovery::save_cache(path, &d);
        }
    }

    /// Resolve a single model's capabilities from the server and upsert it into
    /// the discovered set, so the active model has correct capabilities + live
    /// context even in a headless run that never opens the picker. Resolution
    /// (issue #60 + #316):
    ///
    /// - `/api/show` → capabilities (`vision`/`tools`/`thinking`) + context max.
    /// - `/api/ps` live `context_length` for the loaded model overrides the
    ///   show/max value; a known-good larger value is never downgraded by a
    ///   later, smaller `/api/show` reading.
    ///
    /// Every transport error is treated as "no data": if we learn nothing new
    /// and already know the model, this is a no-op, so a briefly-unreachable or
    /// older server degrades gracefully rather than failing the turn.
    pub(crate) async fn resolve_and_cache(&self, canonical: &str, wire: &str) {
        let existing = self.find_caps(canonical);

        // Always poll the live running window (`/api/ps`) — it's the dynamic
        // part that can change per turn (the model being (un)loaded), and the
        // per-turn upgrade to a larger live window depends on it.
        let live = self
            .transport
            .running_models()
            .await
            .ok()
            .and_then(|r| {
                r.into_iter()
                    .find(|m| m.matches(wire))
                    .and_then(|m| m.context_length)
            })
            .filter(|c| *c > 0);

        // `/api/show` is *static* model metadata, so fetch it only when we don't
        // already have resolved capabilities for this model — a warm turn skips
        // that second round-trip, cutting time-to-first-token (#425). A cached
        // *bootstrap fallback* (server briefly unreachable at first resolution)
        // still re-fetches, so we never freeze on the placeholder capabilities.
        let need_show = existing.is_none_or(|c| c == discovery::bootstrap_capabilities());
        let show = if need_show {
            self.transport.show_model(wire).await.ok().flatten()
        } else {
            None
        };

        // Nothing learned and already known → no-op.
        if live.is_none() && show.is_none() && existing.is_some() {
            return;
        }

        let mut caps = match show.as_ref() {
            Some(s) => discovery::capabilities_from_show(s),
            None => existing.unwrap_or_else(discovery::bootstrap_capabilities),
        };
        if let Some(c) = live {
            caps.max_input_tokens = c;
        } else if let Some(prev) = existing {
            // Don't downgrade a known-good (e.g. live) window with a smaller max.
            caps.max_input_tokens = caps.max_input_tokens.max(prev.max_input_tokens);
        }

        self.upsert(ModelInfo {
            id: canonical.to_string(),
            native_id: wire.to_string(),
            display_name: canonical.to_string(),
            capabilities: caps,
        });
        self.persist();
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
        // Discovered/cached value is the source of truth; an unknown model gets
        // the honest bootstrap default (never a static per-model guess).
        self.find_caps(model)
            .unwrap_or_else(discovery::bootstrap_capabilities)
    }

    fn list_models(&self) -> Vec<ModelInfo> {
        // The discovered set, seeded from the persisted cache at construction.
        self.discovered
            .read()
            .map(|d| d.clone())
            .unwrap_or_default()
    }

    async fn refresh_models(&self) -> Result<Vec<ModelInfo>> {
        // Full discovery (#316): the available models (`/api/tags`), each
        // enriched with server-reported capabilities + context (`/api/show`),
        // with the live loaded context (`/api/ps`) overlaid. The result becomes
        // the source of truth and is persisted for the next cold start. A
        // `/api/tags` failure surfaces as an error so the caller keeps the
        // last-known-good (seeded) list rather than blanking it.
        let tags = self.transport.list_tags().await.map_err(Error::from)?;
        let running = self.transport.running_models().await.unwrap_or_default();

        let mut infos = Vec::with_capacity(tags.len());
        for tag in &tags {
            // `/api/show` per model (sequential — model counts are small, and
            // this runs on picker-open, not per turn). Errors → bootstrap caps.
            let show = self.transport.show_model(&tag.name).await.ok().flatten();
            let mut caps = show.as_ref().map_or_else(
                discovery::bootstrap_capabilities,
                discovery::capabilities_from_show,
            );
            if let Some(ctx) = running
                .iter()
                .find(|m| m.matches(&tag.name))
                .and_then(|m| m.context_length)
                .filter(|c| *c > 0)
            {
                caps.max_input_tokens = ctx;
            }
            infos.push(ModelInfo {
                id: tag.name.clone(),
                native_id: tag.name.clone(),
                display_name: tag.name.clone(),
                capabilities: caps,
            });
        }

        if let Ok(mut d) = self.discovered.write() {
            d.clone_from(&infos);
        }
        self.persist();
        Ok(infos)
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
    use crate::schema::{ModelShow, NativeRequest, NativeResponse, RunningModel, TagEntry};
    use caliban_provider::ToolUseCapability;

    const MODEL: &str = "qwen3.6:27b-mlx";

    fn no_send() -> ! {
        unimplemented!("send/stream are not exercised by discovery")
    }

    /// Transport whose `/api/ps` answer depends on how many times it has been
    /// asked, so a test can simulate "model not loaded yet → loaded → unloaded
    /// again" across turns without any HTTP. `/api/show` always reports a
    /// smaller (configured) window than `/api/ps` so we can tell which source
    /// won.
    struct SequencedProbe {
        ps_calls: AtomicUsize,
    }

    #[async_trait]
    impl Transport for SequencedProbe {
        async fn send(&self, _: NativeRequest) -> std::result::Result<NativeResponse, OllamaError> {
            no_send()
        }
        async fn stream(
            &self,
            _: NativeRequest,
        ) -> std::result::Result<
            BoxStream<'static, std::result::Result<Bytes, OllamaError>>,
            OllamaError,
        > {
            no_send()
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
            let body = r#"{"model_info":{"general.architecture":"qwen3_5","qwen3_5.context_length":131072},"capabilities":["completion","tools"]}"#;
            Ok(Some(serde_json::from_str(body).unwrap()))
        }
    }

    #[tokio::test]
    async fn ps_overrides_show_and_value_is_not_downgraded() {
        // `None` cache: don't touch the real discovery cache during tests.
        let provider = OllamaProvider::from_transport_with_cache(
            SequencedProbe {
                ps_calls: AtomicUsize::new(0),
            },
            None,
        );

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

    /// Counts how many times each probe endpoint is hit, to assert the warm-turn
    /// fast path (#425): `/api/show` (static) is fetched once, `/api/ps` (live)
    /// every turn.
    struct CountingProbe {
        show_calls: AtomicUsize,
        ps_calls: AtomicUsize,
    }

    #[async_trait]
    impl Transport for CountingProbe {
        async fn send(&self, _: NativeRequest) -> std::result::Result<NativeResponse, OllamaError> {
            no_send()
        }
        async fn stream(
            &self,
            _: NativeRequest,
        ) -> std::result::Result<
            BoxStream<'static, std::result::Result<Bytes, OllamaError>>,
            OllamaError,
        > {
            no_send()
        }
        async fn running_models(&self) -> std::result::Result<Vec<RunningModel>, OllamaError> {
            self.ps_calls.fetch_add(1, Ordering::SeqCst);
            Ok(Vec::new())
        }
        async fn show_model(&self, _: &str) -> std::result::Result<Option<ModelShow>, OllamaError> {
            self.show_calls.fetch_add(1, Ordering::SeqCst);
            let body = r#"{"model_info":{"general.architecture":"qwen3_5","qwen3_5.context_length":131072},"capabilities":["completion","tools"]}"#;
            Ok(Some(serde_json::from_str(body).unwrap()))
        }
    }

    #[tokio::test]
    async fn warm_turn_skips_static_show_probe() {
        let provider = OllamaProvider::from_transport_with_cache(
            CountingProbe {
                show_calls: AtomicUsize::new(0),
                ps_calls: AtomicUsize::new(0),
            },
            None,
        );
        for _ in 0..3 {
            provider.resolve_and_cache(MODEL, MODEL).await;
        }
        // /api/show is static → fetched once (cold turn) and skipped after.
        assert_eq!(
            provider.transport.show_calls.load(Ordering::SeqCst),
            1,
            "static /api/show must be fetched only once"
        );
        // /api/ps is live → polled every turn.
        assert_eq!(
            provider.transport.ps_calls.load(Ordering::SeqCst),
            3,
            "live /api/ps must still be polled each turn"
        );
    }

    /// A full-discovery transport: two tags, per-model `/api/show` capabilities,
    /// and one model loaded in `/api/ps` with a larger live context.
    struct DiscoveryProbe;

    #[async_trait]
    impl Transport for DiscoveryProbe {
        async fn send(&self, _: NativeRequest) -> std::result::Result<NativeResponse, OllamaError> {
            no_send()
        }
        async fn stream(
            &self,
            _: NativeRequest,
        ) -> std::result::Result<
            BoxStream<'static, std::result::Result<Bytes, OllamaError>>,
            OllamaError,
        > {
            no_send()
        }
        async fn list_tags(&self) -> std::result::Result<Vec<TagEntry>, OllamaError> {
            Ok(vec![
                TagEntry {
                    name: MODEL.into(),
                    details: crate::schema::TagDetails::default(),
                },
                TagEntry {
                    name: "gemma3:1b".into(),
                    details: crate::schema::TagDetails::default(),
                },
            ])
        }
        async fn running_models(&self) -> std::result::Result<Vec<RunningModel>, OllamaError> {
            // Only the qwen model is loaded, at 256K live.
            Ok(vec![RunningModel {
                name: MODEL.into(),
                model: MODEL.into(),
                context_length: Some(262_144),
            }])
        }
        async fn show_model(
            &self,
            model: &str,
        ) -> std::result::Result<Option<ModelShow>, OllamaError> {
            let body = if model == MODEL {
                r#"{"model_info":{"general.architecture":"qwen3_5","qwen3_5.context_length":131072},"capabilities":["completion","vision","thinking","tools"]}"#
            } else {
                r#"{"model_info":{"general.architecture":"gemma3","gemma3.context_length":8192},"capabilities":["completion"]}"#
            };
            Ok(Some(serde_json::from_str(body).unwrap()))
        }
    }

    #[tokio::test]
    async fn refresh_discovers_models_with_server_caps_and_live_ctx() {
        let p = OllamaProvider::from_transport_with_cache(DiscoveryProbe, None);
        let models = p.refresh_models().await.unwrap();
        assert_eq!(models.len(), 2, "both tags discovered");

        let qwen = models.iter().find(|m| m.id == MODEL).unwrap();
        // /api/ps live (262144) wins over /api/show max (131072).
        assert_eq!(qwen.capabilities.max_input_tokens, 262_144);
        assert!(qwen.capabilities.vision);
        assert!(qwen.capabilities.thinking);
        assert_eq!(qwen.capabilities.tool_use, ToolUseCapability::ParallelCalls);

        let gemma = models.iter().find(|m| m.id == "gemma3:1b").unwrap();
        assert_eq!(gemma.capabilities.max_input_tokens, 8_192);
        assert!(!gemma.capabilities.vision);
        assert_eq!(gemma.capabilities.tool_use, ToolUseCapability::None);

        // Sync readers reflect the discovery.
        assert_eq!(p.capabilities(MODEL).max_input_tokens, 262_144);
        assert_eq!(p.list_models().len(), 2);
    }

    /// Refresh persists; a fresh provider (even with an empty server) is seeded
    /// from the cache — no wrong static default.
    #[tokio::test]
    async fn refresh_persists_and_seeds_next_start() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("ollama-test.json");
        {
            let p = OllamaProvider::from_transport_with_cache(DiscoveryProbe, Some(path.clone()));
            p.refresh_models().await.unwrap();
        }
        // New provider, empty-server transport, SAME cache → seeded from disk.
        let p2 = OllamaProvider::from_transport_with_cache(EmptyProbe, Some(path));
        assert_eq!(
            p2.list_models().len(),
            2,
            "seeded from cache before refresh"
        );
        assert_eq!(p2.capabilities(MODEL).max_input_tokens, 262_144);
    }

    /// Empty server (no tags, no show, no ps) — models a fresh/unreachable box.
    struct EmptyProbe;

    #[async_trait]
    impl Transport for EmptyProbe {
        async fn send(&self, _: NativeRequest) -> std::result::Result<NativeResponse, OllamaError> {
            no_send()
        }
        async fn stream(
            &self,
            _: NativeRequest,
        ) -> std::result::Result<
            BoxStream<'static, std::result::Result<Bytes, OllamaError>>,
            OllamaError,
        > {
            no_send()
        }
    }

    #[tokio::test]
    async fn unknown_model_uses_honest_bootstrap_not_static_32k() {
        // No discovery, no cache → the conservative bootstrap window, NOT the
        // retired static 32K fallback that caused the original bug.
        let p = OllamaProvider::from_transport_with_cache(EmptyProbe, None);
        assert_eq!(
            p.capabilities(MODEL).max_input_tokens,
            discovery::BOOTSTRAP_CONTEXT
        );
        assert_ne!(p.capabilities(MODEL).max_input_tokens, 32_768);
    }

    /// A transport whose `/api/tags` errors — refresh must fail so the caller
    /// keeps the seeded/last-known list rather than blanking it.
    struct UnreachableProbe;

    #[async_trait]
    impl Transport for UnreachableProbe {
        async fn send(&self, _: NativeRequest) -> std::result::Result<NativeResponse, OllamaError> {
            no_send()
        }
        async fn stream(
            &self,
            _: NativeRequest,
        ) -> std::result::Result<
            BoxStream<'static, std::result::Result<Bytes, OllamaError>>,
            OllamaError,
        > {
            no_send()
        }
        async fn list_tags(&self) -> std::result::Result<Vec<TagEntry>, OllamaError> {
            Err(OllamaError::Transport("unreachable".into()))
        }
    }

    #[tokio::test]
    async fn refresh_error_keeps_seeded_list() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("ollama-seed.json");
        // Seed a cache via a good discovery.
        OllamaProvider::from_transport_with_cache(DiscoveryProbe, Some(path.clone()))
            .refresh_models()
            .await
            .unwrap();
        // Unreachable server, same cache → seeded from disk; refresh errors; list intact.
        let p = OllamaProvider::from_transport_with_cache(UnreachableProbe, Some(path));
        assert_eq!(p.list_models().len(), 2);
        assert!(p.refresh_models().await.is_err());
        assert_eq!(
            p.list_models().len(),
            2,
            "seeded list unchanged after error"
        );
    }
}
