//! Configuration helpers for the `OpenRouter` provider.
//!
//! `OpenRouter` speaks `OpenAI`'s wire format, so the transport config *is*
//! [`caliban_provider_openai::config::DirectConfig`] — only the defaults differ.

use secrecy::SecretString;
use url::Url;

pub use caliban_provider_openai::config::DirectConfig;

/// `OpenRouter`'s `OpenAI`-compatible base URL.
pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";

/// Env var conventionally holding an `OpenRouter` API key.
pub const DEFAULT_API_KEY_ENV: &str = "OPENROUTER_API_KEY";

/// A [`DirectConfig`] pointed at `OpenRouter`.
///
/// # Panics
///
/// Panics if the static default base URL cannot be parsed (never in practice).
#[must_use]
pub fn direct(api_key: SecretString) -> DirectConfig {
    let mut cfg = DirectConfig::new(api_key);
    cfg.base_url = Url::parse(DEFAULT_BASE_URL).expect("static URL parses");
    cfg
}
