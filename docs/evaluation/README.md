# Evaluation

Home for how we measure caliban — against real backends, against
competing agents, and (soon) against standard benchmarks.

## Layout

| Directory | Contents |
|-----------|----------|
| [`probes/`](probes/) | Dated, point-in-time findings from live probes of caliban against real backends (LMStudio, Ollama, parallel subagents, …). Each file is a snapshot; keep old ones for history. |
| [`competitors/`](competitors/) | Per-competitor capability inventories and parity analysis. One subdirectory per competitor, each with a documented-capability inventory + a caliban ↔ competitor parity gap matrix. Currently: [`claude-code/`](competitors/claude-code/) (primary parity target), [`codex/`](competitors/codex/) (OpenAI Codex CLI), [`grok-build/`](competitors/grok-build/) (Grok Build — xAI's terminal coding agent, direct head-to-head), and [`opencode/`](competitors/opencode/) (OpenCode — open-source terminal agent, direct head-to-head). [`antigravity/`](competitors/antigravity/) (Google Antigravity — an agent-first **IDE platform**) is compared on its head-to-head slice (agent engine + terminal CLI); its **Agent Manager** multi-agent dashboard is orchestration-layer surface (Prospero's category). **OpenClaw** — an orchestration-layer gateway, not a coding engine — is compared against **Prospero** instead; [`openclaw/`](competitors/openclaw/) here keeps only the caliban-as-worker-backend note. |

## Conventions

- **Probes** are timestamped in the filename (`YYYY-MM-DD-<subject>-probe-findings.md`)
  and are immutable snapshots — add a new file rather than editing an old one.
- **Competitors** each get their own directory under `competitors/<name>/`.
  Inventories are static, dated snapshots of a competitor's documented
  surface; re-baseline them manually before a parity-prioritization pass.

## Coming later

Standardized benchmark runs (e.g. SWE-bench Lite) and their result
summaries will land under this tree once we start capturing them. Exact
structure is deliberately left open until then; tracked separately.
