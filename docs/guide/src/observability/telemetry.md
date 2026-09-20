# Telemetry & Cost

caliban tracks token usage and USD cost for every session using `caliban-telemetry` (ADR 0033). Cost accounting and context-window tracking work for all users regardless of whether OTLP export is enabled. OTLP emission to an external collector is opt-in.

## Cost accounting

After each provider response, `caliban-telemetry` multiplies token counts by per-model rates from a vendored YAML rate card. The card ships with known rates for Anthropic, OpenAI, Google, Bedrock, and Vertex. Local models served through the OpenAI adapter have no rate entry, so they contribute `$0.00`.

Unknown `(provider, model)` pairs contribute `$0.00` and emit a single debounced warning per session. Rates are updated in-tree; operators can override the card with `CALIBAN_RATES_YAML=/path/to/rates.yaml`.

USD arithmetic uses `rust_decimal` internally to avoid floating-point drift. Values are converted to `f64` only at OTLP emit boundaries.

## Slash commands

These commands work in the TUI regardless of whether OTLP export is on.

| Command | Description |
|---------|-------------|
| `/cost` | Cumulative USD spend with a per-model breakdown |
| `/usage` | Cumulative token counts (input and output) with per-model breakdown |
| `/context` | Context-window utilization — per-message-kind token breakdown, percentage of the model's context window used |

The `/cost` and `/usage` overlays share the same underlying `CostAccumulator`; `/cost` leads with dollar amounts, `/usage` leads with token counts. `/context` draws on `ContextWindow`, which is updated independently of OTLP emission.

## Enabling OTLP export

OTLP export is off by default. Turn it on with the `CALIBAN_ENABLE_TELEMETRY` environment variable or the `enable_telemetry` setting:

**Environment variable (any session)**

```bash
CALIBAN_ENABLE_TELEMETRY=1 caliban
```

**`settings.toml` / `settings.json` (persistent)**

```toml
enable_telemetry = true
```

Privacy opt-outs `DISABLE_TELEMETRY=1` and `DO_NOT_TRACK=1` force-disable OTLP emission even when the master switch is on.

## OTLP configuration

caliban adopts the standard `OTEL_*` env-var contract verbatim:

| Variable | Default | Purpose |
|----------|---------|---------|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | — | Collector endpoint (required for OTLP) |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | `grpc` | `grpc`, `http/protobuf`, or `http/json` |
| `OTEL_EXPORTER_OTLP_HEADERS` | — | Static auth / routing headers (`k=v,k2=v2`) |
| `OTEL_METRIC_EXPORT_INTERVAL` | `60s` | How often metrics are flushed |
| `OTEL_METRICS_EXPORTER` | `otlp` | `otlp` exports metrics; any other value (e.g. `none`) suppresses metric export |
| `OTEL_TRACES_EXPORTER` | `otlp` | `otlp` exports spans; any other value (e.g. `none`) suppresses span export |
| `OTEL_LOG_USER_PROMPTS` | `0` | Include prompt/completion content on `gen_ai` spans (ADR 0053) |

caliban recognises the standard mTLS env vars — `OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE`, `OTEL_EXPORTER_OTLP_CLIENT_KEY`, and `OTEL_EXPORTER_OTLP_CERTIFICATE` — but wiring them into the exporter's TLS config is not yet implemented (tracked in #465): today they are parsed and otherwise ignored.

```admonish warning title="Content logging is a privacy footgun"
`OTEL_LOG_USER_PROMPTS` sends potentially sensitive prompt/completion content to your collector. Ensure your collector pipeline is appropriately access-controlled before enabling it.
```

## Dynamic OTLP headers

Short-lived bearer tokens (e.g. from a secrets manager) can be supplied by a helper script. Set `CALIBAN_OTEL_HEADERS_HELPER=/path/to/script`; caliban runs it **once at startup**, parses stdout as `key=value` lines, and merges them with `OTEL_EXPORTER_OTLP_HEADERS` (helper wins on collision) before the exporter is built.

The helper is a one-shot, env-only knob — there is no settings key and no periodic refresh thread (ADR 0033, reconciled in #381). A token minted at startup is used for the lifetime of the process; restart caliban to pick up a new one.

## Traces

When OTLP export is enabled, caliban emits OpenTelemetry traces following the [OTel GenAI semantic conventions](https://opentelemetry.io/docs/specs/semconv/gen-ai/) (ADR 0053):

- a `gen_ai` chat-generation span per model request, carrying the `gen_ai.*` request/response attributes;
- an `execute_tool` span per tool call, carrying `gen_ai.tool.*` attributes, nested under the model request that issued it.

Prompt and completion content is **not** recorded on spans by default. Set `OTEL_LOG_USER_PROMPTS=1` to attach the `gen_ai` input/output messages to spans — enable it only against an access-controlled collector, since those messages contain user content.

## Metric names

OTLP metrics use the `caliban.` prefix (mirroring Claude Code's `claude_code.` names):

| Metric | Kind | Description |
|--------|------|-------------|
| `caliban.session.count` | Counter | Session start/end lifecycle events |
| `caliban.cost.usage` | Counter (USD) | Cumulative cost per session |
| `caliban.token.usage` | Counter | Input and output tokens |
| `caliban.lines_of_code.count` | Counter | Lines touched by file-edit tools |
| `caliban.code_edit_tool.decision` | Counter | Permission decisions on edit tools |
| `caliban.active_time.total` | Gauge (seconds) | Wall time the agent loop ran |

Of these, only `caliban.session.count` currently reaches the collector. The other instruments are defined and the metrics pipeline is wired end-to-end, but their emit call sites are not yet connected (cost/token/active-time emits are tracked in #467). This does not affect the in-session views: `/cost`, `/usage`, and `/context` read caliban's in-process accumulators directly, so they show real numbers whether or not OTLP export is enabled.

## Related pages

- [Health Checks](./doctor.md) — `caliban doctor` and `/doctor`
- [Settings Reference](../configuration/reference.md) — `enable_telemetry` and `telemetry.*` keys
