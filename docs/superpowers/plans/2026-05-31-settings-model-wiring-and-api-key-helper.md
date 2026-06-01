# Settings.model wiring + api_key_helper integration — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `Settings.model` and `api_key_helper` from `.caliban/settings.toml` actually drive runtime model selection and provider auth.

**Architecture:** New `EffectiveModel` struct centralizes resolution (CLI > Settings > Built-in). New `RefreshingProvider<P>` decorator wraps any `Provider` to invalidate + re-acquire the API key on 401 via the existing `ApiKeyHelperPool`. Helper integration goes in both single-provider (`startup::build_provider`) and router (`router::build_one`) paths.

**Tech Stack:** Rust 2024, `async_trait`, `arc-swap`, `tokio`, `secrecy::SecretString`, existing `caliban-provider` trait.

**Spec:** `docs/superpowers/specs/2026-05-31-settings-model-wiring-and-api-key-helper-design.md`

---

## Task 1: `is_auth_error` classifier

**Files:**
- Modify: `crates/caliban-provider/src/error.rs`

- [ ] **Step 1: Write failing tests** — append to `error.rs::tests`:

```rust
#[test]
fn is_auth_error_matches_explicit_auth_variant() {
    assert!(is_auth_error(&Error::Auth("bad key".into())));
}

#[test]
fn is_auth_error_matches_server_error_401() {
    assert!(is_auth_error(&Error::ServerError {
        status: 401,
        body: "unauthorized".into(),
    }));
}

#[test]
fn is_auth_error_matches_server_error_403() {
    assert!(is_auth_error(&Error::ServerError {
        status: 403,
        body: "forbidden".into(),
    }));
}

#[test]
fn is_auth_error_rejects_other_server_errors() {
    assert!(!is_auth_error(&Error::ServerError {
        status: 500,
        body: "boom".into(),
    }));
    assert!(!is_auth_error(&Error::ServerError {
        status: 429,
        body: "slow down".into(),
    }));
}

#[test]
fn is_auth_error_rejects_unrelated_variants() {
    assert!(!is_auth_error(&Error::RateLimit { retry_after: None }));
    assert!(!is_auth_error(&Error::InvalidRequest("nope".into())));
    assert!(!is_auth_error(&Error::Cancelled));
}
```

- [ ] **Step 2: Run tests, confirm failure** — `cargo test -p caliban-provider is_auth_error` → expect "cannot find function `is_auth_error`".

- [ ] **Step 3: Implement** — add to `error.rs` after the `Error` impl:

```rust
/// Classify an error as authentication-shaped. Used by `RefreshingProvider`
/// to decide whether to invalidate the cached API key and retry.
#[must_use]
pub fn is_auth_error(err: &Error) -> bool {
    match err {
        Error::Auth(_) => true,
        Error::ServerError { status, .. } => *status == 401 || *status == 403,
        _ => false,
    }
}
```

- [ ] **Step 4: Re-export from lib.rs** — modify `crates/caliban-provider/src/lib.rs`:

```rust
pub use error::{Error, Result, is_auth_error};
```

- [ ] **Step 5: Run tests** — `cargo test -p caliban-provider is_auth_error` → expect PASS.

- [ ] **Step 6: Commit:**
```bash
git add crates/caliban-provider/src/error.rs crates/caliban-provider/src/lib.rs
git commit -m "feat(provider): add is_auth_error classifier for RefreshingProvider"
```

---

## Task 2: `arc-swap` dependency

**Files:**
- Modify: `crates/caliban-provider/Cargo.toml`

- [ ] **Step 1: Check workspace** — `rg '^arc-swap' Cargo.toml crates/*/Cargo.toml` to find existing dep version.

- [ ] **Step 2: Add to caliban-provider** — under `[dependencies]`:

```toml
arc-swap = "1"
```

If already in workspace, use `arc-swap = { workspace = true }` style if other deps use it.

- [ ] **Step 3: Verify build** — `cargo check -p caliban-provider`.

- [ ] **Step 4: Commit (squash into Task 3 if no other changes worth committing).**

---

