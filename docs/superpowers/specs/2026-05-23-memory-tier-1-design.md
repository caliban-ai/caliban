# Memory Tier 1 — Design

**Date:** 2026-05-23
**Status:** Approved
**Target branch:** `jf/docs/roadmap-post-webfetch` (design only; impl branch TBD)
**Sub-project of:** caliban Rust agent harness
**Depends on:** `caliban-core` (WorkspaceRoot), `caliban` binary (system prompt assembly), ADR 0018
**ADR:** [0018-memory-tier-model.md](../../../docs/adr/0018-memory-tier-model.md)

## Goal

Give caliban persistent memory across sessions by spliceing three
file-backed tiers — operator-global `CLAUDE.md`, project `CLAUDE.md`,
and agent-writable auto-memory — into the system prompt at the start
of every run. The agent maintains auto-memory using the existing
`Write` and `Edit` tools; no new tool is introduced in v1.

## Non-goals

- RAG / vector search over memory. Memory IS the prompt prefix in v1.
- Cross-project memory (sharing learned facts across workspaces).
- A TUI editor / overlay for auto-memory. Use `$EDITOR` via `/memory`.
- Tier-4 MCP-mediated memory (forward link only; ships with MCP).
- Diff/merge of agent-edited memory across concurrent sessions
  (last-write-wins is acceptable for a single-operator harness).
- Encryption-at-rest. Markdown files on the operator's disk inherit
  whatever protections the disk already provides.

## Tier layout

| Tier | Path | Owner | Writable by agent | Loaded eagerly |
| ---- | ---- | ----- | ----------------- | -------------- |
| Global | `$XDG_CONFIG_HOME/caliban/CLAUDE.md` (default `~/.config/caliban/CLAUDE.md`) | Operator | No | Yes |
| Project | `<workspace_root>/CLAUDE.md` | Repo | No | Yes |
| Auto-memory index | `$XDG_DATA_HOME/caliban/projects/<sanitized-cwd>/memory/MEMORY.md` | Agent | Yes | Yes |
| Auto-memory pages | same dir, `<slug>.md` | Agent | Yes | No (Read on demand) |

`<sanitized-cwd>` is built from the absolute workspace root:

1. Canonicalize via `std::fs::canonicalize` (falls back to the original
   path on error — symlink rewrites are best-effort).
2. Strip the leading `/`.
3. Replace each remaining `/` with `-`.
4. Replace any character not in `[A-Za-z0-9._-]` with `_`.

