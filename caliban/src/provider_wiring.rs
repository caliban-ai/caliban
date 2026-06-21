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
