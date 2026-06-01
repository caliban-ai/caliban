//! `api_key_helper` script invocation, caching, and 401 refresh path.
//!
//! Cache TTL defaults to 5 minutes (`refreshIntervalMs` = `300_000`)
//! or the value of `CALIBAN_API_KEY_HELPER_TTL_MS`. A 10-second slow-
//! helper warning is logged when the script takes longer to return.
//!
//! The helper is invoked **without a shell** (`Command::new` directly)
//! so we can't be argv-injected. Env vars passed to the child:
//!
//! - `CALIBAN_PROVIDER=<provider>`
//! - `CALIBAN_API_KEY_HELPER_TTL_MS=<ttl>`

use std::collections::HashMap;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::ApiKeyHelperRaw;

const DEFAULT_REFRESH_MS: u64 = 300_000;
const DEFAULT_SLOW_WARN_MS: u64 = 10_000;
const ENV_TTL: &str = "CALIBAN_API_KEY_HELPER_TTL_MS";

/// One promoted helper spec (post-normalization).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyHelperSpec {
    /// Provider this helper resolves for (`*` = fallback).
    pub provider: String,
    /// Argv: `command[0]` is the executable, `command[1..]` is argv.
    pub command: Vec<String>,
    /// Cache TTL in milliseconds.
    pub refresh_interval_ms: u64,
    /// Threshold above which a slow-helper warning fires.
    pub slow_warning_ms: u64,
}

/// Output of a single helper invocation.
#[derive(Debug, Clone)]
pub struct AuthOutcome {
    /// The API key returned on stdout (trailing newline stripped).
    pub key: String,
    /// Wall-clock elapsed during invocation.
    pub elapsed: Duration,
    /// `true` if the helper exceeded `slow_warning_ms`.
    pub slow: bool,
}

#[derive(Debug, Clone)]
struct Cached {
    key: String,
    expires_at: Instant,
}

/// Pool of `ApiKeyHelperSpec` keyed by provider plus an in-memory
/// cache.
#[derive(Debug)]
pub struct ApiKeyHelperPool {
    /// Provider → helper spec (with `*` fallback).
    specs: Vec<ApiKeyHelperSpec>,
    /// Provider → cached key.
    cache: Mutex<HashMap<String, Cached>>,
}

