# OpenTelemetry export + cost tracking — Design

**Date:** 2026-05-24
**Author:** john.ford2002@gmail.com
**Status:** Proposed
**Sub-project of:** caliban Rust agent harness
**ADR:** `adrs/0033-opentelemetry-and-cost.md`

## Goal

Bring caliban's observability surface to parity with Claude Code's
OpenTelemetry export and operator-visible cost tracking:

1. **OTLP export** of metrics, logs, and traces — gated by
   `CALIBAN_ENABLE_TELEMETRY=1`. Honor the full `OTEL_*` env-var contract
   Claude Code documents so the same dashboards work against caliban.
2. **Cost accumulator** — per-request token usage multiplied by per-model
   rate cards, surfaced as `cost.usage` and `token.usage` metrics, and
   exposed in-TUI via `/usage` and `/context`, plus a status-line
   `12% of 200K` context-window indicator.
3. **`/compact` slash** — manual trigger for the existing summarization
   path; emits a `compact.event` log and updates context utilization.

## Non-goals

- **No new compaction logic.** `/compact` triggers the existing
  summarization route (`RequestPurpose::Summarization`); only the slash +
  metric are new.
- **No automatic budget enforcement.** Cost is observed, not enforced.
  `--max-budget-usd` for headless mode is the right home for hard caps
  (ADR for that lives in J. Headless, not here).
- **No per-tool billing.** We track tokens; tool execution time is
  already covered by `caliban::timing` traces.
- **No third-party rate-card fetching.** Rates are vendored in YAML and
  updated in lockstep with releases. Unknown models cost `$0.00` with a
  warning.

## Architecture

```
caliban binary
  startup
    Telemetry::init_from_env()         ← reads CALIBAN_ENABLE_TELEMETRY + OTEL_*
      OtlpMetricExporter
      OtlpLogExporter
      OtlpSpanExporter
      tracing-opentelemetry layer      ← bridges existing `tracing` spans
                              │
                              ▼
caliban-telemetry
  CostAccumulator (Arc<Mutex<…>>)      ← session-scoped totals
  RateCard  (loaded once from rates.yaml)
  ContextWindow (per-session live count)
  MetricEmitter  (cost.usage / token.usage / session.count / …)

agent-core loop
  on Provider response:
    1. accumulate Usage into CostAccumulator
    2. RateCard::price(usage, &model) → USD
    3. MetricEmitter::emit_cost(model, purpose, usd, attrs)
    4. MetricEmitter::emit_tokens(model, type, n, attrs)
    5. ContextWindow::record(input_tokens)

TUI status bar
  poll ContextWindow::utilization() each frame → "12% of 200K"

slash commands
  /usage    → render CostAccumulator::summary()
  /context  → render ContextWindow::breakdown()
  /compact  → enqueue Summarization request; emit compact.event log
```

The new crate is `caliban-telemetry`. It owns OTLP wiring, the
`CostAccumulator`, the rate-card YAML, and the metric definitions.
`caliban-core` (agent loop) and `caliban` (binary / TUI) take it as a
dependency.

## Crate structure

```
crates/caliban-telemetry/
├── Cargo.toml
├── rates.yaml                      ← vendored rate cards (see schema below)
└── src/
    ├── lib.rs                      ← re-exports + Telemetry facade
    ├── init.rs                     ← env parsing + exporter wiring
    ├── attrs.rs                    ← standard attribute helpers
    ├── cost.rs                     ← CostAccumulator + RateCard
    ├── context.rs                  ← ContextWindow tracker
    ├── metrics.rs                  ← MetricEmitter (typed wrappers)
    ├── logs.rs                     ← structured-log emission helpers
    └── error.rs
```

### Cargo deps (new)

```toml
[dependencies]
opentelemetry            = { version = "0.27", features = ["metrics", "logs", "trace"] }
opentelemetry_sdk        = { version = "0.27", features = ["rt-tokio", "metrics", "logs"] }
opentelemetry-otlp       = { version = "0.27", features = ["grpc-tonic", "http-proto", "http-json", "metrics", "logs"] }
tracing-opentelemetry    = "0.28"
serde                    = { workspace = true, features = ["derive"] }
serde_yaml               = "0.9"
rust_decimal             = "1"       # USD math; never use f64 for money
humantime                = { workspace = true }
tokio                    = { workspace = true }
tracing                  = { workspace = true }
uuid                     = { workspace = true }   # anonymous user.id
```

