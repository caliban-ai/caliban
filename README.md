# caliban

A from-scratch Rust agent harness — a replacement for Claude Code that puts
the operator in control of model routing, memory, skills, and prompt context.

> **Project status.** Daily-usable on `main`. The core agent loop, persistent
> sessions, ratatui TUI, headless `--print` mode, sub-agents, MCP, sandbox,
> permissions, checkpoints, auto-memory, image input, and a multi-tier
> settings system are all shipped. Many small parity gaps with Claude Code
> remain — see [`docs/parity-gap-matrix.md`](docs/parity-gap-matrix.md) for
> the scoreboard and [`docs/TODO.md`](docs/TODO.md) for the actionable
> backlog. Private repo, designed to be open-sourced.

## Why

- **Provider-agnostic.** No SDK lock-in. Anthropic Claude (direct, AWS
  Bedrock, Google Vertex), OpenAI (direct, Azure), Google Gemini (AI
  Studio, Vertex), and local Ollama all speak the same internal IR.
- **Operator control.** You decide what model handles what task, what
  context goes into the prompt, and where memory lives. Routing is
  declarative; settings layer at four scopes; permissions and hooks are
  first-class.
- **Data sovereignty.** Local-first by default. Sessions, checkpoints,
  auto-memory, and tool overflows live on your disk. Designed to slot
  into a self-hosted homelab.
- **Rust-fast.** Harness overhead should be negligible compared to model
  latency. The user's time-to-result is dominated by the model, not the
  runtime.

## License

caliban is licensed under [AGPL-3.0-only](LICENSE). In short: if you
modify caliban and either distribute the binary or run it as a network
service, you must release your changes under AGPL-3.0. Personal use is
unaffected. See the [license ADR](adrs/0003-license-agpl-3.0.md) for
the reasoning.

## Building

Requires the toolchain pinned in `rust-toolchain.toml` (currently Rust
1.95.0). `rustup` installs it automatically on first `cargo` invocation.

```bash
cargo build --workspace                        # build everything
cargo test  --workspace                        # run all tests
cargo run   --bin caliban -- --version         # smoke-test the binary
cargo build --release --bin caliban            # release binary at target/release/caliban
```

For diagnosing TUI issues, run with `--debug` (or set `CALIBAN_DEBUG=1`).
caliban appends events and draws to a file under the platform's cache
directory (e.g. `~/.cache/caliban/debug.log` on Linux,
`~/Library/Caches/caliban/debug.log` on macOS).

## Quick start

### One-shot prompt

```bash
ANTHROPIC_API_KEY=$KEY caliban -p "Summarize README.md"
```

The `-p` / `--print` flag runs headlessly: prompt in, structured stdout
out, exit. Add `--output-format stream-json` for machine-readable
streaming frames; see ADR 0025.

### Persistent sessions

```bash
# First invocation — creates session "research"
caliban --session research "Read README.md"

# Subsequent invocations — conversation continues
caliban --session research "Now look at Cargo.toml"

# Resume the last session interactively
caliban --continue

# Resume a specific session by name
caliban --resume research

# One-off run without saving back to the session
caliban --session research --no-save "what was the first thing I asked?"
```

Sessions are saved as pretty-printed JSON under the per-OS session
directory (override with `--sessions-dir`):

- **Linux:** `$XDG_DATA_HOME/caliban/sessions/<name>.json`
  (default `~/.local/share/caliban/sessions/<name>.json`)
- **macOS:** `~/Library/Application Support/caliban/sessions/<name>.json`
- **Windows:** `%LOCALAPPDATA%\caliban\sessions\<name>.json`

### Interactive TUI

Invoke `caliban` with no prompt + a TTY stdin to enter the TUI:

```
┌────────────────────────────────────────────────────────────────┐
│ user: What's in README.md?                                     │
│                                                                │
│ 🔧 Read({"path":"README.md"})                                  │
│    → → Read README.md, lines 1-83 of 83                        │
│                                                                │
│ assistant: It's a Rust agent harness…                          │
│                                                                │
│ [caliban: 2 turns · 132↑ 48↓ tokens]                           │
├────────────────────────────────────────────────────────────────┤
│ > █                                                            │
├────────────────────────────────────────────────────────────────┤
│ ~/dev/personal/caliban · openai gpt-4o · session: research     │
└────────────────────────────────────────────────────────────────┘
```

