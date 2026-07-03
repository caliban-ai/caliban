# Prefill-aware stream watchdog + stream-path total-timeout exemption — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the stream idle watchdog tolerate slow local-model prefill (a generous pre-first-token budget, distinct from the tight mid-content window) and stop reqwest's total timeout from killing healthy slow streams, closing #263 and #254.

**Architecture:** Two seams. Seam A makes `WatchedStream` phase-aware: before the first chunk the idle limit is a generous `prefill` budget; after it, the existing tight `idle` window. Seam B builds a second reqwest client for the streaming path with no total `.timeout()` (connect-timeout only), leaving the non-streaming `send` path bounded. Budgets are configured via global TOML settings + an ollama-scoped env override; the stream total-timeout is a provider config with a default of "off".

**Tech Stack:** Rust (tokio, futures `Stream`, reqwest), the caliban workspace (`caliban-provider`, `caliban-agent-core`, `caliban-settings`, `caliban-common`, `caliban-provider-ollama`, `caliban-provider-openai`, `caliban` binary).

## Global Constraints

- Local verification gate before any push (CI mirror): `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo build --workspace --all-targets`, `cargo test --workspace`. All four must pass.
- Author identity for commits in `~/dev/caliban-ai/**`: `john.ford2002@gmail.com`; end commit messages with `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- No new error variants: reuse `caliban_provider::Error::StreamIdle(Duration)`.
- `stream_idle_timeout_ms == 0` still disables the watchdog entirely (unchanged gate).
- `stream_prefill_timeout_ms == 0` means "no separate prefill grace" → prefill limit resolves to the idle value (preserves today's behavior).
- Default prefill budget: `300_000` ms. Default idle window: `90_000` ms (unchanged).

---

### Task 1: `WatchedStream` two-phase budget

**Files:**
- Modify: `crates/caliban-provider/src/stream.rs:229-307` (struct, constructor, `poll_next`) and its tests `:309-363`
- Modify: `crates/caliban-agent-core/src/stream/mod.rs:1152-1161` (the sole production caller — update to the new signature, behavior-preserving `prefill = idle` for now; Task 2 makes it configurable)

**Interfaces:**
- Produces: `WatchedStream::new(inner: S, idle: Duration, prefill: Duration) -> Self`. Before the first chunk arrives the active idle limit is `prefill`; after the first chunk it is `idle`. Aborts with `Error::StreamIdle(elapsed)`.

- [ ] **Step 1: Write the failing tests** — append two tests to the `watched_tests` module in `crates/caliban-provider/src/stream.rs` and update the three existing tests' constructor calls to the 3-arg form.

Update existing calls (add a third arg):
```rust
// passes_through_normal_data
let mut w = WatchedStream::new(inner, Duration::from_secs(1), Duration::from_secs(1));
// aborts_after_idle_timeout
let mut w = WatchedStream::new(inner, Duration::from_millis(20), Duration::from_millis(20));
// resets_idle_clock_on_each_chunk
let mut w = WatchedStream::new(inner, Duration::from_millis(100), Duration::from_millis(100));
```

Append new tests:
```rust
/// Before the first chunk, the *prefill* budget governs — a longer pre-first-
/// token silence must be tolerated up to `prefill`, even when `idle` is tight.
#[tokio::test]
async fn prefill_budget_tolerates_slow_first_chunk() {
    // Never yields a chunk. idle=20ms would abort fast; prefill=300ms must
    // hold it open past the idle window before aborting.
    let inner = stream::pending::<Result<StreamEvent>>();
    let mut w = WatchedStream::new(inner, Duration::from_millis(20), Duration::from_millis(300));
    let start = Instant::now();
    let r = w.next().await.expect("Some(_)");
    let waited = start.elapsed();
    assert!(matches!(r, Err(Error::StreamIdle(_))), "still aborts eventually");
    assert!(
        waited >= Duration::from_millis(200),
        "prefill budget should hold open well past the 20ms idle window, waited {waited:?}",
    );
}