## Env-var contract

caliban implements the full Claude Code contract verbatim where the
semantics map cleanly; we replace `CLAUDE_CODE_ENABLE_TELEMETRY` with
`CALIBAN_ENABLE_TELEMETRY` and inherit `OTEL_*` as-is.

| Env var                             | Default          | Effect |
| ----------------------------------- | ---------------- | ------ |
| `CALIBAN_ENABLE_TELEMETRY`          | `0`              | Master switch. `1` initializes exporters; `0` skips all OTel init (zero cost). |
| `OTEL_METRICS_EXPORTER`             | `otlp`           | `otlp` / `prometheus` / `console` / `none`. |
| `OTEL_LOGS_EXPORTER`                | `otlp`           | `otlp` / `console` / `none`. |
| `OTEL_TRACES_EXPORTER`              | `otlp`           | `otlp` / `console` / `none`. |
| `OTEL_EXPORTER_OTLP_PROTOCOL`       | `grpc`           | `grpc` / `http/json` / `http/protobuf`. |
| `OTEL_EXPORTER_OTLP_ENDPOINT`       | `http://localhost:4317` (grpc) / `:4318` (http) | Single shared endpoint. |
| `OTEL_EXPORTER_OTLP_HEADERS`        | _empty_          | `k1=v1,k2=v2`; merged with `otel_headers_helper` output. |
| `OTEL_EXPORTER_OTLP_{METRICS,LOGS,TRACES}_ENDPOINT` | _inherits_ | Per-signal overrides. |
| `OTEL_METRIC_EXPORT_INTERVAL`       | `60s`            | Periodic reader interval. |
| `OTEL_LOGS_EXPORT_INTERVAL`         | `5s`             | Log batch flush. |
| `OTEL_METRICS_INCLUDE_SESSION_ID`   | `1`              | Attach `session.id` attribute. |
| `OTEL_METRICS_INCLUDE_VERSION`      | `1`              | Attach `app.version` attribute. |
| `OTEL_METRICS_INCLUDE_ACCOUNT_UUID` | `0`              | Attach `user.id` attribute (anonymous UUID). |
| `OTEL_LOG_USER_PROMPTS`             | `0`              | If `1`, log user-typed prompt text. **Privacy-sensitive.** |
| `OTEL_LOG_TOOL_DETAILS`             | `0`              | If `1`, log tool input parameters. |
| `OTEL_LOG_TOOL_CONTENT`             | `0`              | If `1`, log tool result bodies. |
| `OTEL_LOG_RAW_API_BODIES`           | `0` or `file:DIR` | `1` logs full request/response JSON; `file:<dir>` writes to disk instead of OTLP. |
| `DISABLE_TELEMETRY`                 | _unset_          | If set to anything truthy, force-disable even if `CALIBAN_ENABLE_TELEMETRY=1`. |
| `DO_NOT_TRACK`                      | _unset_          | RFC-style opt-out. If set to `1`, behaves like `DISABLE_TELEMETRY=1`. |

### `otel_headers_helper` (dynamic auth)

A setting in `~/.config/caliban/settings.toml`:

```toml
[telemetry]
otel_headers_helper = "/opt/secrets/fetch-otel-header.sh"
```

Spawned at startup and on a configurable refresh interval (default
5 min). stdout is parsed as `k1=v1\nk2=v2`. Output merges with
`OTEL_EXPORTER_OTLP_HEADERS` (helper wins on key collision).

### mTLS

`OTEL_EXPORTER_OTLP_CERTIFICATE`, `OTEL_EXPORTER_OTLP_CLIENT_KEY`, and
`OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE` are honored verbatim. Paths
point at PEM-encoded files.

## Metric set

All metrics use the `caliban.` namespace; we mirror Claude Code's metric
names with the `claude_code.` prefix rewritten to `caliban.` to make
dual-shipping operators feasible.

