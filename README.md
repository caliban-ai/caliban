# caliban

A from-scratch Rust agent harness — a replacement for Claude Code that puts
the operator in control of model routing, memory, skills, and prompt context.

> **Project status:** Layer 0 (workspace bootstrap). Private repo, designed
> to be open-sourced. The user-facing binary (`caliban --version`) exists
> as a stub; no real agent runtime yet.

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
1.85.0). `rustup` will install it automatically on first `cargo`
invocation.

```bash
cargo build --workspace             # build everything
cargo test  --workspace             # run all tests
cargo run   --bin caliban -- --version    # smoke-test the binary
```

## Repository layout

```
caliban/             # the user-facing binary
crates/              # libraries
  caliban-core/      # foundational types (Layer 1 seed)
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

See [`adrs/`](adrs/). Notable Layer-0 decisions:
- [Async runtime: tokio](adrs/0001-async-runtime.md)
- [Error model: thiserror libs, anyhow binary](adrs/0002-error-model.md)
- [License: AGPL-3.0](adrs/0003-license-agpl-3.0.md)
- [Naming conventions](adrs/0004-naming-conventions.md)
- [Workspace layout](adrs/0005-workspace-layout.md)

## Design specs

The Layer-0 spec lives at
[`docs/superpowers/specs/2026-05-22-layer-0-bootstrap-design.md`](docs/superpowers/specs/2026-05-22-layer-0-bootstrap-design.md).