impl ApiKeyHelperPool {
    /// Build a pool from the raw setting form.
    #[must_use]
    pub fn from_raw(raw: Option<&ApiKeyHelperRaw>) -> Self {
        Self {
            specs: promote_helpers(raw),
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Inspect the resolved spec list (test/debug helper).
    #[must_use]
    pub fn specs(&self) -> &[ApiKeyHelperSpec] {
        &self.specs
    }

    /// Resolve the helper for a provider — exact match first, then the
    /// `"*"` fallback.
    fn spec_for(&self, provider: &str) -> Option<&ApiKeyHelperSpec> {
        self.specs
            .iter()
            .find(|h| h.provider == provider)
            .or_else(|| self.specs.iter().find(|h| h.provider == "*"))
    }

    /// Return `true` if a helper is configured for `provider` (or via the
    /// `"*"` fallback). Exposed so callers can branch between the helper
    /// path and the env-var path without invoking the script first.
    #[must_use]
    pub fn has_spec_for(&self, provider: &str) -> bool {
        self.spec_for(provider).is_some()
    }

    /// Fetch (and cache) a key. The optional `clock` lets tests inject
    /// a deterministic time source.
    ///
    /// # Errors
    /// Returns a string description on invocation failure.
    ///
    /// # Panics
    /// Panics if the internal cache mutex is poisoned (a bug in a
    /// caller — the mutex never holds across `.await` so a poison
    /// reflects a panic in another thread).
    pub fn key_for(&self, provider: &str) -> Result<AuthOutcome, String> {
        let Some(spec) = self.spec_for(provider) else {
            return Err(format!(
                "no api_key_helper configured for provider {provider}"
            ));
        };
        // Cache hit?
        {
            let map = self.cache.lock().expect("cache mutex");
            if let Some(c) = map.get(provider)
                && c.expires_at > Instant::now()
            {
                return Ok(AuthOutcome {
                    key: c.key.clone(),
                    elapsed: Duration::from_millis(0),
                    slow: false,
                });
            }
        }
        let outcome = invoke_helper(spec, provider)?;
        let ttl = ttl_for(spec);
        self.cache.lock().expect("cache mutex").insert(
            provider.to_string(),
            Cached {
                key: outcome.key.clone(),
                expires_at: Instant::now() + ttl,
            },
        );
        if outcome.slow {
            tracing::warn!(
                target: caliban_common::tracing_targets::TARGET_SETTINGS,
                provider,
                elapsed_ms = outcome.elapsed.as_millis(),
                "api_key_helper exceeded slow-warning threshold",
            );
        }
        Ok(outcome)
    }

    /// Invalidate the cached key for `provider`. Called on a 401 from
    /// the provider transport.
    ///
    /// # Panics
    /// Panics on cache-mutex poisoning (see [`Self::key_for`]).
    pub fn invalidate(&self, provider: &str) {
        self.cache.lock().expect("cache mutex").remove(provider);
    }

    /// Stash a pre-fetched key (test helper).
    #[doc(hidden)]
    pub fn cache_insert(&self, provider: &str, key: &str, ttl: Duration) {
        self.cache.lock().expect("cache mutex").insert(
            provider.to_string(),
            Cached {
                key: key.to_string(),
                expires_at: Instant::now() + ttl,
            },
        );
    }

    /// Inspect whether a cached entry exists (test helper).
    #[doc(hidden)]
    pub fn has_cached(&self, provider: &str) -> bool {
        self.cache
            .lock()
            .expect("cache mutex")
            .get(provider)
            .is_some_and(|c| c.expires_at > Instant::now())
    }
}

fn ttl_for(spec: &ApiKeyHelperSpec) -> Duration {
    let env = std::env::var(ENV_TTL).ok().and_then(|s| s.parse().ok());
    Duration::from_millis(env.unwrap_or(spec.refresh_interval_ms))
}

fn promote_helpers(raw: Option<&ApiKeyHelperRaw>) -> Vec<ApiKeyHelperSpec> {
    let Some(raw) = raw else { return Vec::new() };
    match raw {
        ApiKeyHelperRaw::Command(cmd) => vec![ApiKeyHelperSpec {
            provider: "*".into(),
            command: vec![cmd.clone()],
            refresh_interval_ms: DEFAULT_REFRESH_MS,
            slow_warning_ms: DEFAULT_SLOW_WARN_MS,
        }],
        ApiKeyHelperRaw::Object(obj) => {
            if let Some(spec) = parse_helper_obj(obj) {
                vec![spec]
            } else {
                Vec::new()
            }
        }
        ApiKeyHelperRaw::List(list) => list.iter().filter_map(parse_helper_obj).collect(),
    }
}

fn parse_helper_obj(obj: &std::collections::BTreeMap<String, Value>) -> Option<ApiKeyHelperSpec> {
    let cmd = obj.get("command").and_then(Value::as_str)?.to_string();
    let provider = obj
        .get("provider")
        .and_then(Value::as_str)
        .map_or_else(|| "*".to_string(), str::to_string);
    let mut command = vec![cmd];
    if let Some(args) = obj.get("args").and_then(Value::as_array) {
        for a in args {
            if let Some(s) = a.as_str() {
                command.push(s.to_string());
            }
        }
    }
    let refresh_interval_ms = obj
        .get("refreshIntervalMs")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_REFRESH_MS);
    let slow_warning_ms = obj
        .get("slowHelperWarningMs")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_SLOW_WARN_MS);
    Some(ApiKeyHelperSpec {
        provider,
        command,
        refresh_interval_ms,
        slow_warning_ms,
    })
}