| Metric                       | Instrument   | Attributes (beyond standard) | Notes |
| ---------------------------- | ------------ | ---------------------------- | ----- |
| `caliban.session.count`      | counter      | —                            | Incremented once per session at startup. |
| `caliban.lines_of_code.count`| counter      | `type=added\|removed`        | From `Edit`/`Write`/`MultiEdit` diff stats. |
| `caliban.pull_request.count` | counter      | —                            | Emitted by the (future) PR creation slash. |
| `caliban.commit.count`       | counter      | —                            | Emitted by Bash when a `git commit` succeeds. |
| `caliban.cost.usage`         | counter (f64)| `model`, `query_source`, `speed`, `effort`, `agent`, `skill`, `plugin` | USD. Float for currency is intentional in OTel; we compute with `rust_decimal` and convert at emit time. |
| `caliban.token.usage`        | counter (u64)| `model`, `type=input\|output\|cacheRead\|cacheCreation`, `query_source` | |
| `caliban.code_edit_tool.decision` | counter | `tool=Edit\|Write\|NotebookEdit`, `decision=accept\|reject`, `source=config\|hook\|user_ask\|user_modal`, `language` | |
| `caliban.active_time.total`  | counter (f64)| `source=user\|cli`           | Seconds; `user` excludes idle, `cli` is wall-time. |

### Standard attributes (every metric + log + span)

| Attribute       | Source |
| --------------- | ------ |
| `session.id`    | `caliban-sessions::SessionId` (UUIDv7). |
| `app.version`   | `env!("CARGO_PKG_VERSION")` at build. |
| `app.name`      | `"caliban"`. |
| `user.id`       | First-run UUID persisted at `~/.config/caliban/anon-id`; only attached when `OTEL_METRICS_INCLUDE_ACCOUNT_UUID=1`. |
| `provider`      | Provider name (`anthropic` / `openai` / `bedrock` / `vertex` / `google` / `ollama`). |
| `host.os`       | `std::env::consts::OS`. |
| `tty`           | `is-terminal::is_terminal(stdout)` boolean. |

### `query_source` semantics

| Value         | Meaning                                                |
| ------------- | ------------------------------------------------------ |
| `main`        | `RequestPurpose::MainLoop`.                            |
| `subagent`    | `RequestPurpose::SubAgent`.                            |
| `auxiliary`   | `Summarization`, `FastClassifier`, `Embedding`, `Other`. |

### Cardinality knobs

`OTEL_METRICS_INCLUDE_SESSION_ID=0` strips `session.id` from all metrics
(keeps it on logs and spans). `OTEL_METRICS_INCLUDE_VERSION=0` strips
`app.version`. Operators with high session churn use these to keep
backend cardinality bounded.

## Rate-card schema

`crates/caliban-telemetry/rates.yaml`:

```yaml
# Per-1M-token prices in USD. Effective dates included for audit; the
# loader picks the entry whose `effective_from` is the latest date ≤ now.
version: 1
providers:
  anthropic:
    - model_glob: "claude-opus-4-7*"
      effective_from: 2026-04-01
      input_per_mtok: 15.00
      output_per_mtok: 75.00
      cache_read_per_mtok: 1.50
      cache_creation_per_mtok: 18.75
    - model_glob: "claude-sonnet-4-7*"
      effective_from: 2026-04-01
      input_per_mtok: 3.00
      output_per_mtok: 15.00
      cache_read_per_mtok: 0.30
      cache_creation_per_mtok: 3.75
    - model_glob: "claude-haiku-*"
      effective_from: 2026-01-01
      input_per_mtok: 0.80
      output_per_mtok: 4.00
      cache_read_per_mtok: 0.08
      cache_creation_per_mtok: 1.00
  openai:
    - model_glob: "gpt-5*"
      effective_from: 2026-03-01
      input_per_mtok: 5.00
      output_per_mtok: 20.00
    - model_glob: "gpt-4o*"
      effective_from: 2025-11-01
      input_per_mtok: 2.50
      output_per_mtok: 10.00
  google:
    - model_glob: "gemini-2.5-pro*"
      effective_from: 2026-02-01
      input_per_mtok: 1.25
      output_per_mtok: 10.00
  bedrock:
    - model_glob: "anthropic.claude-opus-4-7*"
      effective_from: 2026-04-01
      input_per_mtok: 15.00
      output_per_mtok: 75.00
      cache_read_per_mtok: 1.50
      cache_creation_per_mtok: 18.75
  vertex:
    - model_glob: "claude-opus-4-7@*"
      effective_from: 2026-04-01
      input_per_mtok: 15.00
      output_per_mtok: 75.00
  ollama:
    - model_glob: "*"
      effective_from: 2026-01-01
      input_per_mtok: 0.00
      output_per_mtok: 0.00
```