The input bar supports multi-line composition (Shift+Enter on terminals
that speak the kitty keyboard protocol — kitty, iTerm2, Ghostty, foot,
WezTerm — and Alt+Enter as a portable fallback). Typing `/` opens a
fuzzy menu of slash commands; typing `@` opens a live file picker
scoped to the directory implied by the typed prefix (workspace-relative,
absolute, `~/`, or `../`). On submit each `@<path>` is read from disk
and inlined into the outgoing message as a `--- attached: ... ---`
block, so the model sees the content without a separate Read tool
round-trip. The transcript shows a single 📎 line per attachment.

Files over `--max-attach-bytes` (default 256 KB) or that exceed the
per-message `--attach-budget-bytes` (default 1 MB) cause an inline
error and abort the send; both flags also honor
`CALIBAN_MAX_ATTACH_BYTES` and `CALIBAN_ATTACH_BUDGET_BYTES`.

Ctrl-C during a turn cancels it. Ctrl-C or Ctrl-D at an empty prompt
exits cleanly.

### Library use

```rust
use std::sync::Arc;

use caliban_agent_core::{Agent, ToolRegistry, Session};
use caliban_provider::Provider;
use caliban_provider_anthropic::{config::DirectConfig, AnthropicProvider};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let provider: Arc<dyn Provider + Send + Sync> = Arc::new(
        AnthropicProvider::direct(DirectConfig::from_env()?)?,
    );

    let agent = Arc::new(Agent::builder()
        .provider(provider)
        .tools(ToolRegistry::new())  // see caliban-tools-builtin for real tools
        .model("claude-3-5-sonnet")
        .max_tokens(1024)
        .build()?);

    let mut session = Session::new(agent);
    session.system("You are helpful.").user_text("Hello!");
    for m in session.run().await? { println!("{m:?}"); }
    Ok(())
}
```

Set `ANTHROPIC_API_KEY` before running. Each provider crate has its own
`config::DirectConfig` and similar adapters for cloud transports.

## Provider matrix

| Schema family | Direct | AWS Bedrock | Google Vertex | Azure |
|---|---|---|---|---|
| Anthropic Claude | ✅ default | ✅ `bedrock` feature | ✅ `vertex` feature | — |
| OpenAI | ✅ default | — | — | ✅ `azure` feature |
| Gemini | ✅ default (AI Studio) | — | ✅ `vertex` feature | — |
| Ollama (native `/api/chat`, local) | ✅ default | — | — | — |

Cargo feature flags gate cloud transports per-crate. To enable
Bedrock-Claude + Vertex-Gemini + Azure-OpenAI in one build:

```bash
cargo build --features \
  caliban-provider-anthropic/bedrock,caliban-provider-google/vertex,caliban-provider-openai/azure
```

The `caliban-model-router` crate (ADR 0038) layers declarative routing
on top: load `caliban.toml` with `--config`, define purposes
(`MainLoop`, `Compaction`, …), assign per-purpose model preferences and
fallbacks, and let the router pick a route per request based on
capability filters (vision, tool-use, thinking) and breaker state.
`caliban router debug --help` shows what the router would resolve for a
synthetic request.

## Major features

