# caliban

A from-scratch Rust agent harness — a replacement for Claude Code that puts
the operator in control of model routing, memory, skills, and prompt context.

> **Project status:** Layer 1 complete (provider + agent-core + tools-builtin)
> + Layer 4 CLI + sessions + ratatui-based TUI. Private repo, designed to be
> open-sourced. Daily-usable: single-prompt mode, persistent sessions, or
> interactive TUI with status bar + dedicated input area. Memory architecture,
> MCP client, model-router, and additional UX surfaces remain as future
> sub-projects.

## Why

- **Provider-agnostic.** No SDK lock-in. Talk to Anthropic, OpenAI,
  local Ollama, or anything else, swapping providers per task.
- **Operator control.** You decide what model handles what task, what
  context goes into the prompt, and where memory lives.
- **Data sovereignty.** Local-first by default. Designed to integrate
  with self-hosted homelab components.
- **Rust-fast.** The harness overhead should be negligible compared to
  model latency. The user's time-to-result is dominated by the model,
  not the runtime.

## License

caliban is licensed under [AGPL-3.0-only](LICENSE). In short: if you
modify caliban and either distribute the binary or run it as a network
service, you must release your changes under AGPL-3.0. Personal use is
unaffected. Read the [license ADR](adrs/0003-license-agpl-3.0.md) for
the reasoning.

## Building

Requires the toolchain pinned in `rust-toolchain.toml` (currently Rust
1.95.0). `rustup` will install it automatically on first `cargo`
invocation.

```bash
cargo build --workspace             # build everything
cargo test  --workspace             # run all tests
cargo run   --bin caliban -- --version    # smoke-test the binary
```

For diagnosing TUI issues, run with `--debug` (or set `CALIBAN_DEBUG=1`).
caliban will append-log events and draws to a file under the platform's
cache directory (e.g. `~/.cache/caliban/debug.log` on Linux,
`~/Library/Caches/caliban/debug.log` on macOS).

## Example usage (library)

```rust
use caliban_provider::{CompletionRequest, Provider};
use caliban_provider_anthropic::{config::DirectConfig, AnthropicProvider};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cfg = DirectConfig::from_env()?;
    let provider = AnthropicProvider::direct(cfg)?;
    let req = CompletionRequest::builder("claude-3-5-sonnet")
        .system("You are helpful.")
        .user_text("What is the airspeed velocity of an unladen swallow?")
        .max_tokens(256)
        .build()?;
    let resp = provider.complete(req).await?;
    println!("{:?}", resp.message);
    Ok(())
}
```

(Set `ANTHROPIC_API_KEY` before running.)

## Example usage (library, with caliban-agent-core)

```rust
use std::sync::Arc;

use caliban_agent_core::{Agent, ToolRegistry, Session};
use caliban_provider::Provider;
use caliban_provider_anthropic::{config::DirectConfig, AnthropicProvider};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let provider: Arc<dyn Provider + Send + Sync> = Arc::new(
        AnthropicProvider::direct(DirectConfig::from_env()?)?,
    );

    let agent = Arc::new(Agent::builder()
        .provider(provider)
        .tools(ToolRegistry::new())  // See caliban-tools-builtin for real tool implementations
        .model("claude-3-5-sonnet")
        .max_tokens(1024)
        .build()?);

    let mut session = Session::new(agent);
    session.system("You are helpful.").user_text("Hello!");
    let new_msgs = session.run().await?;
    for m in new_msgs { println!("{m:?}"); }
    Ok(())
}
```

## Sessions and interactive TUI

caliban persists conversations across invocations and provides an interactive
TUI for iterative work.

### Persistent single-prompt sessions

```bash
# First invocation — creates session "research"
ANTHROPIC_API_KEY=$KEY caliban --session research "Read README.md"

# Subsequent invocations — conversation continues
ANTHROPIC_API_KEY=$KEY caliban --session research "Now look at Cargo.toml"

# One-off run without saving back to the session
caliban --session research --no-save "what was the first thing I asked?"
```