Unknown `(provider, model)` pairs match no entry → price `$0.00`,
`tracing::warn!` once per session (debounced) under
`target = "caliban::cost"`.

Operators can override the bundled file via
`CALIBAN_RATES_YAML=/path/to/rates.yaml`.

## Public API sketches

```rust
// crates/caliban-telemetry/src/lib.rs

pub use cost::{CostAccumulator, CostBreakdown, RateCard};
pub use context::{ContextWindow, ContextBreakdown};
pub use init::{Telemetry, TelemetryConfig};
pub use metrics::{MetricEmitter, QuerySource};

/// Owns the OTLP pipeline + per-session cost/context state.
/// Constructed once at startup; cloned cheaply (Arc inside).
#[derive(Clone)]
pub struct Telemetry {
    pub metrics: MetricEmitter,
    pub cost:    Arc<CostAccumulator>,
    pub context: Arc<ContextWindow>,
}

impl Telemetry {
    /// Read CALIBAN_ENABLE_TELEMETRY + OTEL_* env. When disabled, returns a
    /// no-op shim that satisfies the same trait surface (zero allocations).
    pub fn init_from_env(session_id: SessionId) -> Result<Self, TelemetryError>;

    /// Flush all pending batches; called on shutdown.
    pub async fn shutdown(self);
}
```

```rust
// crates/caliban-telemetry/src/cost.rs

pub struct CostAccumulator { /* Arc<Mutex<Inner>> */ }

impl CostAccumulator {
    pub fn record(&self, model: &str, usage: &Usage, purpose: RequestPurpose);
    pub fn total_usd(&self) -> Decimal;
    pub fn breakdown(&self) -> CostBreakdown;     // per-model totals
}

pub struct CostBreakdown {
    pub total_usd: Decimal,
    pub by_model: Vec<ModelCost>,                 // (model, input, output, cache, usd)
    pub by_query_source: BTreeMap<QuerySource, Decimal>,
}
```

```rust
// crates/caliban-telemetry/src/context.rs

pub struct ContextWindow { /* Arc<Mutex<Inner>> */ }

impl ContextWindow {
    pub fn set_capacity(&self, max_input_tokens: u32);
    pub fn record_message(&self, role: Role, tokens: u32, kind: MessageKind);
    pub fn utilization(&self) -> f32;             // 0.0..=1.0
    pub fn breakdown(&self) -> ContextBreakdown;  // per-MessageKind tokens
}

pub enum MessageKind { System, MemoryPrefix, UserText, AssistantText, ToolCall, ToolResult, Summarized }
```

## TUI surfaces

### Status-line context indicator

The existing status bar (`caliban/src/tui.rs`) gains a new segment to
the right of the elapsed timer:

```
… 03:42 elapsed   12% of 200K
```

Color rules:
- `< 50%` — default fg
- `50%..80%` — yellow
- `80%..95%` — orange
- `≥ 95%` — red bold

Source: `Telemetry::context.utilization()` and
`set_capacity()` is called when the provider's `Capabilities` are first
resolved.

### `/usage` overlay