| Area | Status | Where |
|---|---|---|
| Provider abstraction + IR | ✅ | [ADR 0006](adrs/0006-message-schema-ir.md), [ADR 0007](adrs/0007-transport-trait-pattern.md) |
| Agent loop (stream-as-primitive, parallel tool dispatch) | ✅ | [ADR 0009](adrs/0009-agent-core-design.md), [ADR 0016](adrs/0016-parallel-tool-dispatch.md) |
| Built-in tools (Read/Write/Edit/MultiEdit/NotebookEdit/Bash/BashBg/Glob/Grep/WebFetch/WebSearch/AgentTool/TodoWrite/Plan/Memory) | ✅ | `crates/caliban-tools-builtin/` |
| Persistent sessions + REPL + TUI | ✅ | [ADR 0011](adrs/0011-sessions-and-repl.md), [ADR 0012](adrs/0012-tui-via-ratatui.md), [ADR 0027](adrs/0027-tui-ergonomics.md) |
| Headless mode (`-p`, `stream-json` I/O) | ✅ | [ADR 0025](adrs/0025-headless-output-protocol.md) |
| Sub-agents (parallel + background, isolated worktrees) | ✅ | [ADR 0021](adrs/0021-sub-agent-primitive.md), [ADR 0037](adrs/0037-subagent-isolation-and-background-fleet.md) |
| MCP client (stdio + HTTP transports, OAuth, elicitation) | ✅ | [ADR 0017](adrs/0017-mcp-client-architecture.md), [ADR 0023](adrs/0023-mcp-v2-transports-and-oauth.md) |
| Memory tiers + auto-memory + CLAUDE.md ancestry + @-imports | ✅ | [ADR 0018](adrs/0018-memory-tier-model.md), [ADR 0035](adrs/0035-auto-memory.md), [ADR 0036](adrs/0036-claudemd-ancestry-and-imports.md) |
| Skills loading | ✅ | [ADR 0019](adrs/0019-skills-loading.md) |
| Permission modes (Default/AcceptEdits/Plan/Auto/DontAsk/Bypass) + rules | ✅ | [ADR 0020](adrs/0020-permission-rules.md), [ADR 0029](adrs/0029-permission-modes-and-auto-mode.md) |
| Hooks (extensible event taxonomy) | ✅ | [ADR 0024](adrs/0024-hook-event-taxonomy.md) |
| Settings layering (Managed > User > Project > Local, deep-merge, live reload) | ✅ | [ADR 0026](adrs/0026-settings-layering.md) |
| Checkpoint store + `/rewind` | ✅ | [ADR 0028](adrs/0028-checkpointing-rewind.md) |
| Plugin packaging | ✅ | [ADR 0030](adrs/0030-plugin-packaging.md) |
| Output styles | ✅ | [ADR 0031](adrs/0031-output-styles.md) |
| OS sandbox (Seatbelt on macOS, bubblewrap on Linux) | ✅ | [ADR 0032](adrs/0032-os-sandbox.md), `crates/caliban-sandbox/README.md` |
| OpenTelemetry + per-request cost ledger | ✅ | [ADR 0033](adrs/0033-opentelemetry-and-cost.md) |
| Image / vision input | ✅ | [ADR 0039](adrs/0039-image-and-vision-input.md) |
| Slash command registry | ✅ | [ADR 0040](adrs/0040-slash-command-registry.md) |
| Model router v2 (declarative routes, capability filters) | ✅ | [ADR 0038](adrs/0038-model-router-v2.md) |
| Health-check `caliban doctor` / `/doctor` | ✅ | `caliban/src/diagnostics.rs` |
| Cost surfacing in TUI / `/cost` slash | 🟡 backlog | [`docs/TODO.md`](docs/TODO.md) |
| Stream-idle watchdog, MaxTokens recovery, reactive compaction | 🟡 backlog | [`docs/TODO.md`](docs/TODO.md) |

`✅` = shipped on `main`. `🟡` = identified, scoped, not yet built — see
the linked TODO entry for the exact file and suggested fix.

## Slash commands

Type `/` in the TUI for a live typeahead menu, or `/help` for the full
list with descriptions. The registry currently includes (non-exhaustive):

- **Session / transcript:** `/clear`, `/init`, `/resume`, `/recap`,
  `/export`, `/system`, `/compact`
- **Status / introspection:** `/help`, `/usage`, `/context`, `/cost`,
  `/status`, `/doctor`
- **Models / inference:** `/model`, `/effort`
- **Modes & permissions:** `/plan`, `/permissions`
- **Auth:** `/login`, `/logout`
- **Subsystems:** `/memory`, `/skills`, `/mcp`, `/hooks`, `/agents`,
  `/plugins`, `/output-style`
- **DX:** `/rewind`, `/loop`, `/statusline`, `/feedback`, `/btw`,
  `/heapdump`, `/voice`, `/tui`
- **Exit:** `/quit`, `/exit`

Some entries (e.g. `/cost`, `/effort`, `/resume`) are tracked as
backlog work in [`docs/TODO.md`](docs/TODO.md) — check there before
assuming a slash listed here is fully implemented vs. registered as a
stub.

## Subcommands

