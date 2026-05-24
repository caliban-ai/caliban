# ADR 0033 · OpenTelemetry export + cost tracking

- **Status:** proposed
- **Date:** 2026-05-24
- **Author:** john.ford2002@gmail.com
- **Spec:** `docs/superpowers/specs/2026-05-24-otel-and-cost-design.md`

## Context

caliban already has `tracing` instrumentation under
`caliban::tools`, `caliban::cache`, `caliban::memory`, `caliban::mcp`,
`caliban::skills`, and `caliban::timing`. What it lacks: (a) a way to
ship those signals to an OTLP backend, (b) any concept of dollar cost
on completions, (c) operator-visible context-window utilization. Claude
Code ships all three and operators depend on them for billing, capacity
planning, and right-sizing model choices. We need parity.

The Claude Code env-var contract (`CLAUDE_CODE_ENABLE_TELEMETRY`,
`OTEL_*`) is well-known and supported by every OTLP backend Anthropic
customers run; rather than invent our own knobs we adopt it verbatim
with `CALIBAN_` substitutions only where required.

## Decision

### One new crate, `caliban-telemetry`, owns OTLP + cost + context

It pulls `opentelemetry`, `opentelemetry-otlp`, `tracing-opentelemetry`,
`serde_yaml`, and `rust_decimal`. `caliban-core` (agent loop) and
`caliban` (binary / TUI) depend on it. Other crates do not — they emit
via the existing `tracing` macros and `tracing-opentelemetry` bridges
those into OTLP automatically.

### Master switch is `CALIBAN_ENABLE_TELEMETRY=1`

Defaults to `0`. When `0`, `Telemetry::init_from_env` returns a no-op
shim in ~10 µs and no exporter is constructed. `DISABLE_TELEMETRY=1`
and `DO_NOT_TRACK=1` both force-disable even when
`CALIBAN_ENABLE_TELEMETRY=1` (privacy belt-and-braces).

### `OTEL_*` env vars adopted verbatim from Claude Code

Endpoint, protocol, headers, exporters, intervals, cardinality knobs,
content-control toggles, and mTLS paths — all standard OTel SDK env
names. We do *not* invent caliban-specific names for things OTel
already standardizes. The only caliban-prefixed extras are
`CALIBAN_ENABLE_TELEMETRY` (master switch) and `CALIBAN_RATES_YAML`
(rate-card override path).

### Cost is observed, not enforced

`CostAccumulator` records token usage from every provider response,
multiplies by `RateCard`-resolved per-1M-token prices, and exposes
totals to `/usage` plus the `caliban.cost.usage` metric. **Hard caps
(`--max-budget-usd`) live in headless mode**, not here. This ADR is
purely about visibility; budget enforcement is a downstream concern
that consumes the same `CostAccumulator`.

### Rate cards are vendored YAML, updated in lockstep with releases

`crates/caliban-telemetry/rates.yaml` ships with known rates for
Anthropic, OpenAI, Google, Bedrock, Vertex, and Ollama (the last being
a `$0.00` row for completeness). Unknown `(provider, model)` pairs
match no entry, cost `$0.00`, and emit a single debounced warning per
session. Operators can override via `CALIBAN_RATES_YAML=/path`. We do
*not* fetch rate cards from any third-party API at runtime — the
dependency is one PR-with-a-cron-reminder, not a network call.

### USD math uses `rust_decimal`, never `f64`

Financial accumulation drifts under `f64`. We compute in `Decimal` and
convert to `f64` only at the OTLP emit boundary (the OTel SDK insists).

### Context window is independent of telemetry

`ContextWindow` is part of `caliban-telemetry` for code-locality
reasons but **does not require OTel enabled** to work. `/usage`,
`/context`, and the status-bar percent indicator function for every
caliban user regardless of `CALIBAN_ENABLE_TELEMETRY`. Only OTLP
emission is gated.

### `/compact` reuses existing summarization, just adds a slash + metric

`RequestPurpose::Summarization` already wires through
`caliban-model-router` to a summary-tuned model. The slash command
enqueues that purpose at the head of the loop and emits a `compact.event`
log. No new model routing logic is introduced by this ADR.

### `otel_headers_helper` is a per-startup helper script + refresh

Settings field `[telemetry].otel_headers_helper` points at a path;
caliban spawns it at startup and on a configurable interval
(`telemetry.otel_headers_refresh`, default `5m`), parses stdout as
`k=v\n…`, merges with `OTEL_EXPORTER_OTLP_HEADERS` (helper wins on
collision). This is how operators put short-lived bearer tokens in
front of their collector without checking secrets into env files.

## Consequences

- **Positive:** Closes six 🔴 rows in the parity matrix under
  K. Observability / cost (`/context`, `/usage`, `/compact`,
  Cost tracking, OTLP export, Metric set) in one initiative. Reuses the
  industry-standard `OTEL_*` env contract so any existing OTLP backend
  (Honeycomb, Grafana, Datadog, Tempo, Loki) works out-of-the-box.
  Decoupling cost/context from OTel emission means the operator-visible
  features (`/usage`, status-bar percent) work for everyone — including
  the airgapped offline case.
- **Negative:** Adds ~5 transitive deps via `opentelemetry-otlp`
  (tonic, h2, prost, etc.). Vendored rate cards need monthly refresh
  discipline. `rust_decimal` is yet another money library; we'll need
  a brief style note on when to use it. Content-logging knobs are a
  privacy footgun if operators misconfigure their collector — README
  must call this out prominently.
- **Revisit if:** OTel SDK ships a stable currency / cost convention
  (currently absent), in which case we align metric attribute names.
  If `rust_decimal` proves overkill for the precision we need, swap to
  fixed-point i64 cents. If operators clamor for runtime rate-card
  fetching (e.g. integration with their FinOps platform), add a
  `RateCardSource::Url` variant.