```
┌─ Usage ─────────────────────────────────────────────────────────────┐
│ Session 0193f8…  app v0.4.1  · 03:42 active                         │
│                                                                      │
│   Total                              $0.482                         │
│                                                                      │
│   By model                                                          │
│     claude-opus-4-7        in   42,103   out   8,902   $0.318       │
│     claude-haiku-4-7       in   12,800   out     412   $0.012       │
│     gpt-5                  in   18,000   out   1,290   $0.115       │
│                                                                      │
│   Cache savings (vs no-cache)         $0.214 (30.7%)                │
└──────────────────────────────────────────────────────────────────────┘
[esc] close
```

Cache savings: `(cache_read_input_tokens) × (input_rate - cache_read_rate)`.

### `/context` overlay

```
┌─ Context window ────────────────────────────────────────────────────┐
│ Model claude-opus-4-7 · 200,000-token window · 12% used (24,103)    │
│                                                                      │
│   System prompt           1,402   ▍                                 │
│   Memory prefix           4,212   █▌                                │
│   User text               2,801   █                                 │
│   Assistant text          9,103   ███▎                              │
│   Tool calls              3,802   █▍                                │
│   Tool results            2,783   █                                 │
│   Summarized               0                                        │
└──────────────────────────────────────────────────────────────────────┘
[esc] close   [c] compact now
```

### `/compact` slash

Enqueues a `RequestPurpose::Summarization` request that replays the
existing summarization route from `caliban-model-router`. While running:

- emit log `compact.event` with `before_tokens` / `after_tokens` /
  `model` / `duration_ms` attributes,
- update `ContextWindow` with the new compacted message,
- bump `caliban.token.usage` from the compaction call itself
  (`query_source=auxiliary`).

## Initialization & lifecycle

```rust
// crates/caliban/src/main.rs (sketch)

let session_id = SessionId::new();
let telemetry  = caliban_telemetry::Telemetry::init_from_env(session_id)?;
let cost       = telemetry.cost.clone();
let context    = telemetry.context.clone();

// agent-core loop accepts the emitter as a hook
let agent = AgentCore::builder()
    .telemetry(telemetry.clone())
    .build();

// on shutdown
telemetry.shutdown().await;
```

When `CALIBAN_ENABLE_TELEMETRY=0` (default), `Telemetry::init_from_env`
returns a no-op variant in ~10 µs and emits no spans. Cost + context
*still work* — they're not gated by telemetry; only the OTLP emission
is. `/usage` and `/context` therefore work for everyone.

## Privacy & opt-outs

- `DISABLE_TELEMETRY=1` and `DO_NOT_TRACK=1` both force-disable OTLP
  regardless of `CALIBAN_ENABLE_TELEMETRY`. Cost/context still work
  locally.
- `OTEL_LOG_USER_PROMPTS`, `OTEL_LOG_TOOL_DETAILS`, `OTEL_LOG_TOOL_CONTENT`,
  `OTEL_LOG_RAW_API_BODIES` all default to `0`. The first time a user
  enables any of them in interactive mode, the TUI prints a one-time
  yellow notice: `content logging enabled — verify your OTLP backend
  redacts secrets`.
- `OTEL_LOG_RAW_API_BODIES=file:<dir>` writes JSON to disk *instead* of
  OTLP — useful for local debugging without shipping prompts off-box.
- `user.id` (anonymous UUID) is off by default
  (`OTEL_METRICS_INCLUDE_ACCOUNT_UUID=0`); the UUID lives at
  `~/.config/caliban/anon-id` and can be deleted to rotate.

## Error handling

```rust
pub enum TelemetryError {
    OtlpExporter   { source: opentelemetry_otlp::Error },
    InvalidRates   { path: PathBuf, source: serde_yaml::Error },
    InvalidEndpoint{ value: String },
    HeadersHelper  { path: PathBuf, source: io::Error },
}
```

Initialization failures degrade gracefully: log
`tracing::error!(target = "caliban::telemetry")` and continue with the
no-op variant. We never block agent startup on a flaky OTLP collector.

## Testing strategy

15 enumerated tests:

