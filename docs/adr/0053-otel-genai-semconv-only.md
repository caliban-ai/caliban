# ADR 0053 · OpenTelemetry GenAI semantic conventions for LLM tracing (semconv-only)

- **Status:** accepted
- **Date:** 2026-07-05

## Context

ADR 0033 established that caliban exports telemetry over OTLP and accounts for
cost, but it did not fix the *attribute vocabulary* for LLM-call tracing. A
2026-07-05 review of the shipped telemetry found:

- The OTLP exporter is not actually constructed — the `opentelemetry`,
  `opentelemetry-otlp`, and `tracing-opentelemetry` crates sit behind an
  off-by-default `otlp` Cargo feature with stubbed pipeline bodies.
- What instrumentation exists uses a fully custom scheme: `caliban.*` metrics (a
  rename of Claude Code's `claude_code.*`) and `tracing` spans whose fields are
  bare names (`model`, `session`, `tool`, `id`). There is **zero `gen_ai.*`
  usage** anywhere in the workspace.

OTLP-native LLM-observability backends (Langfuse, Arize, Grafana, Honeycomb)
read the **OpenTelemetry GenAI semantic conventions** (`gen_ai.*`) — an open
standard, though still Experimental and evolving (recently split into the
dedicated `open-telemetry/semantic-conventions-genai` repository). Because the
spec is unstable, backends read a *union* of dialects, and several offer their
own vendor-specific attributes (e.g. Langfuse's `langfuse.observation.*`) that
give richer, backend-tailored fidelity — at the cost of lock-in.

We had to decide which vocabulary caliban emits. Options weighed:

1. **Semconv-only** — emit `gen_ai.*` and nothing else.
2. **Semconv + vendor extensions** — add `langfuse.*` (and/or others) for the two
   things pure semconv cannot express cleanly into a given backend: cost and
   detailed token usage.
3. **Vendor-first** — target one backend's richest attribute set.

## Decision

We will instrument caliban's LLM calls and tool executions using the
**OpenTelemetry GenAI semantic conventions only** (`gen_ai.*`). Specifically:

- **No vendor-specific attributes.** No `langfuse.*` or any other
  backend-proprietary keys. Portability across OTLP backends and adherence to the
  open standard outweigh per-backend fidelity.
- **No cost attribute on spans.** Cost is a derived, pricing-dependent quantity
  and is not part of the GenAI semconv (`gen_ai.usage.cost` is a non-standard
  extension). Backends compute spend from `gen_ai.request.model` +
  `gen_ai.usage.*_tokens` against their own price tables. Caliban's existing
  `CostAccumulator` remains an **internal** concern feeding `/usage` and the
  status bar; cost accounting per ADR 0033 is unchanged and is a metric/internal
  signal, not a trace attribute.
- **Token usage uses the standard keys only** — `gen_ai.usage.input_tokens` /
  `gen_ai.usage.output_tokens`. The cache-token breakdown caliban tracks
  (`cache_read` / `cache_creation`) has no stable semconv key; we fold cache
  reads into `input_tokens` and accept that per-cache-type granularity does not
  surface at the observability layer.
- **Message content** (`gen_ai.input.messages` / `gen_ai.output.messages`) is
  captured only when the operator opts in via `OTEL_LOG_USER_PROMPTS`; off by
  default.

This ADR records the *vocabulary* decision. It **builds on, and does not
supersede,** ADR 0033 (which established OTLP export and cost accounting). The
exporter wiring and concrete span shape are tracked under epic #375 and its
children (#377–#380).

Target generation span:

```
gen_ai.operation.name = "chat"
gen_ai.provider.name
gen_ai.request.model / gen_ai.response.model
gen_ai.request.{temperature,max_tokens,top_p}
gen_ai.response.finish_reasons
gen_ai.usage.input_tokens / gen_ai.usage.output_tokens
gen_ai.input.messages / gen_ai.output.messages   (gated by OTEL_LOG_USER_PROMPTS)
```

Tools: a child `execute_tool` span carrying `gen_ai.tool.name` /
`gen_ai.tool.call.id`.

## Consequences

- **Positive:** caliban's traces render correctly in any OTLP-native backend with
  no per-vendor code; we stay on the open standard with no lock-in; the emitter is
  simpler — one vocabulary, no cost/vendor branches.
- **Negative:** we forgo backend-tailored fidelity — notably, cache-discount
  cost accuracy and per-cache-type token breakdowns will not surface in a backend
  UI. Because the semconv is Experimental, the exact keys (especially content:
  `gen_ai.input.messages` vs the older `gen_ai.prompt`) may shift, and a given
  backend may lag the newest attribute — content may render blank until the
  backend catches up.
- **Revisit if:** a target backend proves unable to read semconv content/usage
  that matters operationally, or the GenAI semconv stabilizes a cost/cache-token
  vocabulary — at which point adopting the standardized keys (still not
  vendor-specific) becomes worthwhile.

Note: ADR 0033 also carries a separate documented-vs-implemented drift (settings
fields `otel_headers_helper` / `otel_headers_refresh` are described but
unimplemented); that reconciliation is tracked in #381 and is out of scope here.