Sessions are saved as pretty-printed JSON under
`~/.local/share/caliban/sessions/<name>.json` (override with
`--sessions-dir`).

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
│ ~/dev/personal/iron-orrery · openai gpt-4o · session: research │
└────────────────────────────────────────────────────────────────┘
```

By default, caliban prepends a system prompt describing itself, the cwd, and
the registered tools so the model knows it's running in caliban. Override
with `--system "<text>"`, load from `--system-file <PATH>`, or disable with
`--no-system`. View the active prompt via `/system` in the TUI.

The REPL preserves message history across turns, even without `--session`.
Use `--session <name>` to persist that history to disk between invocations.

Type `/help`, `/config`, `/mcp`, or `/skills` to open a sub-menu overlay
showing slash command reference, active configuration, planned MCP
server config, or planned skills config respectively. `Esc` or `q`
closes any overlay.

Slash commands (typed at the prompt and submitted with Enter):
`/help`, `/exit`, `/quit`, `/clear` (clear transcript and in-memory history; also clears session if active), `/sessions`, `/save [<name>]`, `/usage`, `/config`, `/mcp`, `/skills`, `/system`.

The input area supports multi-line composition (Shift+Enter on terminals
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
error and abort the send; both flags also honor `CALIBAN_MAX_ATTACH_BYTES`
and `CALIBAN_ATTACH_BUDGET_BYTES` env vars.

Ctrl-C during a turn cancels it and returns to the prompt. Ctrl-C or
Ctrl-D at an empty prompt exits cleanly.

## Provider matrix

| Schema family | Direct | AWS Bedrock | Google Vertex | Azure |
|---|---|---|---|---|
| Anthropic Claude | ✅ default | ✅ `bedrock` feature | ✅ `vertex` feature | — |
| OpenAI | ✅ default | — | — | ✅ `azure` feature |
| Gemini | ✅ default (AI Studio) | — | ✅ `vertex` feature | — |
| Ollama (OpenAI-compat, local) | ✅ default | — | — | — |

Cargo feature flags gate cloud transports per-crate. To enable Bedrock-Claude + Vertex-Gemini + Azure-OpenAI:

```bash
cargo build --features caliban-provider-anthropic/bedrock,caliban-provider-google/vertex,caliban-provider-openai/azure
```

## Repository layout

```
caliban/             # the user-facing binary
crates/              # libraries
  caliban-core/                # foundational types
  caliban-provider/            # provider trait + IR
  caliban-provider-anthropic/  # Claude (direct + Bedrock + Vertex)
  caliban-provider-openai/     # OpenAI (direct + Azure)
  caliban-provider-ollama/     # Ollama (direct)
  caliban-provider-google/     # Gemini (AI Studio + Vertex)
  caliban-agent-core/          # agent loop, tools, session
  caliban-tools-builtin/       # built-in tools (Read/Write/Edit/Bash/Glob/Grep)
  caliban-sessions/            # session persistence + REPL
adrs/                # architecture decision records
docs/superpowers/    # design specs and implementation plans
.github/workflows/   # CI
```

## Adding a new crate

**Library:**
```bash
cargo new --lib crates/caliban-<name>
# then add "crates/caliban-<name>" to the workspace.members list in
# the root Cargo.toml
```

**Binary:**
```bash
cargo new caliban-<name>
# then add "caliban-<name>" to the workspace.members list
```

Both inherit the workspace's package metadata, dependencies, and lints
via `*.workspace = true`. See an existing crate's `Cargo.toml` for the
boilerplate.

## Architecture decisions

See [`adrs/`](adrs/). Notable decisions:

**Layer 0:**
- [Async runtime: tokio](adrs/0001-async-runtime.md)
- [Error model: thiserror libs, anyhow binary](adrs/0002-error-model.md)
- [License: AGPL-3.0](adrs/0003-license-agpl-3.0.md)
- [Naming conventions](adrs/0004-naming-conventions.md)
- [Workspace layout](adrs/0005-workspace-layout.md)
- [Message schema (IR)](adrs/0006-message-schema-ir.md)
- [Transport trait pattern](adrs/0007-transport-trait-pattern.md)
- [System role positional](adrs/0008-system-role-positional.md)

**Layer 1 / C:**
- [Agent-core design (stream-as-primitive, sequential tools, opt-in compaction)](adrs/0009-agent-core-design.md)

## Design specs

- [Layer 0 · Workspace & ADRs](docs/superpowers/specs/2026-05-22-layer-0-bootstrap-design.md)
- [Layer 1 · Provider Abstraction](docs/superpowers/specs/2026-05-22-layer-1-provider-design.md)