## Task 3: `RefreshingProvider<P>` decorator

**Files:**
- Create: `crates/caliban-provider/src/refreshing.rs`
- Modify: `crates/caliban-provider/src/lib.rs`

- [ ] **Step 1: Write failing tests** — bottom of `refreshing.rs`:

```rust
#[cfg(test)]
#[cfg(feature = "mock")]
mod tests {
    use super::*;
    use crate::mock::MockProviderBuilder;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Returns Auth, then succeeds on rebuild's inner.
    fn one_auth_then_ok() -> Box<dyn Fn(SecretString) -> Result<MockProvider> + Send + Sync> {
        let count = std::sync::Arc::new(AtomicUsize::new(0));
        Box::new(move |_key| {
            let n = count.fetch_add(1, Ordering::SeqCst);
            let provider = if n == 0 {
                MockProviderBuilder::new().fail_with(Error::Auth("expired".into())).build()
            } else {
                MockProviderBuilder::new().respond_text("ok").build()
            };
            Ok(provider)
        })
    }

    #[tokio::test]
    async fn refresh_on_auth_failure_retries_once() {
        // Pool with a helper that prints a fresh key on each call.
        let pool = make_pool_with_helper("printf k2");
        let inner = MockProviderBuilder::new()
            .fail_with(Error::Auth("expired".into()))
            .build();
        let rp = RefreshingProvider::new(
            inner,
            pool,
            "openai".into(),
            one_auth_then_ok(),
        );
        let res = rp.complete(dummy_req()).await;
        assert!(res.is_ok(), "expected retry to succeed, got {res:?}");
    }

    #[tokio::test]
    async fn double_auth_propagates_original_error() {
        let pool = make_pool_with_helper("printf k");
        let rebuild = Box::new(|_k: SecretString| {
            Ok(MockProviderBuilder::new().fail_with(Error::Auth("still bad".into())).build())
        });
        let inner = MockProviderBuilder::new()
            .fail_with(Error::Auth("orig".into()))
            .build();
        let rp = RefreshingProvider::new(inner, pool, "openai".into(), rebuild);
        let res = rp.complete(dummy_req()).await;
        assert!(matches!(res, Err(Error::Auth(msg)) if msg == "orig"));
    }

    #[tokio::test]
    async fn non_auth_error_passes_through_without_refresh() {
        let pool = make_pool_with_helper("printf k");
        let inner = MockProviderBuilder::new()
            .fail_with(Error::RateLimit { retry_after: None })
            .build();
        let rebuild_called = std::sync::Arc::new(AtomicUsize::new(0));
        let rebuild_called2 = rebuild_called.clone();
        let rebuild = Box::new(move |_k: SecretString| {
            rebuild_called2.fetch_add(1, Ordering::SeqCst);
            Ok(MockProviderBuilder::new().build())
        });
        let rp = RefreshingProvider::new(inner, pool, "openai".into(), rebuild);
        let _ = rp.complete(dummy_req()).await;
        assert_eq!(rebuild_called.load(Ordering::SeqCst), 0);
    }

    // ----- helpers below --------------------------------------------------

    fn dummy_req() -> crate::CompletionRequest {
        // simplest builder call
        crate::CompletionRequestBuilder::new("m").build().unwrap()
    }

    fn make_pool_with_helper(script: &str) -> std::sync::Arc<caliban_settings::ApiKeyHelperPool> {
        use caliban_settings::ApiKeyHelperRaw;
        use std::collections::BTreeMap;
        let mut obj = BTreeMap::new();
        obj.insert("command".into(), serde_json::Value::String("/bin/sh".into()));
        obj.insert("args".into(), serde_json::json!(["-c", script]));
        let raw = ApiKeyHelperRaw::Object(obj);
        std::sync::Arc::new(caliban_settings::ApiKeyHelperPool::from_raw(Some(&raw)))
    }
}
```

> **Note:** if the `mock` feature in `caliban-provider` lacks `fail_with`, add it as a minimal builder method in this task. If MockProviderBuilder API differs, adjust tests to current shape — the *intent* is what matters: an inner provider that fails with a given Error, and a rebuild closure with a counter.

