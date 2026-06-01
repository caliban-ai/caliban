//! `RefreshingProvider<P>` — wraps any `Provider` with on-401 key refresh.
//!
//! Holds an atomic swap of the inner provider, a reference to the
//! `ApiKeyHelperPool` from `caliban-settings`, and a rebuild closure
//! that constructs a fresh inner from a new key. On an auth-shaped
//! error (see `caliban_provider::is_auth_error`) it invalidates the
//! cached key, fetches a fresh one from the helper, rebuilds the
//! inner, and retries the request once. A second consecutive auth
//! failure propagates the original error variant unchanged.
//!
//! Lives in the binary crate (rather than `caliban-provider`) because
//! it sits at the seam between provider construction and helper
//! plumbing — both of which the binary already owns. Keeping it here
//! avoids pulling `caliban-settings` into `caliban-provider`.

use std::sync::Arc;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use caliban_provider::capabilities::{Capabilities, ModelInfo};
use caliban_provider::error::{Error, Result, is_auth_error};
use caliban_provider::provider::Provider;
use caliban_provider::request::CompletionRequest;
use caliban_provider::response::CompletionResponse;
use caliban_provider::stream::MessageStream;
use caliban_settings::ApiKeyHelperPool;
use secrecy::SecretString;

/// Rebuild closure: given a fresh API key, produce a fresh inner provider.
pub(crate) type RebuildFn<P> =
    Arc<dyn Fn(SecretString) -> std::result::Result<P, Error> + Send + Sync>;

/// Decorator that re-acquires the API key on auth-shaped failures and
/// retries the request once.
pub(crate) struct RefreshingProvider<P: Provider> {
    inner: ArcSwap<P>,
    pool: Arc<ApiKeyHelperPool>,
    provider_id: String,
    static_name: &'static str,
    rebuild: RebuildFn<P>,
}

impl<P: Provider + 'static> RefreshingProvider<P> {
    /// Wrap `inner` so a 401/403 from the API triggers a helper-script
    /// refresh + one retry. `static_name` is the value returned by the
    /// `Provider::name` trait method (must be `'static`).
    pub(crate) fn new(
        inner: P,
        pool: Arc<ApiKeyHelperPool>,
        provider_id: String,
        static_name: &'static str,
        rebuild: impl Fn(SecretString) -> std::result::Result<P, Error> + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner: ArcSwap::from_pointee(inner),
            pool,
            provider_id,
            static_name,
            rebuild: Arc::new(rebuild),
        }
    }

    fn refresh(&self) -> Result<()> {
        self.pool.invalidate(&self.provider_id);
        let outcome = self.pool.key_for(&self.provider_id).map_err(Error::Auth)?;
        let fresh = (self.rebuild)(SecretString::from(outcome.key))?;
        self.inner.store(Arc::new(fresh));
        Ok(())
    }
}

