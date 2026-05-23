# Layer 4 · CLI · Design

- **Date:** 2026-05-23
- **Status:** Draft (pending implementation plan)
- **Sub-project of:** caliban Rust agent harness
- **Depends on:** Layer 1 / B (provider), C (agent-core), D (tools-builtin)
- **Next sub-project:** Layer 2 (memory) or TUI

## Goals

Transform the `caliban/` binary crate from a `--version`-only stub into a runnable CLI that drives the agent end-to-end. Acceptance: a user with `ANTHROPIC_API_KEY` exported can run:

```bash
caliban "Read README.md and summarize it"
```

…and watch the model invoke the `Read` tool, see the file content streamed back, and get a useful summary. `Ctrl+C` cancels in-flight requests cleanly.

## Non-goals

- Interactive REPL — deferred. Each invocation is a single-prompt run-to-completion.
- Multi-message history persistence (`~/.cache/caliban/sessions/`) — deferred.
- Streaming output rendering with cursor/markdown polish — basic write-text-as-it-arrives is fine.
- Interactive tool approval prompts — auto-approve for MVP; `--ask` flag is a follow-up.
- Config file (`~/.config/caliban/config.toml`) — env vars + CLI flags only.
- Image input — deferred. Text only for MVP.

## CLI surface

```
caliban [OPTIONS] <PROMPT>

Arguments:
  <PROMPT>    The user prompt to send. Can also be read from stdin if PROMPT is "-".

Options:
  -p, --prompt <PROMPT>          Alternative way to specify the prompt
      --provider <PROVIDER>      anthropic | openai | ollama | google [default: anthropic]
      --model <MODEL>            Model name (defaults to a provider-specific recommendation)
      --max-tokens <N>           Per-turn output cap [default: 2048]
      --max-turns <N>            Maximum agent loop iterations [default: 50]
      --temperature <FLOAT>      Sampling temperature
      --workspace <DIR>          Workspace root for file/shell tools [default: cwd]
      --no-tools                 Disable all tools (chat-only mode)
      --restrict-paths           Reject paths outside the workspace root
      --quiet                    Suppress tool execution announcements
  -V, --version                  Print version and exit
  -h, --help                     Print help
```

**Provider/model defaults:**
- anthropic → `claude-3-5-sonnet`
- openai → `gpt-4o`
- ollama → `llama3.1`
- google → `gemini-2.0-flash`

**Auth resolution:** uses the relevant provider's `Config::from_env()`. Errors with a clear message if the expected env var is missing.

## Output rendering

Each `TurnEvent` from `Agent::stream_until_done` translates to terminal output:

- `TurnStart`: prints nothing (the first text delta is the visible signal).
- `AssistantTextDelta`: writes the text directly to stdout (flushed). User sees text appearing in real time.
- `AssistantThinkingDelta`: writes to stderr in gray (if a TTY) so it's visually separable from the main output. Skipped if `--quiet`.
- `ToolCallStart`: prints to stderr `🔧 {tool_name}(...) ` (no newline yet). Skipped if `--quiet`.
- `ToolCallInputDelta`: ignored — the input JSON gets dumped at ToolCallStart time once it's complete (we accumulate the partial JSON deltas and decode at end).
- `ToolCallEnd`: prints to stderr `→ {first 80 chars of result, single-lined}`, then newline. If `is_error: true`, prefix with `(error) `. Skipped if `--quiet`.
- `TurnEnd`: prints a newline if the last printed thing wasn't already at column 0.
- `RunEnd`: prints a usage summary to stderr — `[caliban: {turn_count} turn{s} · {input_tokens}↑ {output_tokens}↓ tokens]`. Skipped if `--quiet`.

If the assistant output ends mid-line, a final newline is appended.