- [ ] **Step 2: Run, confirm failure** — `cargo test -p caliban-provider refreshing` → expect "module not found".

- [ ] **Step 3: Implement decorator** — create `refreshing.rs`:

```rust
//! `RefreshingProvider<P>` — wraps any `Provider` with on-401 key refresh.
//!
//! Holds an atomic swap of the inner provider, a reference to the
//! `ApiKeyHelperPool` from `caliban-settings`, and a rebuild closure
//! that constructs a fresh inner from a new key. On an auth-shaped
//! error (see `is_auth_error`) it invalidates the cached key, fetches
//! a fresh one from the helper, rebuilds the inner, and retries the
//! request once.

use std::sync::Arc;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use secrecy::SecretString;

use crate::capabilities::{Capabilities, ModelInfo};
use crate::error::{Error, Result, is_auth_error};
use crate::provider::Provider;
use crate::request::CompletionRequest;
use crate::response::CompletionResponse;
use crate::stream::MessageStream;
use caliban_settings::ApiKeyHelperPool;

/// Rebuild closure: given a fresh key, produce a fresh inner provider.
pub type RebuildFn<P> =
    Arc<dyn Fn(SecretString) -> std::result::Result<P, Error> + Send + Sync>;

/// Decorator that re-acquires the API key on auth-shaped failures.
pub struct RefreshingProvider<P: Provider> {
    inner: ArcSwap<P>,
    pool: Arc<ApiKeyHelperPool>,
    provider_id: String,
    rebuild: RebuildFn<P>,
}

impl<P: Provider + 'static> RefreshingProvider<P> {
    /// Wrap `inner` so a 401 from the API triggers a helper-script
    /// refresh + one retry.
    pub fn new(
        inner: P,
        pool: Arc<ApiKeyHelperPool>,
        provider_id: String,
        rebuild: impl Fn(SecretString) -> std::result::Result<P, Error> + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner: ArcSwap::from_pointee(inner),
            pool,
            provider_id,
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
                        target: "caliban_provider::refresh",
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
                        target: "caliban_provider::refresh",
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
        self.inner.load().name()
    }
}
```

- [ ] **Step 4: Wire module + dep** — modify `crates/caliban-provider/src/lib.rs`:

```rust
pub mod refreshing;
pub use refreshing::RefreshingProvider;
```

And ensure `caliban-provider/Cargo.toml` has:

```toml
arc-swap = "1"
caliban-settings = { path = "../caliban-settings" }
```

> **Cycle check:** confirm `caliban-settings` does NOT depend on `caliban-provider`. If it does, the rebuild closure type stays in `caliban-provider` but the helper-pool reference comes from a trait the binary owns. Likely fine — settings is upstream of provider conceptually.

- [ ] **Step 5: Run tests** — `cargo test -p caliban-provider refreshing --features mock` → expect PASS.

- [ ] **Step 6: Commit:**
```bash
git add crates/caliban-provider/
git commit -m "feat(provider): RefreshingProvider<P> decorator for api_key_helper 401 refresh"
```

---

## Task 4: `EffectiveModel` resolver

**Files:**
- Create: `caliban/src/effective_model.rs`
- Modify: `caliban/src/main.rs` (just `mod effective_model;`)