#[async_trait]
impl<P: Provider + 'static> Provider for RefreshingProvider<P> {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse> {
        let first = self.inner.load_full().complete(req.clone()).await;
        match first {
            Err(ref e) if is_auth_error(e) => {
                if let Err(refresh_err) = self.refresh() {
                    tracing::warn!(
                        target: "caliban::refreshing_provider",
                        provider = %self.provider_id,
                        error = %refresh_err,
                        "api_key_helper refresh failed; surfacing original auth error",
                    );
                    return first;
                }
                self.inner.load_full().complete(req).await
            }
            other => other,
        }
    }

    async fn stream(&self, req: CompletionRequest) -> Result<MessageStream> {
        let first = self.inner.load_full().stream(req.clone()).await;
        match first {
            Err(ref e) if is_auth_error(e) => {
                if let Err(refresh_err) = self.refresh() {
                    tracing::warn!(
                        target: "caliban::refreshing_provider",
                        provider = %self.provider_id,
                        error = %refresh_err,
                        "api_key_helper refresh failed; surfacing original auth error",
                    );
                    return first;
                }
                self.inner.load_full().stream(req).await
            }
            other => other,
        }
    }

    fn capabilities(&self, model: &str) -> Capabilities {
        self.inner.load().capabilities(model)
    }

    fn list_models(&self) -> Vec<ModelInfo> {
        self.inner.load().list_models()
    }

    fn name(&self) -> &'static str {
        self.static_name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caliban_provider::mock::MockProvider;
    use caliban_provider::{CompletionRequest, CompletionResponse, Message, StopReason, Usage};
    use caliban_settings::ApiKeyHelperRaw;
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn make_pool_with_helper(script: &str) -> Arc<ApiKeyHelperPool> {
        let mut obj = BTreeMap::new();
        obj.insert("provider".into(), Value::String("openai".into()));
        obj.insert("command".into(), Value::String("/bin/sh".into()));
        obj.insert(
            "args".into(),
            Value::Array(vec![
                Value::String("-c".into()),
                Value::String(script.into()),
            ]),
        );
        let raw = ApiKeyHelperRaw::Object(obj);
        Arc::new(ApiKeyHelperPool::from_raw(Some(&raw)))
    }

    fn dummy_request() -> CompletionRequest {
        CompletionRequest::builder("test-model")
            .user_text("hi")
            .max_tokens(16)
            .build()
            .expect("build dummy request")
    }

    fn ok_response(text: &str) -> CompletionResponse {
        CompletionResponse {
            id: "resp".into(),
            model: "test-model".into(),
            message: Message::assistant_text(text),
            stop_reason: StopReason::EndTurn,
            stop_sequence: None,
            usage: Usage::default(),
        }
    }

    fn provider_with_complete(result: Result<CompletionResponse>) -> MockProvider {
        let p = MockProvider::new();
        p.enqueue_complete(result);
        p
    }

    #[tokio::test]
    async fn refresh_on_auth_failure_retries_once() {
        let pool = make_pool_with_helper("printf sk-fresh");
        let inner = provider_with_complete(Err(Error::Auth("expired".into())));
        let counter = Arc::new(AtomicUsize::new(0));
        let counter2 = counter.clone();
        let rp = RefreshingProvider::new(inner, pool, "openai".into(), "mock", move |_key| {
            counter2.fetch_add(1, Ordering::SeqCst);
            Ok(provider_with_complete(Ok(ok_response("ok-after-refresh"))))
        });
        let resp = rp
            .complete(dummy_request())
            .await
            .expect("retry should succeed");
        assert_eq!(resp.id, "resp");
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn double_auth_propagates_original_error() {
        let pool = make_pool_with_helper("printf sk-fresh");
        let inner = provider_with_complete(Err(Error::Auth("orig".into())));
        let rp = RefreshingProvider::new(inner, pool, "openai".into(), "mock", |_key| {
            Ok(provider_with_complete(Err(Error::Auth("still bad".into()))))
        });
        let err = rp
            .complete(dummy_request())
            .await
            .expect_err("must propagate");
        match err {
            Error::Auth(msg) => assert_eq!(msg, "still bad"),
            other => panic!("expected Auth error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn non_auth_error_passes_through_without_refresh() {
        let pool = make_pool_with_helper("printf sk-fresh");
        let inner = provider_with_complete(Err(Error::RateLimit { retry_after: None }));
        let counter = Arc::new(AtomicUsize::new(0));
        let counter2 = counter.clone();
        let rp = RefreshingProvider::new(inner, pool, "openai".into(), "mock", move |_key| {
            counter2.fetch_add(1, Ordering::SeqCst);
            Ok(provider_with_complete(Ok(ok_response("never"))))
        });
        let err = rp
            .complete(dummy_request())
            .await
            .expect_err("rate limit passes through");
        assert!(matches!(err, Error::RateLimit { .. }));
        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "rebuild must not be called on non-auth error"
        );
    }

    #[tokio::test]
    async fn ok_passes_through_unchanged() {
        let pool = make_pool_with_helper("printf sk-fresh");
        let inner = provider_with_complete(Ok(ok_response("first")));
        let rebuild_called = Arc::new(AtomicUsize::new(0));
        let rc2 = rebuild_called.clone();
        let rp = RefreshingProvider::new(inner, pool, "openai".into(), "mock", move |_key| {
            rc2.fetch_add(1, Ordering::SeqCst);
            Ok(provider_with_complete(Ok(ok_response("after"))))
        });
        let resp = rp.complete(dummy_request()).await.expect("ok");
        assert_eq!(resp.id, "resp");
        assert_eq!(rebuild_called.load(Ordering::SeqCst), 0);
    }
}