Examples:
- `/Users/jf/dev/caliban` → `Users-jf-dev-caliban`
- `/home/jf/work/foo bar` → `home-jf-work-foo_bar`
- `C:\src\proj` (Windows) → `C__src_proj` (`:` and `\` replaced)

The `caliban-memory` crate provides this as `sanitize_workspace(&Path) -> String`.

## Ingestion flow at startup

The binary's startup path (currently `caliban/src/main.rs` lines
377–430) gets one new step between workspace resolution and system
prompt resolution:

```rust
let workspace = /* existing WorkspaceRoot resolution */;

let memory = caliban_memory::load(
    &workspace,
    MemoryConfig::from_env(), // honors XDG vars + CALIBAN_MEMORY_DIR
)
.await?; // Result<MemoryPrefix, MemoryError>

let system_prompt = system_prompt::resolve(
    args.system.as_deref(),
    args.system_file.as_deref(),
    args.no_system,
    &cwd,
    &tool_names,
    args.no_tools,
)?
.map(|body| memory.splice_into(&body));
```

`MemoryPrefix::splice_into(&self, default_body)` returns the assembled
string (memory blocks + blank line + default body). When the operator
supplies `--system`, `--system-file`, or `--no-system`, memory is
**not** spliced — the operator's choice is absolute. (Spliceing into
`--system-file` would be a footgun; if the operator wants memory plus
custom prompt, they can pull the prefix from a future `/memory show
--prefix` command.)

All file IO uses `tokio::fs` (consistent with the recent perf-baseline
work in commit `3233253`). Missing files are non-errors: they yield
empty tier blocks that are skipped at splice time.

## System-prompt splicing

The assembled prefix looks like:

```
<global-claude-md path="/home/jf/.config/caliban/CLAUDE.md">
… file contents …
</global-claude-md>

<project-claude-md path="/home/jf/dev/caliban/CLAUDE.md">
… file contents …
</project-claude-md>

<auto-memory-index path="/home/jf/.local/share/caliban/projects/home-jf-dev-caliban/memory/MEMORY.md">
… file contents …
</auto-memory-index>

<default system prompt body>
```

Tag delimiters are XML-style because Claude responds well to them and
they're explicit for any provider. A tier whose file is missing
contributes zero bytes (no empty tag block).

`MemoryPrefix` also carries provenance — the absolute path of each
loaded file — so `/memory` (below) can show paths without re-walking
the filesystem.

## Token budget enforcement

`MemoryConfig` carries `max_tokens: usize` (default **8 000**). The
estimator is `chars / 4` — provider-agnostic, deterministic, fast.

Algorithm:

1. Read all three files into `(tier, path, body)` tuples (UTF-8 lossy
   decode; clamp any single file at 256 KB on disk before counting).
2. Compute `tokens(body)` for each.
3. Sum. If `≤ max_tokens`, splice as-is.
4. Else truncate in reverse priority order: auto-memory first, then
   project, then global. Truncation cuts on a line boundary so the
   result is still valid Markdown, and appends:
   `\n\n[truncated: NNN bytes over budget; raise CALIBAN_MEMORY_BUDGET_TOKENS or trim]`.
5. If even after dropping auto-memory + project the global file alone
   exceeds budget, truncate it and emit a `tracing::warn!` —
   operator's bug, but don't fail startup.

## Workspace-aware auto-memory directory

Auto-memory is **per workspace**, not per session. Two `caliban`
sessions in the same workspace share an auto-memory dir; sessions in
different workspaces are isolated.

`MemoryConfig::auto_memory_dir(workspace)` returns the path and
ensures it exists (`tokio::fs::create_dir_all`). The first time
caliban runs in a workspace, the dir is created lazily with an empty
seed `MEMORY.md`:

```markdown
# Memory index

_No memories yet. Add entries below as `- [title](slug.md) — one-line summary`._
```

This gives the agent a target to `Edit` rather than a missing file
that triggers a `Write` it might forget to do.

## `/memory` slash command

In the TUI (and as a CLI subcommand `caliban memory show`), `/memory`
prints:

```
Active memory tiers (combined: 1 842 tokens / 8 000 budget):

  global   ~/.config/caliban/CLAUDE.md            (412 tokens)
  project  ~/dev/caliban/CLAUDE.md                ( 87 tokens, missing)
  auto     ~/.local/share/caliban/projects/…/MEMORY.md  (1 343 tokens, 6 entries)

Topic pages (lazy-loaded):
  ~/.local/share/…/memory/
    caliban-overview.md   1 412 bytes
    personal-email.md       312 bytes
    sprint-mode.md          468 bytes

Commands:
  /memory edit global    open global CLAUDE.md in $EDITOR
  /memory edit project   open project CLAUDE.md in $EDITOR
  /memory edit auto      open auto-memory MEMORY.md in $EDITOR
  /memory reload         re-read all tiers (next turn picks them up)
```

`/memory reload` is the escape hatch when the operator edits memory
mid-session — the in-memory `MemoryPrefix` is rebuilt and stored on
the TUI's `App`, but the persisted system prompt in the **current**
session message log is not retroactively rewritten (per ADR 0014's
persistence rule). The reloaded prefix takes effect for the *next*
new session; for the current one, only ephemeral REPL turns pick up
the change. Documented as expected behavior.

## Auto-memory write conventions

The agent learns these conventions via a paragraph injected at the
bottom of the `<auto-memory-index>` block (so it's read every turn).
Mirrors the spirit of the user's own CLAUDE.md taxonomy:

**Save to auto-memory:**

- **User profile** — stable facts about the operator (preferred email
  per repo context, name, timezone) that they've stated and that you'd
  re-derive every session if you didn't write them down.
- **Feedback / standing rulings** — "always use rg, never grep";
  "when working in personal repos, use `john.ford2002@gmail.com`";
  "sprint mode: skip the human-review checkpoint."
- **Project state** — facts about the workspace that aren't in the
  repo itself (where a deployed service lives, what the staging URL
  is, which branch is the long-running integration branch).
- **External references** — pointers to people, docs, ticket trackers.

**Do NOT save to auto-memory:**

- Transient task state ("currently fixing the X bug") — the session
  log already covers this.
- Debugging traces, error messages, command output — clutter.
- Code patterns already in the repo — duplication; let the agent
  `Grep` the source instead.
- Anything the operator hasn't confirmed or that the agent inferred
  weakly. False memories are worse than no memories.

**Format conventions:**

- One topic per file. File slug is `kebab-case`.
- Topic page starts with `# <Title>` and a one-sentence summary line.
- `MEMORY.md` is the index ONLY. Each entry is a bullet:
  `- [title](slug.md) — one-line summary` (≤ 100 chars).
- When deleting a memory, remove both the topic file AND the
  `MEMORY.md` line.
- `MEMORY.md` must stay ≤ 200 lines. If it grows past that, the
  agent is expected to merge related topics or prune.

## MEMORY.md index format

```markdown
# Memory index

- [caliban overview](caliban-overview.md) — project: Rust agent harness, AGPL-3.0, provider-agnostic, ~11 crates
- [personal email](personal-email.md) — feedback: use john.ford2002@gmail.com for author fields in ~/dev/personal/**
- [sprint mode](sprint-mode.md) — feedback: user prefers consolidated design + spec + plan + impl in one pass

<!-- caliban: auto-memory conventions follow; do not delete -->
Write to this index when you learn something durable about the user, project, or environment. One topic per file, slug in kebab-case. Do not save transient task state, debug traces, or facts already in the repo. Keep this file ≤ 200 lines.
```

The trailing HTML-comment block is injected by `caliban-memory` if not
present, so the conventions are always in the agent's prompt without
the operator having to maintain them.

## Crate location

**New crate: `crates/caliban-memory/`.** Reasons:

- `caliban-agent-core` should not depend on filesystem layout or
  XDG conventions — those are binary-level concerns.
- The crate has a narrow, testable surface: path sanitization,
  config resolution, async file reads, splice formatting, budget
  enforcement.
- A future MCP tier loader can live in the same crate (or a sibling
  `caliban-memory-mcp`) without dragging `caliban-agent-core` along.

Public surface:

```rust
pub struct MemoryConfig {
    pub global_path: Option<PathBuf>,
    pub project_path: Option<PathBuf>,
    pub auto_memory_dir: PathBuf,
    pub max_tokens: usize,
}

impl MemoryConfig {
    pub fn from_env(workspace: &WorkspaceRoot) -> Self;
}

pub struct MemoryPrefix {
    pub global: Option<TierFile>,
    pub project: Option<TierFile>,
    pub auto: Option<TierFile>,
    pub estimated_tokens: usize,
    pub truncated: bool,
}

impl MemoryPrefix {
    pub fn splice_into(&self, default_body: &str) -> String;
    pub fn summary_lines(&self) -> Vec<String>; // for /memory
}

pub struct TierFile {
    pub path: PathBuf,
    pub body: String,
    pub estimated_tokens: usize,
    pub truncated_bytes: usize,
}

pub async fn load(
    workspace: &WorkspaceRoot,
    config: MemoryConfig,
) -> Result<MemoryPrefix, MemoryError>;

pub fn sanitize_workspace(p: &Path) -> String;
```

Workspace deps: `tokio` (workspace), `tracing`, `thiserror`, `dirs`,
`caliban-core` (for `WorkspaceRoot`).

## Testing strategy

Unit tests in `caliban-memory`:

1. `sanitize_workspace_replaces_slashes` — `/a/b/c` → `a-b-c`.
2. `sanitize_workspace_drops_leading_dash` — no leading `-`.
3. `sanitize_workspace_replaces_unsafe_chars` — spaces, `:`, `\` → `_`.
4. `sanitize_workspace_idempotent` — twice = once.
5. `splice_into_orders_tiers_correctly` — global → project → auto.
6. `splice_into_omits_missing_tiers` — no empty tag blocks.
7. `splice_into_preserves_default_body` — default body appended
   verbatim with one blank line.
8. `budget_under_cap_no_truncation` — small files, `truncated=false`.
9. `budget_truncates_auto_first` — auto + project + global oversized;
   only auto carries the truncation marker, others intact.
10. `budget_truncates_on_line_boundary` — never mid-line.
11. `budget_emits_warn_when_global_alone_oversized` — `tracing-test`
    captures the warn event.
12. `token_estimate_uses_chars_div_4` — deterministic.

Integration test (`crates/caliban-memory/tests/end_to_end.rs`):

13. `end_to_end_with_tempdir` — `tempfile::TempDir` builds a fake
    home + workspace, writes a global + project + MEMORY.md, calls
    `load`, asserts splice matches a golden string.
14. `end_to_end_seeds_empty_memory_md_on_first_run` — auto-memory
    dir doesn't exist; after `load`, `MEMORY.md` exists with the
    seed content.
15. `end_to_end_handles_missing_xdg_vars` — unset `$XDG_*`, defaults
    to `~/.config` and `~/.local/share` per `dirs` crate.

Binary-level test (smoke, optional): `caliban --no-tools --no-system
--prompt "echo memory"` does NOT splice memory (operator override
honored).

## Risks

- **Operator writes secrets to `CLAUDE.md`.** Memory is sent to the
  provider on every turn. Mitigation: the spec is the warning; we
  do not auto-detect or redact. Documented in the `/memory` output:
  "Contents are sent to your provider on every turn."
- **Agent writes garbage to auto-memory.** A model that ignores the
  conventions can fill `MEMORY.md` with debug noise. Mitigation: the
  conventions live inside the prompt every turn, and the 200-line
  cap on `MEMORY.md` forces eventual pruning. `/memory edit auto`
  is the manual escape hatch.
- **Token-cap surprise on the first prompt.** A 6 000-token global
  file plus a 4 000-token project file silently truncates
  auto-memory. The truncation marker plus the `/memory` byte+token
  display make this visible. Operator can raise
  `CALIBAN_MEMORY_BUDGET_TOKENS`.
- **Provider prompt-caching invalidates on every auto-memory edit.**
  Auto-memory is at the end of the prefix; an edit invalidates the
  cache for the prefix. With Anthropic prompt caching (enabled per
  default per main.rs), this means an auto-memory write costs one
  full-prefix cache miss next turn. Acceptable — writes are rare
  compared to reads.
- **Concurrent sessions race on `MEMORY.md`.** Two `caliban`
  processes in the same workspace both editing auto-memory will
  last-write-wins. Acceptable for a single-operator harness;
  documented. If multi-agent runs become common, switch the
  auto-memory backing to SQLite with WAL.
- **Path sanitization collision.** `/a/b` and `/a-b` both sanitize
  to `a-b`. Unlikely in practice; if it bites, append an 8-char
  blake3 hash suffix in a future migration.

## Acceptance criteria

- New crate `caliban-memory` builds clean: `cargo build -p caliban-memory`
  and `cargo clippy -p caliban-memory -- -D warnings`.
- `cargo test -p caliban-memory` passes — ≥ 12 unit tests + ≥ 3
  integration tests per the table above.
- `caliban/src/main.rs` calls `caliban_memory::load` between
  workspace resolution and system-prompt resolution; memory is
  spliced only when the default system prompt is in effect.
- `/memory` slash command implemented in the TUI; `caliban memory
  show` subcommand in the binary.
- README's overview gets one paragraph and a forward link to ADR 0018.
- ADR 0018 merged before this spec's implementation lands.
- Manual smoke: run `caliban --prompt "what do you know about me?"`
  in a workspace with a hand-authored `CLAUDE.md`; the model's
  response references the file's contents.