- [ ] **Step 1: Write failing tests** — at the bottom of `effective_model.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use caliban_settings::{ModelSelector, Settings};

    fn args(provider: Option<ProviderKind>, model: Option<&str>) -> crate::args::Args {
        // Minimal Args constructor — fill required defaults; use clap's
        // try_parse_from for the actual struct shape.
        use clap::Parser;
        let mut argv: Vec<String> = vec!["caliban".into()];
        if let Some(p) = provider {
            argv.push("--provider".into());
            argv.push(match p {
                ProviderKind::Anthropic => "anthropic",
                ProviderKind::Openai => "openai",
                ProviderKind::Ollama => "ollama",
                ProviderKind::Google => "google",
            }.into());
        }
        if let Some(m) = model {
            argv.push("--model".into());
            argv.push(m.into());
        }
        crate::args::Args::try_parse_from(argv).expect("parse args")
    }

    #[test]
    fn cli_provider_and_model_win() {
        let mut s = Settings::default();
        s.model = Some(ModelSelector::Qualified {
            provider: "openai".into(),
            name: "gpt-4o".into(),
        });
        let eff = EffectiveModel::resolve(&args(Some(ProviderKind::Anthropic), Some("claude-haiku-4-7")), &s).unwrap();
        assert!(matches!(eff.provider, ProviderKind::Anthropic));
        assert_eq!(eff.name, "claude-haiku-4-7");
        assert_eq!(eff.source, ModelSource::Cli);
    }

    #[test]
    fn settings_qualified_picks_provider_and_model() {
        let mut s = Settings::default();
        s.model = Some(ModelSelector::Qualified {
            provider: "openai".into(),
            name: "gpt-4o".into(),
        });
        let eff = EffectiveModel::resolve(&args(None, None), &s).unwrap();
        assert!(matches!(eff.provider, ProviderKind::Openai));
        assert_eq!(eff.name, "gpt-4o");
        assert_eq!(eff.source, ModelSource::Settings);
    }

    #[test]
    fn settings_bare_name_keeps_provider_default_warns() {
        let mut s = Settings::default();
        s.model = Some(ModelSelector::Name("gpt-4o".into()));
        let eff = EffectiveModel::resolve(&args(None, None), &s).unwrap();
        // No provider info — falls back to the builtin default provider.
        assert!(matches!(eff.provider, ProviderKind::Anthropic));
        assert_eq!(eff.name, "gpt-4o");
    }

    #[test]
    fn cli_model_only_takes_settings_provider() {
        let mut s = Settings::default();
        s.model = Some(ModelSelector::Qualified {
            provider: "openai".into(),
            name: "gpt-4o".into(),
        });
        let eff = EffectiveModel::resolve(&args(None, Some("gpt-5.5")), &s).unwrap();
        assert!(matches!(eff.provider, ProviderKind::Openai));
        assert_eq!(eff.name, "gpt-5.5");
    }

    #[test]
    fn nothing_set_falls_back_to_builtin_default() {
        let s = Settings::default();
        let eff = EffectiveModel::resolve(&args(None, None), &s).unwrap();
        assert!(matches!(eff.provider, ProviderKind::Anthropic));
        assert_eq!(eff.name, "claude-sonnet-4-6");
        assert_eq!(eff.source, ModelSource::BuiltinDefault);
    }

    #[test]
    fn fallback_model_lifts_from_settings() {
        let mut s = Settings::default();
        s.fallback_model = Some(ModelSelector::Qualified {
            provider: "anthropic".into(),
            name: "claude-haiku-4-7".into(),
        });
        let eff = EffectiveModel::resolve(&args(None, None), &s).unwrap();
        assert_eq!(
            eff.fallback,
            Some((ProviderKind::Anthropic, "claude-haiku-4-7".into())),
        );
    }
}
```

- [ ] **Step 2: Run, confirm failure** — `cargo test -p caliban effective_model` → "module not found / cannot find type".

- [ ] **Step 3: Implement** — create `effective_model.rs`:

