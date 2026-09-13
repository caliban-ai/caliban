//! Shared provider-construction plumbing for the `caliban` binary.
//!
//! Both the single-provider path ([`crate::startup::compose`]) and the
//! `caliban.toml` router path ([`crate::router`]) need the same two
//! mechanisms when wiring a concrete adapter:
//!
//! - [`resolve_key`] — pick the API key from the `api_key_helper` pool when a
//!   spec exists, else fall back to the named env var.
//! - [`wrap_with_refresh_if_helper`] — wrap an adapter in a
//!   [`crate::refreshing_provider::RefreshingProvider`] iff the pool has a
//!   spec for it (so a helper-supplied key can be re-fetched on expiry),
//!   returning the bare adapter otherwise.
//!
//! Centralizing them here removes the triplicated `RefreshingProvider`
//! boilerplate that the per-provider builders in `compose` previously
//! inlined, so startup and the router share one construction path (#165).

use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use caliban_provider::Provider;

/// Resolve the API key for `(provider_id, api_key_env)`. The
/// `api_key_helper` pool wins when a spec is configured for `provider_id`;
/// the named env var is the fallback.
pub(crate) fn resolve_key(
    provider_id: &str,
    api_key_env: &str,
    pool: &Arc<caliban_settings::ApiKeyHelperPool>,
) -> Result<secrecy::SecretString> {
    if pool.has_spec_for(provider_id) {
        let outcome = pool
            .key_for(provider_id)
            .map_err(|e| anyhow!("api_key_helper for {provider_id}: {e}"))?;
        Ok(secrecy::SecretString::from(outcome.key))
    } else {
        let key = std::env::var(api_key_env)
            .with_context(|| format!("env var {api_key_env} is unset"))?;
        Ok(secrecy::SecretString::from(key))
    }
}

/// Resolve the API key like [`resolve_key`], but tolerate a missing key when the
/// caller has overridden the endpoint's `base_url` (`base_url_overridden`).
///
/// A local OpenAI-compatible server (llama.cpp, mlx-lm, LM Studio, llama-swap)
/// ignores the bearer token entirely, and a keyed proxy rejects the request
/// itself — so a `base_url` override means "you own auth here", and an absent
/// key yields an empty bearer instead of a construction-time error (#641). The
/// canonical endpoint (no override) still requires a key. A configured
/// `api_key_helper` that fails is never masked — its error propagates.
pub(crate) fn resolve_key_optional(
    provider_id: &str,
    api_key_env: &str,
    pool: &Arc<caliban_settings::ApiKeyHelperPool>,
    base_url_overridden: bool,
) -> Result<secrecy::SecretString> {
    if pool.has_spec_for(provider_id) {
        return resolve_key(provider_id, api_key_env, pool);
    }
    match std::env::var(api_key_env) {
        Ok(key) => Ok(secrecy::SecretString::from(key)),
        Err(_) if base_url_overridden => {
            tracing::debug!(
                provider = provider_id,
                "no API key for the overridden base_url; using an empty bearer (the local endpoint enforces its own auth)"
            );
            Ok(secrecy::SecretString::from(String::new()))
        }
        Err(_) => Err(anyhow!("env var {api_key_env} is unset")),
    }
}

/// Wrap `inner` in a [`crate::refreshing_provider::RefreshingProvider`] iff
/// the pool has a spec for `provider_id`. Without a spec, no refresh path is
/// needed and the inner provider is returned as-is.
pub(crate) fn wrap_with_refresh_if_helper<P>(
    inner: P,
    pool: &Arc<caliban_settings::ApiKeyHelperPool>,
    provider_id: &str,
    static_name: &'static str,
    rebuild: impl Fn(secrecy::SecretString) -> std::result::Result<P, caliban_provider::Error>
    + Send
    + Sync
    + 'static,
) -> Arc<dyn Provider + Send + Sync>
where
    P: Provider + 'static,
{
    if pool.has_spec_for(provider_id) {
        Arc::new(crate::refreshing_provider::RefreshingProvider::new(
            inner,
            pool.clone(),
            provider_id.to_string(),
            static_name,
            rebuild,
        ))
    } else {
        Arc::new(inner)
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_key_optional;
    use secrecy::ExposeSecret as _;
    use std::sync::Arc;

    #[test]
    fn keyless_allowed_only_when_base_url_is_overridden() {
        let pool = Arc::new(caliban_settings::ApiKeyHelperPool::from_raw(None));
        // A deterministically-unset env var and no helper spec (#641).
        let var = "CALIBAN_TEST_MISSING_KEY_XYZ";
        // Overridden base_url (a local endpoint) → empty bearer, no error.
        let k = resolve_key_optional("openai", var, &pool, true).expect("keyless local ok");
        assert!(
            k.expose_secret().is_empty(),
            "local override with no key should yield an empty bearer"
        );
        // Canonical endpoint (no override) → still requires a key.
        assert!(
            resolve_key_optional("openai", var, &pool, false).is_err(),
            "canonical endpoint must still require a key"
        );
    }
}