```bash
caliban doctor [--deep]            # health checks; --deep adds provider auth pings
caliban config print               # print merged effective settings as JSON
caliban config migrate [--dry-run] # roll up legacy {permissions,mcp,hooks}.toml into settings.json
caliban router debug ...           # show candidate routes + breaker + effort table
caliban agents list                # list background sub-agents (ADR 0037)
caliban agents attach <id>         # stream a running agent's transcript live
caliban agents spawn --prompt ...  # start a new background agent
caliban daemon status              # supervisor daemon health
caliban plugin <verb> [args]       # plugin package management (list/info/install/...)
caliban --bg "<task>"              # spawn a background agent and return immediately
```

`caliban --help` is the source of truth — the flag surface is large
(`-p`, `--session`, `--continue`, `--resume`, `--provider`, `--model`,
`--config`, `--settings`, `--allow/--deny/--ask`, `--permission-mode`,
`--system/--system-file/--no-system`, `--bg`, `--bare`,
`--output-format`, `--input-format`, `--no-skills`, `--no-mcp`,
`--no-plugins`, `--no-sub-agent`, `--no-hooks`, and more).

## Configuration

caliban reads `settings.json` (or `.toml`) at four scopes — **Managed >
User > Project > Local** — with deep-merge semantics for nested
objects and array-concat for permission arrays. Live reload via
`notify` picks up edits without restarting. See
[ADR 0026](adrs/0026-settings-layering.md) for the layering rules and
[`docs/examples/permissions.example.toml`](docs/examples/permissions.example.toml)
and [`docs/examples/hooks.example.toml`](docs/examples/hooks.example.toml)
for example fragments. Legacy per-feature TOMLs (`mcp.toml`,
`permissions.toml`, `hooks.toml`) still load via the `compat::maybe_load_legacy_*`
shims; migrate them with `caliban config migrate`.

## Known model limitations

### Qwen3 reasoning models on LM Studio — long tool chains break (verified 2026-05-27)

**Affected models.** Qwen3-family *reasoning* variants when served via
LM Studio: e.g. `qwen3.5-9b-mlx`, `qwen3-72b-mlx`, and similar Qwen3
reasoning-mode builds. Non-reasoning Qwen variants are not affected.

**What works.** Two-step tool chains (e.g. `Glob` → `Read` → final
answer) run end-to-end. Reasoning content arrives in the OpenAI
`reasoning_content` delta and is preserved as `thinking` blocks in
the assistant message.

**What breaks.** Once a chain reaches a third tool dispatch, the
model serializes the next tool call as Qwen-native `<tool_call>` XML
**inside its reasoning channel** (the OpenAI `reasoning_content`
delta, surfaced as a `thinking` block in caliban) instead of populating
the OpenAI `tool_calls` array:

```
<tool_call>
<function=Read>
<parameter=path>
Cargo.toml
</parameter>
</function>
</tool_call>
```

caliban's OpenAI-spec parser sees a thinking block with no
`tool_calls` field and a `finish_reason: "stop"`, so the turn ends
without dispatching anything. The user sees a stalled run that
exits cleanly with no apparent error.

**Why caliban doesn't fix this.** LM Studio passes the model's
reasoning output through verbatim and does not rewrite Qwen-native
`<tool_call>` XML into the OpenAI `tool_calls` array. Building and
maintaining a reasoning-channel XML scanner inside caliban's OpenAI
provider would add parsing complexity and ongoing maintenance for
every Qwen template variation. We've elected to keep the bug visible
and document the limitation instead.

**Workarounds (any one of):**

1. **Keep tool chains short** (≤ 2 dispatches per turn). Restructure
   prompts so each turn only requires a Glob+Read or Read+Edit pair,
   not a three-step plan.
2. **Configure LM Studio's per-model tool-call adapter** to emit
   OpenAI-style `tool_calls`. LM Studio exposes a per-model JSON
   config for tool-call normalization; if your build supports
   rewriting Qwen-native tool calls into the OpenAI shape, enable it
   for the affected model.
3. **Use a non-reasoning Qwen variant** (e.g. a plain `qwen3-*`
   instruct build without reasoning mode) served via LM Studio.
4. **Use a hosted provider** — Anthropic, OpenAI, or Google — where
   tool-call schemas are normalized server-side.

## Repository layout