```rust
//! `EffectiveModel` — the resolved provider/model pair the binary
//! actually runs against. Built once in `main.rs` from the CLI args
//! and the merged `Settings` snapshot; threaded into every site that
//! previously called `default_model_for(args.provider)`.

use anyhow::Result;
use caliban_settings::{ModelSelector, Settings};

use crate::args::{Args, ProviderKind, default_model_for};

/// Where the effective model selection came from. Surfaced in
/// `/config` and `caliban doctor` diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModelSource {
    Cli,
    Settings,
    BuiltinDefault,
}

/// Resolved provider/model pair for this run.
#[derive(Debug, Clone)]
pub(crate) struct EffectiveModel {
    pub provider: ProviderKind,
    pub name: String,
    pub fallback: Option<(ProviderKind, String)>,
    pub source: ModelSource,
}

impl EffectiveModel {
    /// Resolve CLI > Settings > Built-in. Order of precedence:
    /// 1. `--provider` and `--model` CLI flags (when explicitly set).
    /// 2. `Settings.model` (qualified form pins provider; bare-name
    ///    keeps the CLI/default provider).
    /// 3. `default_model_for(ProviderKind::Anthropic)` last-resort.
    pub fn resolve(args: &Args, settings: &Settings) -> Result<Self> {
        let cli_provider = args.provider;
        let cli_model = args.model.as_deref();

        // Both CLI-set: pure CLI win.
        if let (Some(p), Some(m)) = (cli_provider, cli_model) {
            return Ok(Self {
                provider: p,
                name: m.to_string(),
                fallback: Self::fallback_from_settings(settings)?,
                source: ModelSource::Cli,
            });
        }

        let (settings_provider, settings_name) = match &settings.model {
            Some(ModelSelector::Qualified { provider, name }) => {
                (Some(parse_provider(provider)?), Some(name.clone()))
            }
            Some(ModelSelector::Name(name)) => (None, Some(name.clone())),
            None => (None, None),
        };

        let provider = cli_provider
            .or(settings_provider)
            .unwrap_or(ProviderKind::Anthropic);

        let (name, source) = if let Some(m) = cli_model {
            (m.to_string(), ModelSource::Cli)
        } else if let Some(n) = settings_name.clone() {
            (n, ModelSource::Settings)
        } else {
            (default_model_for(provider).to_string(), ModelSource::BuiltinDefault)
        };

        if settings_name.is_some() && settings_provider.is_none() && cli_provider.is_none() {
            tracing::warn!(
                target: "caliban::config",
                "[model] bare-string in settings: pin the provider via \
                 `[model] provider = \"...\"` to avoid Anthropic default"
            );
        }

        Ok(Self {
            provider,
            name,
            fallback: Self::fallback_from_settings(settings)?,
            source,
        })
    }

    fn fallback_from_settings(settings: &Settings) -> Result<Option<(ProviderKind, String)>> {
        match &settings.fallback_model {
            Some(ModelSelector::Qualified { provider, name }) => {
                Ok(Some((parse_provider(provider)?, name.clone())))
            }
            Some(ModelSelector::Name(name)) => {
                Ok(Some((ProviderKind::Anthropic, name.clone())))
            }
            None => Ok(None),
        }
    }
}

fn parse_provider(s: &str) -> Result<ProviderKind> {
    match s {
        "anthropic" => Ok(ProviderKind::Anthropic),
        "openai" => Ok(ProviderKind::Openai),
        "ollama" => Ok(ProviderKind::Ollama),
        "google" => Ok(ProviderKind::Google),
        other => anyhow::bail!("unknown provider in settings: {other}"),
    }
}
```

- [ ] **Step 4: Add module** — `caliban/src/main.rs`:

```rust
mod effective_model;
```

- [ ] **Step 5: Run tests** — `cargo test -p caliban effective_model` → PASS.

- [ ] **Step 6: Commit:**
```bash
git add caliban/src/effective_model.rs caliban/src/main.rs
git commit -m "feat(cli): EffectiveModel — resolve CLI > Settings > builtin default"
```

---

## Task 5: Flip `args.provider` to `Option<ProviderKind>`

**Files:**
- Modify: `caliban/src/args.rs`

- [ ] **Step 1: Change the field**

```rust
// Before:
#[arg(long, value_enum, default_value_t = ProviderKind::Anthropic)]
pub(crate) provider: ProviderKind,

// After:
/// Which provider to use. If omitted, resolved from `Settings.model`
/// or falls back to Anthropic.
#[arg(long, value_enum)]
pub(crate) provider: Option<ProviderKind>,
```

- [ ] **Step 2: Build the workspace** — `cargo check --workspace` to enumerate broken call sites. There will be ~10 of them across `main.rs`, `startup.rs`, `tui.rs`, `tui/app.rs`, `tui/events.rs`, `tui/render.rs`, `tui/overlay.rs`, `tui/slash/session.rs`, `agents_cli.rs`, `diagnostics.rs`.

- [ ] **Step 3: Defer fixes to Task 6/7** — temporarily satisfy the compiler by adding a helper in `args.rs`:

```rust
/// Bridge for not-yet-migrated call sites. Will be removed in Task 7.
#[deprecated(note = "read from EffectiveModel instead")]
pub(crate) fn legacy_provider(args: &Args) -> ProviderKind {
    args.provider.unwrap_or(ProviderKind::Anthropic)
}
```

Replace each broken `args.provider` read with `legacy_provider(&args)` (or `legacy_provider(args)`) just enough to compile. Use `#[allow(deprecated)]` on the calling fn or module if needed.