For non-TTY stdout (piping to a file or another tool), strip the color codes and tool-call notices (they're on stderr by default, so the main stdout stays clean text).

## Cancellation

`tokio::signal::ctrl_c().await` triggers cancellation. First `Ctrl+C` cancels the running token (gracefully stops the loop); a second one within 500ms forces process exit with code 130.

## Crate changes

Modify the existing `caliban/` binary crate:

- `caliban/Cargo.toml` — add deps: `clap` (4.x, with `derive` feature), `caliban-provider`, `caliban-provider-anthropic`, `caliban-provider-openai`, `caliban-provider-ollama`, `caliban-provider-google`, `caliban-agent-core`, `caliban-tools-builtin`, `tokio-util`, `anyhow`. Drop the unused-anyhow note from Layer 0 — it's used now.
- `caliban/src/main.rs` — full CLI entry point with clap derive + agent setup + event-loop renderer.

Optionally split into modules:
- `caliban/src/main.rs` — entry point + arg parsing
- `caliban/src/render.rs` — TurnEvent → stdout/stderr renderer
- `caliban/src/setup.rs` — provider + agent construction from args

For MVP just keep it in one `main.rs` if it stays under ~300 lines; otherwise factor.

## Implementation notes

### Provider construction

Each provider crate has a `Config::from_env()` + `XxxProvider::direct(cfg)` constructor. The CLI dispatches on the `--provider` flag:

```rust
let provider: Arc<dyn Provider + Send + Sync> = match args.provider {
    Provider::Anthropic => Arc::new(AnthropicProvider::direct(DirectConfig::from_env()?)?),
    Provider::OpenAI    => Arc::new(OpenAIProvider::direct(DirectConfig::from_env()?)?),
    Provider::Ollama    => Arc::new(OllamaProvider::local()?),
    Provider::Google    => Arc::new(GoogleProvider::ai_studio(AIStudioConfig::from_env()?)?),
};
```

Note: each provider's `DirectConfig` lives in `caliban_provider_xxx::config::DirectConfig`. The CLI imports them under aliases to avoid ambiguity.

### Tool registry

```rust
let registry = if args.no_tools {
    ToolRegistry::new()
} else {
    let mut r = ToolRegistry::new();
    let root = if args.restrict_paths { workspace.restricted() } else { workspace };
    r.register(Arc::new(ReadTool::new(root.clone())));
    r.register(Arc::new(WriteTool::new(root.clone())));
    r.register(Arc::new(EditTool::new(root.clone())));
    r.register(Arc::new(BashTool::new(root.clone())));
    r.register(Arc::new(GlobTool::new(root.clone())));
    r.register(Arc::new(GrepTool::new(root)));
    r
};
```

### Agent

```rust
let agent = Arc::new(Agent::builder()
    .provider(provider)
    .tools(registry)
    .model(args.model.unwrap_or_else(|| default_model_for(args.provider).to_string()))
    .max_tokens(args.max_tokens)
    .max_turns(args.max_turns)
    .build()?);
```

### Event loop

```rust
let mut stream = Arc::clone(&agent).stream_until_done(messages, cancel);
let mut tool_input_buffers: HashMap<String, String> = HashMap::new();
while let Some(event) = stream.next().await {
    match event? {
        TurnEvent::AssistantTextDelta { text, .. } => {
            print!("{text}");
            std::io::stdout().flush().ok();
        }
        TurnEvent::AssistantThinkingDelta { text, .. } if !args.quiet => {
            eprint!("\x1b[2m{text}\x1b[0m");  // dim if TTY
        }
        TurnEvent::ToolCallStart { tool_use_id, name, .. } if !args.quiet => {
            tool_input_buffers.insert(tool_use_id.clone(), String::new());
            eprint!("\n🔧 {name}(");
        }
        TurnEvent::ToolCallInputDelta { tool_use_id, partial_json, .. } => {
            tool_input_buffers.entry(tool_use_id).or_default().push_str(&partial_json);
        }
        TurnEvent::ToolCallEnd { tool_use_id, is_error, content, .. } if !args.quiet => {
            let input_str = tool_input_buffers.remove(&tool_use_id).unwrap_or_default();
            let summarized_input = summarize(&input_str, 80);
            let prefix = if is_error { "(error) " } else { "" };
            let summarized_result = summarize_blocks(&content, 80);
            eprintln!("{summarized_input})\n   → {prefix}{summarized_result}");
        }
        TurnEvent::TurnEnd { .. } => {}
        TurnEvent::RunEnd { total_usage, turn_count, .. } if !args.quiet => {
            eprintln!("\n[caliban: {turn_count} turns · {}↑ {}↓ tokens]", total_usage.input_tokens, total_usage.output_tokens);
        }
        _ => {}
    }
}
```

### Ctrl+C handling

```rust
let cancel = CancellationToken::new();
let cancel_handle = cancel.clone();
tokio::spawn(async move {
    let _ = tokio::signal::ctrl_c().await;
    eprintln!("\n[caliban: cancelling…]");
    cancel_handle.cancel();
    let _ = tokio::signal::ctrl_c().await;
    std::process::exit(130);
});
```

## Acceptance criteria

- `caliban` builds: `cargo build --bin caliban` succeeds.
- `caliban --help` shows the documented CLI surface.
- `caliban --version` still works (clap auto-derives it from `CARGO_PKG_VERSION`).
- `cargo run --bin caliban -- --provider ollama "say hi" --no-tools` works against a local Ollama (skipped if not running).
- Integration test: a `tests/` binary in `caliban/` that uses `MockProvider` to drive a fake "Read README.md" conversation, verifying the renderer produces expected stdout/stderr. (One end-to-end test, not exhaustive.)
- `cargo clippy --workspace --all-targets -- -D warnings` clean.

## Risks

- **Stream rendering on Windows terminals**: ANSI escapes may need `enable_ansi_support()`. We're focused on macOS/Linux for v1. Documented.
- **Tool-call summarization may hide useful info** when the result is JSON; one-line truncation could elide important details. Mitigation: include a `--verbose` flag in a follow-up that dumps full tool inputs/outputs.
- **Provider crate compile time** — pulling in all four provider crates inflates `caliban/Cargo.toml`. Plain build is fine; cloud-feature builds (Bedrock/Vertex/Azure) add minutes but aren't on the default path.
- **Anthropic streaming inside Ctrl+C handler** — the cancel-then-exit handler relies on the stream parser honoring the cancel token. The cancellation check inside the stream drain (from C's caveat-fix) ensures this works.