```
caliban/             # the user-facing binary
crates/              # 24 library crates, grouped below
adrs/                # architecture decision records (0001–0044)
docs/                # design specs, parity matrix, capability inventory
docs/superpowers/    # active design specs + implementation plans
docs/examples/       # sample settings / permission / hook fragments
.github/workflows/   # CI
```

The 24 library crates, grouped by purpose:

| Group | Crates |
|---|---|
| **Foundation** | `caliban-common` (fs/paths/glob/http/expand helpers) |
| **Providers** | `caliban-provider` (trait + IR), `caliban-provider-anthropic`, `caliban-provider-openai`, `caliban-provider-google`, `caliban-provider-ollama`, `caliban-provider-bedrock`, `caliban-provider-vertex` |
| **Agent core** | `caliban-agent-core` (loop, hooks, compaction, cache markers), `caliban-tools-builtin` |
| **Sessions & state** | `caliban-sessions`, `caliban-checkpoint`, `caliban-memory`, `caliban-output-styles` |
| **Routing** | `caliban-model-router` |
| **Integration** | `caliban-mcp-client`, `caliban-images`, `caliban-skills`, `caliban-plugins` |
| **Infrastructure** | `caliban-settings`, `caliban-sandbox`, `caliban-telemetry`, `caliban-supervisor` (sub-agent fleet), `caliban-worktrees` |

## Adding a new crate

**Library:**
```bash
cargo new --lib crates/caliban-<name>
# then add "crates/caliban-<name>" to workspace.members in the root Cargo.toml
```

**Binary:**
```bash
cargo new caliban-<name>
# then add "caliban-<name>" to workspace.members
```

Both inherit the workspace's package metadata, dependencies, and lints
via `*.workspace = true`. See an existing crate's `Cargo.toml` for the
boilerplate.

## Parity with Claude Code

caliban tracks parity against Claude Code in two living documents:

- [`docs/parity-gap-matrix.md`](docs/parity-gap-matrix.md) — checklist
  of capabilities, marked ✅ / 🟡 / 🔴, grouped into themes (A–N) with
  tier ordering at the bottom. Consult before prioritizing the next
  feature; tick rows in the same PR that closes a gap.
- [`docs/claude-code-capability-inventory.md`](docs/claude-code-capability-inventory.md)
  — static snapshot of Claude Code's documented surface, captured from
  `docs.claude.com`. Re-baselined manually before each parity review.

Concrete actionable items (small enough to skip a full design spec but
specific enough to act on) live in [`docs/TODO.md`](docs/TODO.md).

## Architecture decisions

Browse [`adrs/`](adrs/) for all 44 ADRs (0001–0044). Highlights by layer:

- **Foundation (0001–0008):** tokio runtime, error model
  (thiserror libs / anyhow binary), AGPL-3.0, naming, workspace
  layout, message-schema IR, transport trait pattern.
- **Agent + tools (0009–0016):** stream-as-primitive agent core,
  sessions, ratatui TUI, parallel tool dispatch.
- **MCP + memory + skills + permissions (0017–0020, 0023):** MCP v1
  + v2 (HTTP transport, OAuth), memory tier model, skills loader,
  permission rules.
- **Sub-agents + settings + sandbox + telemetry (0021, 0026, 0029,
  0032–0033, 0037):** sub-agent primitive, settings layering, OS
  sandbox, OpenTelemetry + cost, sub-agent isolation + background
  fleet.
- **Cloud providers + headless + checkpoints + auto-memory + image
  + slash + router (0025, 0028, 0034–0036, 0038–0040):**
  Bedrock + Vertex, headless I/O protocol, checkpoint/rewind, auto-memory,
  CLAUDE.md ancestry + imports, model router v2, image + vision input,
  slash command registry.
- **Recent (0041–0044):** TUI redraw tick closeout, caliband binary
  placement, arc-swap shared state, rmcp version pin.

## Design specs

Active design + implementation plans live under
[`docs/superpowers/`](docs/superpowers/) (specs and plans pair 1:1 by
date and feature name). See also:

- [Layer 0 · Workspace & ADRs](docs/superpowers/specs/2026-05-22-layer-0-bootstrap-design.md)
- [Layer 1 · Provider Abstraction](docs/superpowers/specs/2026-05-22-layer-1-provider-design.md)