- [ ] **Step 4: Run** — `cargo check --workspace` → PASS.

- [ ] **Step 5: Run tests** — `cargo test --workspace` → all green (this is purely a compatibility shim).

- [ ] **Step 6: Commit:**
```bash
git add caliban/src/
git commit -m "refactor(cli): provider: Option<ProviderKind> with legacy_provider shim"
```

---

## Task 6: Thread `EffectiveModel` through `main.rs` + `startup.rs`

**Files:**
- Modify: `caliban/src/main.rs` (build `EffectiveModel` after settings load)
- Modify: `caliban/src/startup.rs` (accept `&EffectiveModel`)
- Modify: `caliban/src/diagnostics.rs` (use `EffectiveModel` when available)

- [ ] **Step 1: Locate the model-resolution site** — `main.rs:240-243`. Replace:

```rust
let model = args
    .model
    .clone()
    .unwrap_or_else(|| default_model_for(args.provider).to_string());
```

with:

```rust
let effective = crate::effective_model::EffectiveModel::resolve(&args, &settings_snapshot)
    .context("resolving effective model from CLI args + settings")?;
let model = effective.name.clone();
```

(Adjust `args` mutation: also assign `args.provider = Some(effective.provider)` so `legacy_provider` calls return the resolved provider. Document this as a sprint-step bridge that goes away in Task 7.)

- [ ] **Step 2: Update `startup::build_provider`** — replace `args.provider` reads with `effective.provider`. Change signature:

```rust
pub(crate) fn build_provider(
    args: &Args,
    effective: &EffectiveModel,
    pool: &caliban_settings::ApiKeyHelperPool,  // empty when no helper set
) -> Result<Arc<dyn Provider + Send + Sync>> { ... }
```

For each provider arm, no behavior change yet to helper (Task 8 does that); just switch the discriminant from `args.provider` to `effective.provider`.

- [ ] **Step 3: Update `preflight_model_check`** — same.

- [ ] **Step 4: Update `diagnostics`** — sites that branched on `args.provider` now take `&EffectiveModel`.

- [ ] **Step 5: Build + run tests** — `cargo check && cargo test --workspace`.

- [ ] **Step 6: Commit:**
```bash
git add caliban/src/
git commit -m "feat(cli): thread EffectiveModel through main + startup"
```

---

## Task 7: Migrate TUI call sites + remove `legacy_provider`

**Files:**
- Modify: `caliban/src/tui.rs`, `tui/app.rs`, `tui/events.rs`, `tui/render.rs`, `tui/overlay.rs`, `tui/slash/session.rs`
- Modify: `caliban/src/agents_cli.rs`
- Modify: `caliban/src/args.rs` (delete `legacy_provider`)

- [ ] **Step 1: Pass `EffectiveModel` into the TUI** — extend `tui::App::new` (or the existing constructor) to accept `effective: EffectiveModel` and store it. Hot-modify the App's existing constructor sites in `main.rs`.

- [ ] **Step 2: Replace each `default_model_for(args.provider)` and `args.provider` read** — `rg "args\\.provider|default_model_for" caliban/src` enumerates them. For each:

```rust
// Before
let model = app.args.model.clone().unwrap_or_else(|| crate::default_model_for(app.args.provider).to_string());

// After
let model = app.effective.name.clone();
```

Same for provider branches: `app.effective.provider` instead of `app.args.provider`.

The `tui/overlay.rs::settings.model` display row stays — but now also include `EffectiveModel.source` so the operator can see which scope/precedence won.