fn invoke_helper(spec: &ApiKeyHelperSpec, provider: &str) -> Result<AuthOutcome, String> {
    if spec.command.is_empty() {
        return Err("helper command is empty".into());
    }
    let start = Instant::now();
    let mut cmd = Command::new(&spec.command[0]);
    for arg in &spec.command[1..] {
        cmd.arg(arg);
    }
    cmd.env("CALIBAN_PROVIDER", provider);
    cmd.env(ENV_TTL, spec.refresh_interval_ms.to_string());
    let output = cmd
        .output()
        .map_err(|e| format!("api_key_helper spawn failed: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "api_key_helper exited {}: {}",
            output.status,
            stderr.trim()
        ));
    }
    let elapsed = start.elapsed();
    let slow = elapsed.as_millis() > u128::from(spec.slow_warning_ms);
    let key = String::from_utf8_lossy(&output.stdout)
        .trim_end_matches('\n')
        .to_string();
    Ok(AuthOutcome { key, elapsed, slow })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn cmd_helper(script: &str) -> Vec<String> {
        vec!["/bin/sh".into(), "-c".into(), script.into()]
    }

    fn make_spec(script: &str) -> ApiKeyHelperSpec {
        ApiKeyHelperSpec {
            provider: "*".into(),
            command: cmd_helper(script),
            refresh_interval_ms: 60_000,
            slow_warning_ms: 5_000,
        }
    }

    #[test]
    fn invocation_returns_stdout_as_key() {
        let spec = make_spec("printf 'sk-abc'");
        let outcome = invoke_helper(&spec, "anthropic").unwrap();
        assert_eq!(outcome.key, "sk-abc");
        assert!(!outcome.slow);
    }

    #[test]
    fn non_zero_exit_errors() {
        let spec = make_spec("echo fail >&2; exit 1");
        let err = invoke_helper(&spec, "anthropic").unwrap_err();
        assert!(err.contains("exited"));
    }

    #[test]
    fn cache_hit_within_ttl() {
        let raw = ApiKeyHelperRaw::Object({
            let mut o = BTreeMap::new();
            o.insert("command".into(), Value::String("/bin/sh".into()));
            o.insert(
                "args".into(),
                Value::Array(vec![
                    Value::String("-c".into()),
                    Value::String("printf real-key".into()),
                ]),
            );
            o.insert("refreshIntervalMs".into(), Value::from(60_000_u64));
            o
        });
        let pool = ApiKeyHelperPool::from_raw(Some(&raw));
        // Seed cache → second call must not re-invoke (we'd notice if
        // it did because the script writes the same value, but the
        // elapsed time on a cache hit is zero).
        let first = pool.key_for("anthropic").unwrap();
        assert_eq!(first.key, "real-key");
        let second = pool.key_for("anthropic").unwrap();
        assert_eq!(second.elapsed, Duration::from_millis(0));
    }

    #[test]
    fn invalidate_forces_refresh() {
        let raw = ApiKeyHelperRaw::Object({
            let mut o = BTreeMap::new();
            o.insert("command".into(), Value::String("/bin/sh".into()));
            o.insert(
                "args".into(),
                Value::Array(vec![
                    Value::String("-c".into()),
                    Value::String("printf k1".into()),
                ]),
            );
            o.insert("refreshIntervalMs".into(), Value::from(60_000_u64));
            o
        });
        let pool = ApiKeyHelperPool::from_raw(Some(&raw));
        pool.key_for("anthropic").unwrap();
        assert!(pool.has_cached("anthropic"));
        pool.invalidate("anthropic");
        assert!(!pool.has_cached("anthropic"));
    }

    #[test]
    fn slow_helper_warning_threshold() {
        // 100ms sleep, threshold 10ms → slow=true.
        let mut spec = make_spec("sleep 0.1; printf k");
        spec.slow_warning_ms = 10;
        let outcome = invoke_helper(&spec, "anthropic").unwrap();
        assert!(outcome.slow);
    }

    #[test]
    fn bare_string_helper_promotes_to_wildcard() {
        let raw = ApiKeyHelperRaw::Command("/bin/true".into());
        let pool = ApiKeyHelperPool::from_raw(Some(&raw));
        assert_eq!(pool.specs().len(), 1);
        assert_eq!(pool.specs()[0].provider, "*");
    }

    #[test]
    fn provider_specific_helper_takes_precedence_over_wildcard() {
        let raw = ApiKeyHelperRaw::List(vec![
            {
                let mut o = BTreeMap::new();
                o.insert("provider".into(), Value::String("anthropic".into()));
                o.insert("command".into(), Value::String("/bin/anthropic-key".into()));
                o
            },
            {
                let mut o = BTreeMap::new();
                o.insert("provider".into(), Value::String("*".into()));
                o.insert("command".into(), Value::String("/bin/star-key".into()));
                o
            },
        ]);
        let pool = ApiKeyHelperPool::from_raw(Some(&raw));
        let anthropic = pool.spec_for("anthropic").unwrap();
        assert_eq!(anthropic.command, vec!["/bin/anthropic-key".to_string()]);
        let openai = pool.spec_for("openai").unwrap();
        assert_eq!(openai.command, vec!["/bin/star-key".to_string()]);
    }

    #[test]
    fn ttl_default_matches_spec_when_env_unset() {
        // Avoid mutating process env (edition 2024 forbids unsafe set_var
        // in our workspace lint config). When the env var is unset, the
        // resolved TTL must equal the spec's refreshIntervalMs.
        if std::env::var(ENV_TTL).is_err() {
            let spec = ApiKeyHelperSpec {
                provider: "*".into(),
                command: vec!["/bin/true".into()],
                refresh_interval_ms: 1_234,
                slow_warning_ms: 10_000,
            };
            assert_eq!(ttl_for(&spec), Duration::from_millis(1_234));
        }
    }
}
