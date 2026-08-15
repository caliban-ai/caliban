# Evaluation

Home for how we measure caliban — against real backends, against
competing agents, and (soon) against standard benchmarks.

## Layout

| Directory | Contents |
|-----------|----------|
| [`probes/`](probes/) | Dated, point-in-time findings from live probes of caliban against real backends (LMStudio, Ollama, parallel subagents, …). Each file is a snapshot; keep old ones for history. |
| [`competitors/`](competitors/) | Per-competitor capability inventories and parity analysis. One subdirectory per competitor, each with a documented-capability inventory + a caliban ↔ competitor parity gap matrix. Currently: [`claude-code/`](competitors/claude-code/) (primary parity target), [`codex/`](competitors/codex/) (OpenAI Codex CLI), [`grok-build/`](competitors/grok-build/) (Grok Build — xAI's terminal coding agent, direct head-to-head), and [`opencode/`](competitors/opencode/) (OpenCode — open-source terminal agent, direct head-to-head). [`pi/`](competitors/pi/) (Pi — a minimal terminal coding harness and caliban's closest architectural analogue) is compared on its head-to-head slice (`packages/coding-agent`); its broader **agent toolkit** (unified LLM API, agent loop, TUI library) is adjacent surface, noted but not scored as parity. [`antigravity/`](competitors/antigravity/) (Google Antigravity — an agent-first **IDE platform**) is compared on its head-to-head slice (agent engine + terminal CLI); its **Agent Manager** multi-agent dashboard is orchestration-layer surface (Prospero's category). **OpenClaw** — an orchestration-layer gateway, not a coding engine — is compared against **Prospero** instead; [`openclaw/`](competitors/openclaw/) here keeps only the caliban-as-worker-backend note. |

## Conventions

- **Probes** are timestamped in the filename (`YYYY-MM-DD-<subject>-probe-findings.md`)
  and are immutable snapshots — add a new file rather than editing an old one.
- **Competitors** each get their own directory under `competitors/<name>/`.
  Inventories are static, dated snapshots of a competitor's documented
  surface; re-baseline them manually before a parity-prioritization pass.

### Scoring rule for parity matrices

Three independent primary-source refreshes (#516 Claude Code, #517 Pi, #519
the sibling sweep) each arrived at the same rule after each found the same
failure mode — rows drift optimistic, because a capability gets ticked ✅ when
it is *designed, scaffolded, or unit-tested*, and nobody ticks it back down
when it turns out nothing calls it. Writing the rule down is what stops the
drift recurring:

> **A row is ✅ only when a production call path from the shipped binary
> reaches it.** Machinery that compiles and is unit-tested but has no
> non-test caller is **🟡 at most**. A capability with no user-reachable
> path at all is **🔴**, however complete the crate behind it is.

Corollaries, all of them things a real refresh has had to rule on:

- **`#[allow(dead_code)]` is a 🔴/🟡 signal, not a formality.** If the only
  callers of a function are its own `#[cfg(test)]` module, it is not shipped.
- **A stub that prints "lands with the &lt;X&gt; spec" is 🔴**, even when the
  command, flag, or overlay is registered and reachable.
- **A parsed-but-unread config key is not a feature.** A settings field with
  no reader, or a struct field hardwired empty at every production
  constructor, scores as if it were absent.
- **Design coverage is not implementation.** An accepted ADR or a spec in
  `docs/superpowers/` justifies nothing above 🔴 on its own.
- **Cite the evidence inline** — file path, ADR number, or PR/issue number —
  in the Notes column of every row you change. Anchor cross-matrix
  references by **section + row label, never line number**: these files get
  re-flowed on every refresh, so a line anchor is self-invalidating.
- **State your counting convention** when you report totals. The one in use:
  counts are capability-table rows in the lettered sections; roadmap and
  tier-audit tables are excluded; a combined row split into worse-scoring
  halves counts as a down-tick, and deleting a duplicate row is neither.

Prefer 🟡 at merge time over a ✅ that has to be undone. The matrices are the
prioritization input — an overstated row causes a real gap to be skipped in
sprint planning.

## Coming later

Standardized benchmark runs (e.g. SWE-bench Lite) and their result
summaries will land under this tree once we start capturing them. Exact
structure is deliberately left open until then; tracked separately.