- [ ] **Step 3: Remove `legacy_provider`** — delete the helper from `args.rs` and the `#[allow(deprecated)]` annotations.

- [ ] **Step 4: Build** — `cargo check --workspace` → PASS.

- [ ] **Step 5: Tests** — `cargo test --workspace` → PASS.

- [ ] **Step 6: Commit:**
```bash
git add caliban/src/
git commit -m "refactor(tui): read provider/model from EffectiveModel; drop legacy shim"
```

---

## Task 8: Wire `ApiKeyHelperPool` into provider construction

**Files:**
- Modify: `caliban/src/startup.rs` (`build_provider` — single-provider path)
- Modify: `caliban/src/router.rs` (`build_one` — router path)
- Modify: `caliban/src/main.rs` (construct pool from settings + pass through)

- [ ] **Step 1: Construct pool in main.rs**

After settings load:

```rust
let helper_pool = std::sync::Arc::new(
    caliban_settings::ApiKeyHelperPool::from_raw(
        settings_snapshot.api_key_helper.as_ref(),
    ),
);
```

- [ ] **Step 2: Update `startup::build_provider` arms** — each provider arm becomes:

```rust
ProviderKind::Openai => {
    use caliban_provider_openai::{OpenAIProvider, config::DirectConfig};
    let provider_id = "openai";
    let key = resolve_key(provider_id, pool, "OPENAI_API_KEY")?;
    let cfg = DirectConfig::new(key);
    let inner = OpenAIProvider::direct(cfg)?;
    if pool.spec_for_exists(provider_id) {
        let pool_cl = pool.clone();
        let rebuild = move |k: SecretString| {
            let cfg = DirectConfig::new(k);
            OpenAIProvider::direct(cfg).map_err(|e| caliban_provider::Error::adapter(e))
        };
        Arc::new(RefreshingProvider::new(inner, pool_cl, provider_id.into(), rebuild))
    } else {
        Arc::new(inner)
    }
}
```

Plus a private helper:

```rust
fn resolve_key(
    provider_id: &str,
    pool: &caliban_settings::ApiKeyHelperPool,
    env_var: &str,
) -> Result<SecretString> {
    if pool.spec_for_exists(provider_id) {
        let outcome = pool.key_for(provider_id)
            .map_err(|e| anyhow::anyhow!("api_key_helper for {provider_id}: {e}"))?;
        Ok(SecretString::from(outcome.key))
    } else {
        let key = std::env::var(env_var).map_err(|_| missing_key_err(env_var))?;
        Ok(SecretString::from(key))
    }
}
```

If `spec_for` is private, add a public `pub fn spec_for_exists(&self, p: &str) -> bool` to `caliban-settings`. Single-line method.

Mirror the pattern for `Anthropic`, `Google`. `Ollama` has no API key so skip helper wiring (still rebuild path unused).

- [ ] **Step 3: Update `router::build_one`** — same shape; signature gains `&Arc<ApiKeyHelperPool>`. Callers pass the pool from `RouterWiring::try_load`.

- [ ] **Step 4: Build + run unit tests** — `cargo test --workspace`.

- [ ] **Step 5: Commit:**
```bash
git add caliban/src/ crates/caliban-settings/
git commit -m "feat(cli): wire ApiKeyHelperPool into single-provider + router paths"
```

---

## Task 9: Integration test — helper feeds OpenAI key, settings selects provider

**Files:**
- Create: `caliban/tests/it_settings_model_and_helper.rs`

- [ ] **Step 1: Write the test**

