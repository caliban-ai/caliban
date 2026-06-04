# Troubleshooting

This page covers the most common problems operators encounter and how to fix them. Start with `caliban doctor` — it checks the most likely failure points in one command.

---

## Running `caliban doctor`

```bash
caliban doctor          # quick sanity checks
caliban doctor --deep   # adds provider auth pings (costs one API call per provider)
```

The output lists each check with a `✓` (pass), `!` (warning), or `✗` (fail) prefix. Warnings such as "no CLAUDE.md found in ancestry" or "no scope files found" are informational; failures indicate something caliban cannot proceed without.

```admonish tip title="Deep checks cost an inference"
`--deep` issues a real model request to confirm provider auth. Run it when you suspect a key or endpoint problem, not on every invocation.
```

---

## Provider authentication failures

**Symptoms:** `Error: ANTHROPIC_API_KEY is not set`, `OPENAI_API_KEY is not set`, or similar on startup.

**Fixes:**

1. Export the relevant key in your shell:

   ```bash
   export ANTHROPIC_API_KEY=sk-ant-...
   export OPENAI_API_KEY=sk-...
   ```

2. Or configure `apiKeyHelper` in your settings file to fetch credentials dynamically. See [Configuring Providers & API Keys](./providers/configuration.md).

3. Run `caliban doctor --deep` to confirm the key reaches the provider.

**Malformed base URL:** If you set `OPENAI_BASE_URL` to a URL that cannot be parsed (e.g. `not://a:url`), caliban may report a misleading "API key not set" error. Verify the URL is a valid HTTP/HTTPS address before exporting it.

---

## Qwen3 on LM Studio: tool calls leak into reasoning

When running a **Qwen3 reasoning model via LM Studio** (MLX engine), you may see tool calls appear inside the model's thinking/reasoning channel rather than as structured `tool_use` blocks. The practical effects:

- 2-step tool chains (e.g. Glob → Read) usually complete correctly.
- Chains of 3 or more steps stall: the model re-emits the first tool call across multiple turns and hits `--max-turns` without progressing.

This is an **LM Studio MLX engine limitation**, not a caliban defect. The same Qwen3 model on **Ollama (GGUF)** parses tool calls correctly — the leak does not reproduce there.

```admonish warning title="LM Studio + Qwen3 reasoning models"
Multi-step agentic tasks (3+ tool calls) are unreliable when using Qwen3 reasoning models through LM Studio's MLX path. For agentic work, switch to Ollama or another server that handles Qwen-native `<tool_call>` XML parsing server-side.
```

**Workarounds:**

| Situation | Workaround |
|---|---|
| Need Qwen3 specifically | Switch to Ollama: `--provider ollama --model qwen3.5:9b` |
| Must use LM Studio | Limit chains to at most 2 tool calls; use `--max-turns` to prevent runaway loops |
| Reasoning is optional | Use a non-reasoning Qwen model (e.g. `qwen2.5-coder-7b-instruct`) |

---

## Ollama: `tool_call_id` not round-tripped

Caliban's Ollama provider does not correlate `tool_call_id` across the request/response boundary — it is set on the outgoing tool result but is not echoed back by the Ollama server. This is a known limitation of the Ollama API and does not affect tool dispatch correctness in practice.

```admonish note
If you are building a custom consumer of the `stream-json` output and need to correlate `tool_use` and `tool_result` frames, use the `id` field on the `tool_use` frame and the `tool_use_id` field on `tool_result` as emitted by caliban — they match correctly on the client side regardless of provider.
```

---

## Parallel sub-agents slow on self-hosted Ollama

If you run parallel sub-agents (`AgentTool`) against a self-hosted Ollama instance and they are slower than expected, the backend may be serialising requests due to `OLLAMA_NUM_PARALLEL=1` (the default on most hardware).

On a `NUM_PARALLEL=1` backend, parallel sub-agents do **not** increase throughput — every inference still queues at the single model slot, and the per-sub-agent overhead (a full reasoning + summary loop per agent) makes total wall time significantly longer than the parent doing the same work inline.

**Options:**

- Raise `OLLAMA_NUM_PARALLEL` on the server if your GPU has enough VRAM for multiple KV-cache allocations.
- Use `--no-sub-agent` and let the parent model read files inline.
- Switch to a hosted provider (Anthropic, OpenAI) where each sub-agent gets independent fleet capacity.
- Cap dispatch with `--parallel-tool-limit N` to limit concurrent sub-agent calls.

```admonish tip title="Sub-agents on a serialising backend"
Parallel sub-agents still provide **context isolation** (each sub-agent gets a fresh context window) even when `NUM_PARALLEL=1`. That can be worth the wall-time cost for long independent tasks, but not for latency-sensitive pipelines.
```

---

## Headless `Ask`→deny remediation

In headless (`-p`) mode, tools that require user confirmation (the default "Ask" rule) are auto-denied because there is no TTY to prompt on. If a headless run silently fails to write a file or run a command, this is the likely cause.

**Fix:** add an explicit `--allow` rule or switch to `--auto-allow` for unattended runs:

```bash
# Allow a specific tool pattern
caliban -p "..." --allow "Write:**"

# Allow all tool calls (use with care)
caliban -p "..." --auto-allow
```

See [Headless & Audit](./permissions/headless-and-audit.md) for the full headless permission model and how to configure durable rules.

---

## `--debug` file logging

Pass `--debug` (or set `CALIBAN_DEBUG=1`) to write a detailed event + render log to disk. This is useful when diagnosing silent failures, unexpected tool behaviour, or TUI rendering issues.

**Log file locations:**

| OS | Path |
|---|---|
| macOS | `~/Library/Caches/caliban/debug.log` |
| Linux / WSL | `~/.cache/caliban/debug.log` |

```admonish warning title="Debug log can be large"
The debug log grows quickly under active use. Delete or rotate it after capturing the relevant session. It contains full message content, tool inputs/outputs, and provider requests — do not share it if your prompts contain sensitive information.
```

The log appends across runs; it is not rotated automatically.
