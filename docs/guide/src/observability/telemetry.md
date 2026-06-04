# Telemetry & Cost

caliban tracks token usage and USD cost for every session using `caliban-telemetry` (ADR 0033). Cost accounting and context-window tracking work for all users regardless of whether OTLP export is enabled. OTLP emission to an external collector is opt-in.

## Cost accounting

After each provider response, `caliban-telemetry` multiplies token counts by per-model rates from a vendored YAML rate card. The card ships with known rates for Anthropic, OpenAI, Google, Bedrock, Vertex, and Ollama (Ollama rows are `$0.00`).

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
| `OTEL_LOGS_EXPORTER` | `otlp` | `otlp`, `console`, or `none` |
| `OTEL_METRICS_EXPORTER` | `otlp` | Same options |
| `OTEL_TRACES_EXPORTER` | `otlp` | Same options |
| `OTEL_LOG_USER_PROMPTS` | `0` | Include user prompt text in log spans |
| `OTEL_LOG_TOOL_DETAILS` | `0` | Include tool name/args in spans |
| `OTEL_LOG_TOOL_CONTENT` | `0` | Include full tool output in spans |
| `OTEL_LOG_RAW_API_BODIES` | `0` | Log raw provider request/response bodies (`0`, `1`, or `file:<dir>`) |

mTLS is configured via `OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE`, `OTEL_EXPORTER_OTLP_CLIENT_KEY`, and `OTEL_EXPORTER_OTLP_CERTIFICATE`.

```admonish warning title="Content logging is a privacy footgun"
`OTEL_LOG_USER_PROMPTS`, `OTEL_LOG_TOOL_CONTENT`, and `OTEL_LOG_RAW_API_BODIES` send potentially sensitive content to your collector. Ensure your collector pipeline is appropriately access-controlled before enabling these.
```

## Dynamic OTLP headers

Short-lived bearer tokens (e.g. from a secrets manager) can be injected without restarting caliban. Set `telemetry.otel_headers_helper` in your settings to a path; caliban spawns it at startup and periodically (`telemetry.otel_headers_refresh`, default `5m`), parses stdout as `key=value` lines, and merges them with `OTEL_EXPORTER_OTLP_HEADERS` (helper wins on collision).

Alternatively, the env-var escape hatch `CALIBAN_OTEL_HEADERS_HELPER=/path/to/script` achieves the same effect without a settings file.

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

## Related pages

- [Health Checks](./doctor.md) — `caliban doctor` and `/doctor`
- [Settings Reference](../configuration/reference.md) — `enable_telemetry` and `telemetry.*` keys