```rust
//! Integration: TOML settings + helper script drive provider selection
//! and API auth end-to-end. Uses a mock HTTP server in place of OpenAI.

use std::process::Command;
use tempfile::TempDir;

#[test]
fn settings_model_picks_openai_and_helper_supplies_key() {
    let dir = TempDir::new().unwrap();
    let workspace = dir.path();

    // Write settings.toml selecting OpenAI + a helper that prints a stub key.
    let settings_dir = workspace.join(".caliban");
    std::fs::create_dir_all(&settings_dir).unwrap();
    std::fs::write(
        settings_dir.join("settings.toml"),
        r#"
[model]
provider = "openai"
name = "gpt-4o"

[[api_key_helper]]
provider = "openai"
command = "/bin/sh"
args = ["-c", "printf sk-from-helper"]
"#,
    ).unwrap();

    // Spin up a mock that asserts on Authorization header.
    let mock = httpmock::MockServer::start();
    let m = mock.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path_contains("/chat/completions")
            .header("Authorization", "Bearer sk-from-helper");
        then.status(200).body(r#"{"id":"x","choices":[{"message":{"role":"assistant","content":"hello"}}]}"#);
    });

    // Run caliban headless with the temp workspace, unsetting OPENAI_API_KEY.
    let bin = env!("CARGO_BIN_EXE_caliban");
    let status = Command::new(bin)
        .arg("--bare")
        .arg("--print").arg("hi")
        .arg("--workspace").arg(workspace)
        .env_remove("OPENAI_API_KEY")
        .env("OPENAI_BASE_URL", mock.base_url())
        .status()
        .expect("run caliban");

    assert!(status.success(), "caliban headless exited non-zero");
    m.assert_hits(1);
}
```

- [ ] **Step 2: Add `httpmock` and `tempfile` to caliban's `[dev-dependencies]`** if not already present.

- [ ] **Step 3: Run** — `cargo test -p caliban --test it_settings_model_and_helper` → PASS.

> If `httpmock` is heavier than wanted, fall back to a tiny in-process `tokio::TcpListener` that accepts one request and returns canned bytes. The intent is to assert on the `Authorization` header.

- [ ] **Step 4: Commit:**
```bash
git add caliban/Cargo.toml caliban/tests/
git commit -m "test(integ): settings.model + api_key_helper end-to-end via mock server"
```

---

## Task 10: Update parity matrix + open PR

- [ ] **Step 1: Edit `docs/parity-gap-matrix.md`** — find the row(s) for:
  - `apiKeyHelper` (mark shipped)
  - Any row mentioning `Settings.model` not consumed (mark shipped)

  Confirm by `rg -n 'apiKeyHelper|settings.model|Settings.model' docs/parity-gap-matrix.md`.

- [ ] **Step 2: Final workspace check**
```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

All three must pass.

- [ ] **Step 3: Push branch + open PR**

```bash
git push -u origin worktree-settings-model-and-key-helper
gh pr create --title "feat: wire Settings.model + api_key_helper into provider construction" --body "$(cat <<'EOF'
## Summary
- `EffectiveModel` resolves CLI > Settings > builtin default; finally consumed by the binary instead of decoratively parsed
- `RefreshingProvider<P>` decorator wraps any provider for transparent 401 refresh via `ApiKeyHelperPool`
- Helper integration in both single-provider (`startup::build_provider`) and router (`router::build_one`) paths

Spec: `docs/superpowers/specs/2026-05-31-settings-model-wiring-and-api-key-helper-design.md`

## Test plan
- [ ] `cargo test --workspace` green
- [ ] Manual: `.caliban/settings.toml` with `[model] provider = "openai"` actually picks OpenAI
- [ ] Manual: `[[api_key_helper]]` script provides key when `OPENAI_API_KEY` is unset
- [ ] Manual: 401 from upstream triggers helper re-run + retry
EOF
)"
```

---

## Self-review

- **Spec coverage:** EffectiveModel (§Architecture), RefreshingProvider (§Architecture), is_auth_error (§Error classification), helper plumbing in build_one+build_provider (§Helper plumbing). ✓
- **Placeholders:** Task 3 has one note about MockProviderBuilder API drift — that's a real-world adaptation hint, not a "TBD". Task 9 has a fallback if httpmock is too heavy — also concrete. No TODOs/TBDs in steps themselves. ✓
- **Type consistency:** `EffectiveModel { provider, name, fallback, source }`, `ModelSource::{Cli, Settings, BuiltinDefault}` used consistently across Tasks 4, 6, 7. `RefreshingProvider::new(inner, pool, provider_id, rebuild)` consistent across Tasks 3, 8. ✓
