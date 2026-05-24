# Memory Tier 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: `superpowers:executing-plans`.

**Goal:** Add a `caliban-memory` crate that loads three file-backed memory tiers (global CLAUDE.md, project CLAUDE.md, auto-memory MEMORY.md) and splices them into the system prompt when the default prompt is in effect.

**Architecture:** New `crates/caliban-memory` crate with `MemoryConfig`, `MemoryPrefix`, `TierFile`, async `load()`. Binary calls it between workspace resolution and system-prompt resolution. Memory is spliced only when no `--system`/`--system-file`/`--no-system` override is in effect.

**Spec:** `docs/superpowers/specs/2026-05-23-memory-tier-1-design.md`

---

## File map

| Path | Action |
|---|---|
| `crates/caliban-memory/Cargo.toml` | create |
| `crates/caliban-memory/src/lib.rs` | create — public API |
| `crates/caliban-memory/src/sanitize.rs` | create — `sanitize_workspace` |
| `crates/caliban-memory/src/config.rs` | create — `MemoryConfig::from_env` |
| `crates/caliban-memory/src/loader.rs` | create — async `load()` + budget enforcement |
| `crates/caliban-memory/src/prefix.rs` | create — `MemoryPrefix`, `splice_into`, `summary_lines` |
| `crates/caliban-memory/src/error.rs` | create — `MemoryError` |
| `crates/caliban-memory/tests/end_to_end.rs` | create — 3 integration tests |
| `Cargo.toml` (workspace) | add `crates/caliban-memory` member |
| `caliban/Cargo.toml` | add `caliban-memory` dep |
| `caliban/src/main.rs` | call `caliban_memory::load` between workspace + system_prompt; splice only when default prompt |
| `README.md` | one paragraph + ADR 0018 link |

---

## Tasks

### Task 1: Crate scaffold + sanitize_workspace
- `mkdir crates/caliban-memory/src`; `Cargo.toml` minimal.
- Workspace `Cargo.toml`: append `"crates/caliban-memory"` member.
- `src/sanitize.rs`: `pub fn sanitize_workspace(p: &Path) -> String` matching the spec rules.
- 4 unit tests: replaces_slashes, drops_leading_dash, replaces_unsafe_chars, idempotent.
- Commit.

### Task 2: `MemoryError` + `MemoryConfig::from_env`
- `src/error.rs`: thiserror enum with `Io`, `Utf8`, `OverBudget`.
- `src/config.rs`: `MemoryConfig` with global_path/project_path/auto_memory_dir/max_tokens; `from_env(workspace)` honors `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `CALIBAN_MEMORY_DIR`, `CALIBAN_MEMORY_BUDGET_TOKENS`.
- 3 unit tests.
- Commit.

### Task 3: `TierFile` + `MemoryPrefix::splice_into`
- `src/prefix.rs`: `TierFile { path, body, estimated_tokens, truncated_bytes }`, `MemoryPrefix { global, project, auto, estimated_tokens, truncated }`.
- `splice_into(&self, default_body: &str) -> String` building the tag-wrapped block.
- `summary_lines() -> Vec<String>` for `/memory` rendering.
- 3 unit tests: orders correctly, omits missing tiers, preserves default body.
- Commit.

### Task 4: Loader (`load`)
- `src/loader.rs`: async `load(workspace, config) -> Result<MemoryPrefix, MemoryError>`:
  - read global/project/auto with `tokio::fs::read_to_string`; missing files → None tier
  - clamp single-file disk read at 256 KB
  - sum tokens; if over budget, truncate auto→project→global on line boundary, append `[truncated: NNN bytes ...]` marker
  - first-run seed: if auto-memory dir missing, create + write seed `MEMORY.md`
  - inject the trailing HTML-comment conventions block into MEMORY.md content (in-memory only) if missing
- 5 unit tests: under-cap, truncates auto first, line boundary, global-alone-warn, conventions injected.
- Commit.

### Task 5: Crate-level re-exports + ensure all public types accessible
- `src/lib.rs`: re-export `MemoryConfig`, `MemoryPrefix`, `TierFile`, `MemoryError`, `load`, `sanitize_workspace`.
- Commit.

### Task 6: Integration tests
- `tests/end_to_end.rs`:
  - end_to_end_with_tempdir (golden splice)
  - end_to_end_seeds_empty_memory_md_on_first_run
  - end_to_end_handles_missing_xdg_vars (defaults to `~/.config` / `~/.local/share`)
- Commit.

### Task 7: Wire into `caliban/src/main.rs`
- Insert after workspace resolution: `let memory = caliban_memory::load(&workspace, MemoryConfig::from_env(&workspace)).await?;`
- After `system_prompt::resolve(...)`, splice ONLY when none of `--system`, `--system-file`, `--no-system` was given (track that in a `bool default_in_effect`).
- README paragraph.
- Commit.

### Task 8: `/memory` slash command (TUI) + `caliban memory show` subcommand
- TUI: add `/memory` to SLASH_COMMANDS; on accept, render `summary_lines()` to a new transcript variant or to the toast/overlay. (Use the existing `TranscriptLine::Info` for v1; full overlay can come later.)
- Binary: clap subcommand `caliban memory show` that prints summary to stdout.
- Commit.

### Task 9: Full verification + PR
- `cargo fmt --all`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, ci-cloud clippy.
- Push, open PR, merge after CI passes.