1. `init_from_env` returns the no-op variant when `CALIBAN_ENABLE_TELEMETRY` is unset.
2. `init_from_env` honors `DISABLE_TELEMETRY=1` even when `CALIBAN_ENABLE_TELEMETRY=1`.
3. `init_from_env` honors `DO_NOT_TRACK=1`.
4. Rate-card load: `claude-opus-4-7-20260423` matches the `claude-opus-4-7*` row.
5. Rate-card load: unknown model returns `$0.00` and emits exactly one warning per session.
6. Rate-card load: `effective_from` picks the latest entry ≤ now when two rows match a glob.
7. `CostAccumulator::record` sums input/output/cache_read/cache_creation correctly across multiple calls.
8. `CostAccumulator::breakdown` groups by `(model, query_source)`.
9. Decimal math: `1_234_567` input tokens × `$15/Mtok` produces `$18.518505` (no float drift).
10. `ContextWindow::utilization` returns `0.0` before `set_capacity`; `0.5` after recording 100K of a 200K window.
11. `ContextWindow::breakdown` segregates `System` from `MemoryPrefix` correctly.
12. `MetricEmitter::emit_cost` attaches all standard attributes when `OTEL_METRICS_INCLUDE_SESSION_ID=1`.
13. `MetricEmitter::emit_cost` strips `session.id` when `OTEL_METRICS_INCLUDE_SESSION_ID=0`.
14. `otel_headers_helper` stdout `Authorization=Bearer X\nX-Tenant=Y` parses to two headers; collisions with `OTEL_EXPORTER_OTLP_HEADERS` favor helper.
15. OTLP shutdown flushes pending batches inside 2s (validated against an in-process collector fixture).

Integration test (`crates/caliban-telemetry/tests/otlp_roundtrip.rs`)
spins an `opentelemetry-otlp` HTTP collector mock and asserts a
`cost.usage` metric with the right attribute set arrives end-to-end.

## Risks

- **Vendored rate cards drift from reality.** Mitigation: monthly
  release cadence updates `rates.yaml`; document
  `CALIBAN_RATES_YAML` for operators who need fresher numbers.
- **OTLP collector backpressure** could stall the agent if exporters are
  synchronous. Mitigation: use `opentelemetry_sdk`'s batch span / log /
  metric readers (already async + bounded).
- **Cardinality explosion.** Per-skill / per-agent attributes can blow
  up backend storage. Mitigation: cardinality knobs documented; default
  attribute set is the minimum Claude Code dashboards expect.
- **Privacy regression** if `OTEL_LOG_*` envs ship raw prompts to a
  shared collector. Mitigation: one-time TUI notice on first enable;
  README §Privacy documents the surface.
- **Decimal-to-double conversion** at OTLP emit may introduce float
  rounding in dashboards. Mitigation: emit `cost.usage` as an
  `opentelemetry`-native `f64` (it's a `Histogram` in some SDKs; we use
  `Counter<f64>`); internal accounting stays Decimal.
- **Status-bar repaint cost** on every frame for the percent indicator.
  Mitigation: `ContextWindow::utilization` is `Arc<AtomicU16>` (bps);
  read is lock-free.

## Acceptance criteria

- `cargo build --workspace` clean; `clippy --workspace --all-targets -- -D warnings` clean; `fmt --check` clean.
- `CALIBAN_ENABLE_TELEMETRY=0` startup overhead is < 1 ms (benchmark).
- With `CALIBAN_ENABLE_TELEMETRY=1` pointed at a local OTLP collector, a
  one-turn `/usage` smoke flow produces `caliban.cost.usage`,
  `caliban.token.usage` (×4 types), and `caliban.session.count` metrics
  with all standard attributes attached.
- `/usage`, `/context`, `/compact` slash commands render the documented
  overlays.
- Status-bar percent indicator updates within 1 frame of each new
  assistant turn.
- All 15 unit tests + the OTLP roundtrip integration test pass.
- `docs/parity-gap-matrix.md` rows under **K. Observability / cost** —
  `/context`, `/usage`, `/compact`, `Cost ($) tracking`, `OpenTelemetry
  export (OTLP metrics / logs / traces)`, and `Metric set` — move
  🔴 → ✅.
- README's new "Observability" section documents `CALIBAN_ENABLE_TELEMETRY`,
  the `OTEL_*` matrix, and the `otel_headers_helper` setting.
- ADR 0033 in `accepted` status (this spec's prerequisite).