/// After the first chunk arrives, the tight `idle` window governs mid-content
/// stalls — the generous prefill budget no longer applies.
#[tokio::test]
async fn idle_window_governs_after_first_chunk() {
    // One real chunk, then silence forever. prefill is huge (would never fire);
    // idle is tiny, so the post-first-chunk stall must abort at ~idle.
    let inner = Box::pin(stream::unfold(0u32, |n| async move {
        match n {
            0 => Some((Ok(StreamEvent::Ping), 1)),
            _ => {
                std::future::pending::<()>().await;
                None
            }
        }
    }));
    let mut w = WatchedStream::new(inner, Duration::from_millis(20), Duration::from_secs(60));
    // First poll yields the chunk.
    let first = w.next().await.expect("first item");
    assert!(matches!(first, Ok(StreamEvent::Ping)));
    // Second poll must abort at the tight idle window, not the 60s prefill.
    let start = Instant::now();
    let second = w.next().await.expect("second item");
    assert!(matches!(second, Err(Error::StreamIdle(_))));
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "post-first-chunk stall must abort at idle (20ms), not prefill (60s)",
    );
}
```

Note: add `use std::time::Instant;` to the `watched_tests` module imports (alongside the existing `Duration` import).

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p caliban-provider watched_tests`
Expected: FAIL — `prefill_budget_tolerates_slow_first_chunk` and `idle_window_governs_after_first_chunk` fail to compile (3-arg `new` doesn't exist yet), and the existing tests fail to compile after the arg change.

- [ ] **Step 3: Implement the two-phase budget** in `crates/caliban-provider/src/stream.rs`.

Struct — add a `prefill` field and a `first_chunk_seen` flag (replace lines 229-239):
```rust
pub struct WatchedStream<S> {
    inner: S,
    /// Idle window that governs *after* the first chunk has arrived
    /// (mid-content stalls).
    idle: Duration,
    /// Idle window that governs *before* the first chunk arrives (slow
    /// prefill on a large-context local-model turn). Typically >= `idle`.
    prefill: Duration,
    /// Set once the inner stream yields its first chunk. Switches the active
    /// idle limit from `prefill` to `idle`. See #263.
    first_chunk_seen: bool,
    last_chunk_at: Instant,
    warned: bool,
    /// A single, resettable wakeup timer armed to the idle deadline. Reused
    /// across polls (reset on each Pending, re-anchored on each chunk) so the
    /// watchdog never spawns a fresh task per poll — see #117. Created lazily
    /// on the first Pending so construction needs no runtime context.
    wakeup: Option<Pin<Box<tokio::time::Sleep>>>,
}
```

Constructor (replace lines 241-253):
```rust
impl<S> WatchedStream<S> {
    /// Build a new `WatchedStream`. `idle` is the maximum silence tolerated
    /// *after* the first chunk (mid-content stall); `prefill` is the maximum
    /// silence tolerated *before* the first chunk (slow prefill, #263). Both
    /// surface [`Error::StreamIdle`] on expiry.
    pub fn new(inner: S, idle: Duration, prefill: Duration) -> Self {
        Self {
            inner,
            idle,
            prefill,
            first_chunk_seen: false,
            last_chunk_at: Instant::now(),
            warned: false,
            wakeup: None,
        }
    }

    /// The idle limit currently in force: `prefill` until the first chunk is
    /// seen, `idle` thereafter.
    fn active_idle(&self) -> Duration {
        if self.first_chunk_seen {
            self.idle
        } else {
            self.prefill
        }
    }
}
```

`poll_next` (replace lines 261-306) — mark the first chunk, and compare against `active_idle()`; add a `phase` field to the tracing events:
```rust
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(item)) => {
                self.last_chunk_at = Instant::now();
                self.first_chunk_seen = true;
                self.warned = false;
                Poll::Ready(Some(item))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => {
                let limit = self.active_idle();
                let phase = if self.first_chunk_seen { "content" } else { "prefill" };
                let elapsed = self.last_chunk_at.elapsed();
                if elapsed >= limit {
                    tracing::error!(
                        target: "caliban::stream",
                        phase = phase,
                        elapsed_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
                        "recovery.stream_idle.abort"
                    );
                    return Poll::Ready(Some(Err(Error::StreamIdle(elapsed))));
                }
                if !self.warned && elapsed >= limit / 2 {
                    self.warned = true;
                    tracing::warn!(
                        target: "caliban::stream",
                        phase = phase,
                        elapsed_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
                        "recovery.stream_idle.warning"
                    );
                }
                // Arm (or re-arm) a single resettable timer at the active idle
                // deadline so we fire the abort even if `inner` stays Pending.
                let remaining = limit.checked_sub(elapsed).unwrap_or(Duration::ZERO);
                let deadline = tokio::time::Instant::now() + remaining + Duration::from_millis(1);
                let wakeup = self
                    .wakeup
                    .get_or_insert_with(|| Box::pin(tokio::time::sleep_until(deadline)));
                wakeup.as_mut().reset(deadline);
                let _ = wakeup.as_mut().poll(cx);
                Poll::Pending
            }
        }
    }
```

Update the doc comment on the struct (lines 218-222) to mention both windows:
```rust
/// Wraps a `Stream` and aborts with [`Error::StreamIdle`] when no chunk
/// arrives within the active idle window. The window is `prefill` before the
/// first chunk (slow local-model prefill, #263) and `idle` after it
/// (mid-content stall).
///
/// Emits a `tracing::warn` at half-time (with a `phase` field) and
/// `Err(Error::StreamIdle)` on full timeout.
```

- [ ] **Step 4: Update the production caller** in `crates/caliban-agent-core/src/stream/mod.rs:1155-1158` so the workspace compiles (behavior-preserving: pass `prefill = idle` for now):
```rust
                > = if self.config.stream_idle_timeout_ms > 0 {
                    let idle = Duration::from_millis(self.config.stream_idle_timeout_ms.into());
                    Box::pin(caliban_provider::stream::WatchedStream::new(
                        provider_stream,
                        idle,
                        idle,
                    ))
                } else {
                    provider_stream
                };
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p caliban-provider watched_tests && cargo build -p caliban-agent-core`
Expected: PASS (5 watched_tests pass; agent-core builds).

- [ ] **Step 6: Commit**

```bash
git add crates/caliban-provider/src/stream.rs crates/caliban-agent-core/src/stream/mod.rs
git commit -m "feat(providers): phase-aware WatchedStream (prefill vs mid-content idle) (#263)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: `AgentConfig.stream_prefill_timeout_ms` + wire the real prefill value

**Files:**
- Modify: `crates/caliban-agent-core/src/agent.rs:63-64` (field), `:125` (default), `:155-167` (default test)
- Modify: `crates/caliban-agent-core/src/stream/mod.rs:1154-1161` (compute prefill from config)

**Interfaces:**
- Consumes: `WatchedStream::new(inner, idle, prefill)` from Task 1.
- Produces: `AgentConfig.stream_prefill_timeout_ms: u32` (default `300_000`). Resolution rule: `prefill = if stream_prefill_timeout_ms > 0 { that } else { stream_idle_timeout_ms }`.

- [ ] **Step 1: Write the failing test** — extend `default_recovery_knobs` in `crates/caliban-agent-core/src/agent.rs` (after line 160):
```rust
        assert_eq!(cfg.stream_prefill_timeout_ms, 300_000);
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p caliban-agent-core default_recovery_knobs`
Expected: FAIL — no field `stream_prefill_timeout_ms`.

- [ ] **Step 3: Add the field + default.**

Field (insert after line 64, immediately below `stream_idle_timeout_ms`):
```rust
    /// Pre-first-token idle budget (ms) for the stream watchdog. Governs the
    /// silence tolerated *before* the first output chunk (slow local-model
    /// prefill on a large-context turn, #263). `0` falls back to
    /// `stream_idle_timeout_ms`. Frontier models prefill in ms and never
    /// approach this, so a single generous global default is safe.
    pub stream_prefill_timeout_ms: u32,
```

Default (insert after line 125, below `stream_idle_timeout_ms: 90_000,`):
```rust
            stream_prefill_timeout_ms: 300_000,
```

- [ ] **Step 4: Wire the resolved prefill into the watchdog** — replace the Task-1 stopgap in `crates/caliban-agent-core/src/stream/mod.rs`:
```rust
                > = if self.config.stream_idle_timeout_ms > 0 {
                    let idle = Duration::from_millis(self.config.stream_idle_timeout_ms.into());
                    // `0` prefill → no separate grace, fall back to `idle`.
                    let prefill = if self.config.stream_prefill_timeout_ms > 0 {
                        Duration::from_millis(self.config.stream_prefill_timeout_ms.into())
                    } else {
                        idle
                    };
                    Box::pin(caliban_provider::stream::WatchedStream::new(
                        provider_stream,
                        idle,
                        prefill,
                    ))
                } else {
                    provider_stream
                };
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p caliban-agent-core default_recovery_knobs && cargo build -p caliban-agent-core`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/caliban-agent-core/src/agent.rs crates/caliban-agent-core/src/stream/mod.rs
git commit -m "feat(agent-core): stream_prefill_timeout_ms config (default 300s) (#263)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: Settings TOML knobs + `apply_stream_watchdog` + compose wiring

**Files:**
- Modify: `crates/caliban-settings/src/settings.rs:284-299` (fields), add helper near `apply_context_management` (`:482-495`), add tests in the `tests` module (`:559+`)
- Modify: `caliban/src/startup/compose.rs:1182` (call the new helper)

**Interfaces:**
- Consumes: `AgentConfig.stream_idle_timeout_ms`, `AgentConfig.stream_prefill_timeout_ms` from Task 2.
- Produces: `Settings.stream_idle_timeout_ms: Option<u32>`, `Settings.stream_prefill_timeout_ms: Option<u32>`, and `Settings::apply_stream_watchdog(&self, cfg: &mut AgentConfig)`.

- [ ] **Step 1: Write the failing tests** — add to the `tests` module in `crates/caliban-settings/src/settings.rs`:
```rust
    #[test]
    fn apply_stream_watchdog_overrides_each_field() {
        let raw = r#"{
            "stream_idle_timeout_ms": 45000,
            "stream_prefill_timeout_ms": 600000
        }"#;
        let s: Settings = serde_json::from_str(raw).unwrap();
        let mut cfg = caliban_agent_core::AgentConfig::default();
        s.apply_stream_watchdog(&mut cfg);
        assert_eq!(cfg.stream_idle_timeout_ms, 45_000);
        assert_eq!(cfg.stream_prefill_timeout_ms, 600_000);
    }

    #[test]
    fn apply_stream_watchdog_leaves_defaults_when_unset() {
        let s: Settings = serde_json::from_str(r"{}").unwrap();
        let mut cfg = caliban_agent_core::AgentConfig::default();
        let snap_idle = cfg.stream_idle_timeout_ms;
        let snap_prefill = cfg.stream_prefill_timeout_ms;
        s.apply_stream_watchdog(&mut cfg);
        assert_eq!(cfg.stream_idle_timeout_ms, snap_idle);
        assert_eq!(cfg.stream_prefill_timeout_ms, snap_prefill);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p caliban-settings apply_stream_watchdog`
Expected: FAIL — no fields / no method `apply_stream_watchdog`.

- [ ] **Step 3: Add the fields.** Insert into the `Settings` struct after line 299 (`pub max_tokens_recovery: Option<bool>,`), in a new commented group:
```rust

    // ----- stream watchdog (#263 / #254) ------------------------------------
    /// Idle window (ms) tolerated *after* the first output chunk (mid-content
    /// stall). `None` keeps the 90s default; `0` disables the watchdog.
    pub stream_idle_timeout_ms: Option<u32>,
    /// Idle window (ms) tolerated *before* the first output chunk (slow
    /// local-model prefill, #263). `None` keeps the 300s default; `0` falls
    /// back to the idle window.
    pub stream_prefill_timeout_ms: Option<u32>,
```

- [ ] **Step 4: Add the helper.** Insert after `apply_context_management` (after line 495):
```rust
    /// Apply stream-watchdog knobs onto a fresh
    /// [`caliban_agent_core::AgentConfig`]. Only fields explicitly set in
    /// settings override the defaults. See #263 / #254.
    pub fn apply_stream_watchdog(&self, cfg: &mut caliban_agent_core::AgentConfig) {
        if let Some(v) = self.stream_idle_timeout_ms {
            cfg.stream_idle_timeout_ms = v;
        }
        if let Some(v) = self.stream_prefill_timeout_ms {
            cfg.stream_prefill_timeout_ms = v;
        }
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p caliban-settings apply_stream_watchdog`
Expected: PASS.

- [ ] **Step 6: Wire the call in `build_agent`.** In `caliban/src/startup/compose.rs`, immediately after line 1182 (`settings_snapshot.apply_context_management(&mut cfg);`):
```rust
    // Stream-watchdog knobs from Settings — stream_idle_timeout_ms,
    // stream_prefill_timeout_ms (#263 / #254). Same wire-or-it-never-arrives
    // caveat as apply_context_management above.
    settings_snapshot.apply_stream_watchdog(&mut cfg);
```

- [ ] **Step 7: Verify build + commit**

Run: `cargo build -p caliban && cargo test -p caliban-settings`
Expected: PASS.
```bash
git add crates/caliban-settings/src/settings.rs caliban/src/startup/compose.rs
git commit -m "feat(settings): TOML knobs for stream idle/prefill timeouts (#263)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: `build_stream_client` (no total timeout, connect-timeout only)

**Files:**
- Modify: `crates/caliban-common/src/http.rs` (add function after `build_client`, `:88`; add test in the `tests` module)

**Interfaces:**
- Produces: `caliban_common::http::build_stream_client(connect_timeout: Duration) -> reqwest::Result<reqwest::Client>` — a client with `.connect_timeout(connect_timeout)` and **no** total `.timeout()`.

- [ ] **Step 1: Write the failing test** — add to the `tests` module in `crates/caliban-common/src/http.rs`:
```rust
    #[test]
    fn build_stream_client_constructs_without_total_timeout() {
        // The stream client sets a connect timeout but no total timeout, so a
        // healthy slow stream is never wall-clock-killed. We can only smoke the
        // constructor here; the "no total timeout" property is covered by the
        // provider transport tests + the eval re-run.
        let client = build_stream_client(Duration::from_secs(30)).expect("stream client builds");
        let _ = client;
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p caliban-common build_stream_client`
Expected: FAIL — `build_stream_client` not found.

- [ ] **Step 3: Implement.** Add after line 88 in `crates/caliban-common/src/http.rs`:
```rust
/// Build a [`reqwest::Client`] for the **streaming** path: a bounded
/// `connect_timeout` but **no** total `.timeout()`.
///
/// reqwest's `.timeout()` is a *total* deadline (connect → body finished),
/// which kills a healthy-but-slow streaming response (large local-model turn)
/// even while tokens are flowing (#254). The streaming path instead relies on
/// the application-level `WatchedStream` idle watchdog for stall detection, so
/// the transport only needs to bound connection establishment.
///
/// `default_client_builder()` applies `DEFAULT_TIMEOUT` as a total timeout, so
/// we start from `reqwest::Client::builder()` directly to keep the shared
/// user-agent / redirect / http2 config while omitting the total timeout.
///
/// # Errors
///
/// Returns the underlying [`reqwest::Error`] if the TLS / DNS backend fails to
/// initialize.
pub fn build_stream_client(connect_timeout: Duration) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .redirect(reqwest::redirect::Policy::limited(10))
        .http2_adaptive_window(true)
        .connect_timeout(connect_timeout)
        .build()
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p caliban-common build_stream_client`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-common/src/http.rs
git commit -m "feat(common): build_stream_client with connect-only timeout (#254)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 5: Provider transports — dedicated stream client + `stream_total_timeout`

**Files:**
- Modify: `crates/caliban-provider-ollama/src/config.rs` (add field, default, env parse, tests)
- Modify: `crates/caliban-provider-ollama/src/transport/direct.rs` (second client, use in `stream()`)
- Modify: `crates/caliban-provider-openai/src/config.rs` (add field + default)
- Modify: `crates/caliban-provider-openai/src/transport/direct.rs` (second client, use in `stream()`)

**Interfaces:**
- Consumes: `caliban_common::http::build_stream_client` from Task 4, `caliban_common::http::DEFAULT_TIMEOUT` (30s connect).
- Produces: `DirectConfig.stream_total_timeout: Option<Duration>` on both ollama and openai configs (default `None` = no total cap on the stream path). ollama also parses `OLLAMA_STREAM_TOTAL_TIMEOUT_MS`.

- [ ] **Step 1: Write the failing test** (ollama) — add to the `tests` module in `crates/caliban-provider-ollama/src/config.rs`:
```rust
    #[test]
    fn stream_total_timeout_defaults_off() {
        let cfg = DirectConfig::new();
        assert!(cfg.stream_total_timeout.is_none());
    }

    #[test]
    fn stream_total_timeout_parsed_from_env() {
        let cfg = DirectConfig::from_env_parts(None, Some("120000"))
            .expect("valid ms parses");
        assert_eq!(cfg.stream_total_timeout, Some(Duration::from_millis(120_000)));
    }

    #[test]
    fn stream_total_timeout_absent_env_is_none() {
        let cfg = DirectConfig::from_env_parts(None, None).expect("unset ok");
        assert!(cfg.stream_total_timeout.is_none());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p caliban-provider-ollama stream_total_timeout`
Expected: FAIL — no field / no `from_env_parts`.

- [ ] **Step 3: Implement ollama config.** In `crates/caliban-provider-ollama/src/config.rs`:

Add the field to the struct (after `pub timeout: Duration,`, line 19):
```rust
    /// Optional **total** timeout for the streaming path. `None` (default)
    /// means no total cap — the stream relies on the connect timeout + the
    /// agent-core `WatchedStream` idle watchdog (#254). `Some(d)` re-imposes a
    /// hard wall-clock cap for operators who want one.
    pub stream_total_timeout: Option<Duration>,
```

Set it in `new()` (inside the `Self { .. }`, after `timeout:`):
```rust
            stream_total_timeout: None,
```

Rework `from_env`/`from_env_value` to also read the new env var. Replace `from_env` (line 51-53) and generalize the helper:
```rust
    /// Load configuration from environment variables.
    ///
    /// Optional: `OLLAMA_BASE_URL` (defaults to `http://localhost:11434`),
    /// `OLLAMA_STREAM_TOTAL_TIMEOUT_MS` (unset = no total cap on the stream
    /// path).
    ///
    /// # Errors
    ///
    /// Returns `Err(OllamaError::Transport)` if `OLLAMA_BASE_URL` is set to a
    /// value that is not a valid URL, or `OLLAMA_STREAM_TOTAL_TIMEOUT_MS` is
    /// set to a non-integer.
    pub fn from_env() -> Result<Self, OllamaError> {
        Self::from_env_parts(
            std::env::var("OLLAMA_BASE_URL").ok().as_deref(),
            std::env::var("OLLAMA_STREAM_TOTAL_TIMEOUT_MS").ok().as_deref(),
        )
    }

    /// Back-compat shim: `from_env_value(url)` == `from_env_parts(url, None)`.
    /// Retained for existing callers/tests.
    ///
    /// # Errors
    ///
    /// Returns `Err(OllamaError::Transport)` if `url` is `Some` but not a
    /// parseable URL.
    pub fn from_env_value(url: Option<&str>) -> Result<Self, OllamaError> {
        Self::from_env_parts(url, None)
    }

    /// Build a config from explicit env values (or `None` for the unset
    /// case). Exposed for tests so URL-parsing and timeout-parsing branches
    /// can be exercised independently.
    ///
    /// # Errors
    ///
    /// Returns `Err(OllamaError::Transport)` if `url` is `Some` but not a
    /// parseable URL, or `total_timeout_ms` is `Some` but not an integer.
    pub fn from_env_parts(
        url: Option<&str>,
        total_timeout_ms: Option<&str>,
    ) -> Result<Self, OllamaError> {
        let mut cfg = Self::new();
        if let Some(url) = url {
            cfg.base_url = Url::parse(url).map_err(|e| OllamaError::Transport(Box::new(e)))?;
        }
        if let Some(ms) = total_timeout_ms {
            let ms: u64 = ms
                .parse()
                .map_err(|e| OllamaError::Transport(Box::new(e)))?;
            cfg.stream_total_timeout = Some(Duration::from_millis(ms));
        }
        Ok(cfg)
    }
```

- [ ] **Step 4: Run ollama config tests**

Run: `cargo test -p caliban-provider-ollama config::`
Expected: PASS (existing `from_env_value` tests still pass via the shim; new tests pass).

- [ ] **Step 5: Use a dedicated stream client in the ollama transport.** In `crates/caliban-provider-ollama/src/transport/direct.rs`:

Add a `stream_client` field (struct, line 13-17):
```rust
#[derive(Debug)]
pub struct DirectTransport {
    client: reqwest::Client,
    stream_client: reqwest::Client,
    config: DirectConfig,
}
```

Build it in `new()` (replace lines 25-29):
```rust
    pub fn new(config: DirectConfig) -> Result<Self, OllamaError> {
        let client =
            caliban_common::http::build_client(config.timeout).map_err(OllamaError::Http)?;
        // Streaming path: no total timeout by default (#254); rely on the
        // connect timeout + the agent-core WatchedStream watchdog. If the
        // operator set a stream total timeout, honor it.
        let stream_client = match config.stream_total_timeout {
            Some(total) => caliban_common::http::build_client(total),
            None => caliban_common::http::build_stream_client(caliban_common::http::DEFAULT_TIMEOUT),
        }
        .map_err(OllamaError::Http)?;
        Ok(Self {
            client,
            stream_client,
            config,
        })
    }
```

Use `stream_client` in `stream()` (line 63-64, change `self.client` → `self.stream_client`):
```rust
        let resp = self
            .stream_client
            .post(self.endpoint())
```

- [ ] **Step 6: Mirror on openai.** In `crates/caliban-provider-openai/src/config.rs`, add the same field to `DirectConfig` (after `pub timeout: Duration,`, line 25):
```rust
    /// Optional **total** timeout for the streaming path. `None` (default) =
    /// no total cap; the stream relies on connect timeout + the WatchedStream
    /// watchdog (#254). `Some(d)` re-imposes a hard wall-clock cap.
    pub stream_total_timeout: Option<Duration>,
```
Set `stream_total_timeout: None,` in `DirectConfig::new` (and in `from_parts` if that constructor builds the struct literally — mirror the existing `timeout` initialization there).

In `crates/caliban-provider-openai/src/transport/direct.rs`, add the `stream_client` field (struct line 15-18) and build it in `new()` (replace lines 26-30) exactly as ollama:
```rust
#[derive(Debug)]
pub struct DirectTransport {
    client: reqwest::Client,
    stream_client: reqwest::Client,
    config: DirectConfig,
}
```
```rust
    pub fn new(config: DirectConfig) -> Result<Self, OpenAIError> {
        let client =
            caliban_common::http::build_client(config.timeout).map_err(OpenAIError::Http)?;
        let stream_client = match config.stream_total_timeout {
            Some(total) => caliban_common::http::build_client(total),
            None => caliban_common::http::build_stream_client(caliban_common::http::DEFAULT_TIMEOUT),
        }
        .map_err(OpenAIError::Http)?;
        Ok(Self {
            client,
            stream_client,
            config,
        })
    }
```
Use `self.stream_client` in `stream()` (line 88-90, change `self.client` → `self.stream_client`).

- [ ] **Step 7: Full build + affected tests + commit**

Run: `cargo build -p caliban-provider-ollama -p caliban-provider-openai && cargo test -p caliban-provider-ollama -p caliban-provider-openai`
Expected: PASS. (Watch for other `DirectConfig { .. }` literal construction sites in tests — if a test builds the struct without `..Default::default()`, add `stream_total_timeout: None,`. Search: `rg 'DirectConfig \{' crates/caliban-provider-ollama crates/caliban-provider-openai`.)
```bash
git add crates/caliban-provider-ollama crates/caliban-provider-openai
git commit -m "feat(providers): dedicated stream client, no total timeout by default (#254)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 6: ollama-scoped env override for the watchdog budgets

**Files:**
- Modify: `caliban/src/startup/compose.rs` (after the `apply_stream_watchdog` call from Task 3)

**Interfaces:**
- Consumes: `AgentConfig.stream_idle_timeout_ms`, `AgentConfig.stream_prefill_timeout_ms`; `caliban::args::{resolved_provider, provider_name}` (returns `&'static str`, e.g. `"ollama"`).
- Produces: precedence `env > settings TOML > default` for the watchdog budgets, **only when the active provider is ollama**. Env vars: `OLLAMA_STREAM_IDLE_TIMEOUT_MS`, `OLLAMA_STREAM_PREFILL_TIMEOUT_MS`. A non-integer value is ignored with a warning (never fatal).

- [ ] **Step 1: Add a small pure helper + its test** in `caliban/src/startup/compose.rs`. Place the helper near `build_agent`:
```rust
/// Overlay an env override onto a watchdog budget field. Reads `var`; on a
/// valid `u32` it overwrites `*slot`; on a malformed value it warns and leaves
/// `*slot` unchanged. Returns whether an override was applied (for tests).
fn apply_env_ms_override(var: &str, raw: Option<&str>, slot: &mut u32) -> bool {
    match raw {
        None => false,
        Some(s) => match s.parse::<u32>() {
            Ok(v) => {
                *slot = v;
                true
            }
            Err(_) => {
                tracing::warn!(
                    target: caliban_common::tracing_targets::TARGET_SETTINGS,
                    var = var,
                    value = s,
                    "ignoring malformed stream-timeout env override (expected integer ms)",
                );
                false
            }
        },
    }
}
```

Test (add to `compose.rs`'s `#[cfg(test)] mod tests`, or create one if absent):
```rust
    #[test]
    fn env_ms_override_applies_valid_and_ignores_garbage() {
        let mut slot = 90_000_u32;
        assert!(super::apply_env_ms_override("X", Some("120000"), &mut slot));
        assert_eq!(slot, 120_000);
        assert!(!super::apply_env_ms_override("X", Some("abc"), &mut slot));
        assert_eq!(slot, 120_000, "garbage leaves the prior value");
        assert!(!super::apply_env_ms_override("X", None, &mut slot));
        assert_eq!(slot, 120_000, "unset leaves the prior value");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p caliban env_ms_override`
Expected: FAIL — `apply_env_ms_override` not found.

- [ ] **Step 3: Wire the ollama-scoped override** in `build_agent`, immediately after the Task-3 `apply_stream_watchdog` call:
```rust
    // #263: ollama-only env override for the watchdog budgets so eval /
    // emulated runs widen the window without a rebuild. Scoped to ollama
    // because the watchdog is global (provider-agnostic) but this knob exists
    // for the slow-local-model case; applying it to a frontier provider would
    // be surprising. Precedence: env > settings > default.
    if crate::args::provider_name(crate::args::resolved_provider(args)) == "ollama" {
        apply_env_ms_override(
            "OLLAMA_STREAM_IDLE_TIMEOUT_MS",
            std::env::var("OLLAMA_STREAM_IDLE_TIMEOUT_MS").ok().as_deref(),
            &mut cfg.stream_idle_timeout_ms,
        );
        apply_env_ms_override(
            "OLLAMA_STREAM_PREFILL_TIMEOUT_MS",
            std::env::var("OLLAMA_STREAM_PREFILL_TIMEOUT_MS").ok().as_deref(),
            &mut cfg.stream_prefill_timeout_ms,
        );
    }
```
(Confirm the `use` path for `resolved_provider`/`provider_name` — they are `pub(crate)` in `caliban/src/args.rs`; reference them as `crate::args::…`.)

- [ ] **Step 4: Run test + build**

Run: `cargo test -p caliban env_ms_override && cargo build -p caliban`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add caliban/src/startup/compose.rs
git commit -m "feat(cli): OLLAMA_STREAM_*_TIMEOUT_MS env override for watchdog (#263)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 7: End-to-end integration test + docs

**Files:**
- Modify: `crates/caliban-provider/src/mock.rs` (add a delayed-first-chunk mock entry)
- Modify: `crates/caliban-agent-core/tests/recovery_stream_idle.rs` (prefill-grace + mid-content-abort cases)
- Modify: the config reference in the user guide if one enumerates settings knobs (search below)

**Interfaces:**
- Consumes: everything above.
- Produces: `MockProviderBuilder::with_delayed_first_chunk(delay: Duration, text: &str)` — a stream that stays silent for `delay` (exercising the prefill budget), then emits a normal `EndTurn` response.

- [ ] **Step 1: Write the failing integration tests** in `crates/caliban-agent-core/tests/recovery_stream_idle.rs`:
```rust
/// A stream that delays its first chunk past the (tight) idle window but
/// within the (generous) prefill budget must NOT abort — it completes. Guards
/// the #263 prefill grace end-to-end.
#[tokio::test]
async fn slow_prefill_within_budget_completes() {
    let provider = MockProvider::builder()
        .with_delayed_first_chunk(Duration::from_millis(120), "done")
        .build();
    let cfg = AgentConfig {
        model: "mock".into(),
        stream_idle_timeout_ms: 40,       // tight mid-content window
        stream_prefill_timeout_ms: 5_000, // generous prefill budget
        ..Default::default()
    };
    let agent = Arc::new(
        Agent::builder()
            .provider(Arc::new(provider))
            .config(cfg)
            .build()
            .expect("agent"),
    );
    let mut stream =
        agent.stream_until_done(vec![Message::user_text("x")], CancellationToken::new());
    let mut last_stop = None;
    while let Some(Ok(ev)) = stream.next().await {
        if let TurnEvent::RunEnd { stopped_for, .. } = ev {
            last_stop = Some(stopped_for);
        }
    }
    assert!(
        !matches!(last_stop, Some(StopCondition::StreamIdle(_))),
        "slow prefill within budget must not trip the idle watchdog, got {last_stop:?}",
    );
}
```
(Keep the existing `stream_idle_aborts_run` test — a silent stream still aborts; with the default 300s prefill it would be slow, so leave that test's `stream_idle_timeout_ms: 200` and also set `stream_prefill_timeout_ms: 200` so the silent-forever stream still aborts quickly under the prefill budget. Update it:)
```rust
    let cfg = AgentConfig {
        model: "mock".into(),
        stream_idle_timeout_ms: 200,
        stream_prefill_timeout_ms: 200,
        ..Default::default()
    };
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p caliban-agent-core --test recovery_stream_idle`
Expected: FAIL — `with_delayed_first_chunk` not found.

- [ ] **Step 3: Add the mock capability** in `crates/caliban-provider/src/mock.rs`.

Add a `MockEntry` variant (find the `enum MockEntry` definition and add):
```rust
    /// A stream that stays silent for `delay`, then emits the given events.
    DelayedFirstChunk { delay: Duration, events: Vec<Result<StreamEvent>> },
```

Handle it in `stream()` (in the `match s.stream_queue.remove(0)` arm, alongside `Events`/`Silent`):
```rust
            MockEntry::DelayedFirstChunk { delay, events } => {
                let s = async_stream::stream! {
                    tokio::time::sleep(delay).await;
                    for ev in events {
                        yield ev;
                    }
                };
                Ok(Box::pin(s))
            }
```
(If `async-stream` is not already a dependency of `caliban-provider`, build the delayed stream with `futures::stream::once` + `unfold` instead — check `crates/caliban-provider/Cargo.toml`. Fallback without a new dep:)
```rust
            MockEntry::DelayedFirstChunk { delay, events } => {
                let delayed = stream::once(async move {
                    tokio::time::sleep(delay).await;
                })
                .flat_map(move |()| stream::iter(events.clone()));
                Ok(Box::pin(delayed))
            }
```
(For the fallback, the closure must own a `Vec` it can clone; capture `events` by value and `.clone()` inside — `events` is `Vec<Result<StreamEvent>>`; ensure `StreamEvent` and `Error` are `Clone`. If they are not `Clone`, use the `async-stream` variant, adding `async-stream` to `caliban-provider`'s dev/normal deps to match how sibling crates pull it.)

Add the builder method (near `with_silent_stream`, line 321):
```rust
    /// Enqueue a stream that stays silent for `delay` (exercising the prefill
    /// budget), then emits a normal `EndTurn` response with `text`.
    #[must_use]
    pub fn with_delayed_first_chunk(mut self, delay: Duration, text: &str) -> Self {
        let events = build_text_events(text, StopReason::EndTurn, 1);
        self.entries
            .push(MockEntry::DelayedFirstChunk { delay, events });
        self
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p caliban-agent-core --test recovery_stream_idle && cargo test -p caliban-provider`
Expected: PASS.

- [ ] **Step 5: Docs.** Search for a settings/config reference that enumerates knobs and add the two new settings + the ollama env vars:

Run: `rg -l "stream_idle_timeout_ms|auto_compact_threshold|tool_result_cap_chars" docs/`
For each hit that is a user-facing config table (e.g. under `docs/guide/`), add rows:
```markdown
| `stream_idle_timeout_ms` | 90000 | Silence (ms) tolerated after the first token before aborting a stalled stream. `0` disables the watchdog. |
| `stream_prefill_timeout_ms` | 300000 | Silence (ms) tolerated before the first token (slow local-model prefill). `0` falls back to the idle window. |
```
And, where env vars / ollama tuning are documented, note `OLLAMA_STREAM_IDLE_TIMEOUT_MS`, `OLLAMA_STREAM_PREFILL_TIMEOUT_MS` (widen the watchdog for ollama runs without a rebuild), and `OLLAMA_STREAM_TOTAL_TIMEOUT_MS` (re-impose a hard total cap on the ollama stream path; default off). If no such reference file exists, skip this step.

- [ ] **Step 6: Commit**

```bash
git add crates/caliban-provider/src/mock.rs crates/caliban-agent-core/tests/recovery_stream_idle.rs docs/
git commit -m "test(providers): end-to-end prefill-grace + mid-content-abort coverage (#263)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 8: Full gate + PR readiness

- [ ] **Step 1: Run the full CI-mirror gate**

Run:
```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --all-targets
cargo test --workspace
```
Expected: all four pass. If `fmt` complains, run `cargo fmt --all` and re-check. If clippy flags the new code, fix and re-run.

- [ ] **Step 2: Sanity-grep for stragglers** — any other `WatchedStream::new(` call sites (should be only the one in `stream/mod.rs`), any `DirectConfig {` literal missing `stream_total_timeout`:

Run: `rg 'WatchedStream::new\(' ; rg 'DirectConfig \{' crates caliban --type rust`
Fix any 2-arg `WatchedStream::new` or struct literal that now misses a field.

- [ ] **Step 3: Handoff to cai-ship-it** — the gate is green; proceed to open the PR (this is the sprint-mode Ship step).

## Self-Review

- **Spec coverage:** Component 1 → Task 1; Component 2 → Task 2; Component 3 → Tasks 1–2 (wiring); Component 4 → Task 3; Component 5 → Tasks 4–5 (+ ollama env total-timeout); ollama env budget override → Task 6; testing → Tasks 1–7; out-of-band eval → not in-plan (post-merge, noted). All spec sections mapped.
- **Placeholder scan:** every code step shows the actual code; the only conditional ("if such a doc file exists", "if `async-stream` isn't a dep") includes the exact fallback code and the command to decide. No TBD/TODO.
- **Type consistency:** `WatchedStream::new(inner, idle, prefill)` used identically in Tasks 1, 2, 7. `stream_total_timeout: Option<Duration>` consistent across ollama+openai. `apply_stream_watchdog(&self, &mut AgentConfig)` matches its call site. `apply_env_ms_override(&str, Option<&str>, &mut u32) -> bool` matches its test and call sites. `from_env_parts(Option<&str>, Option<&str>)` matches its callers and the retained `from_env_value` shim.
