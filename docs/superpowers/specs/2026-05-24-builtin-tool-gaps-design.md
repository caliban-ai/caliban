# Built-in tool gaps — Design (`WebSearch`, `NotebookEdit`, `MultiEdit`, background Bash)

**Date:** 2026-05-24
**Status:** Proposed
**Author:** john.ford2002@gmail.com
**Sub-project of:** caliban Rust agent harness
**ADR:** (none — these are additive tools, not architectural shifts)
**Depends on:** existing permissions framework (ADR 0020), Bash tool
implementation in `caliban-tools-builtin`, image-input spec only for the
shared `caliban.toml` discovery path (otherwise standalone).

## Goal

Close four residual built-in-tool parity gaps with Claude Code by adding
the corresponding tools to `caliban-tools-builtin`:

1. **`WebSearch`** — query a web search API and return ranked results
   (title + URL + snippet) as text content blocks. Default backend: Brave
   Search; env-toggled alternatives Tavily and Exa.
2. **`NotebookEdit`** — read/write Jupyter `.ipynb` cells (v4 schema):
   add, edit, delete, or move a cell by index, preserving metadata and
   outputs.
3. **`MultiEdit`** — atomic multi-replacement on a single file: a list of
   `{old, new, replace_all?}` operations applied in order, rolled back on
   any failure.
4. **Background Bash + `Ctrl+B`** — run a Bash command in the background,
   assign it a shell id, expose `BashOutput` to read its streams and
   `KillShell` to terminate. In the TUI, `Ctrl+B` moves the currently-
   running foreground Bash to background and the status bar shows the
   running-background count.

Each is implemented as a discrete `impl Tool` (and, for background Bash,
a small shared registry). All four respect the existing permission rule
grammar and the in-process `Hooks` chain.

## Non-goals

- **Search re-ranking or summarization.** `WebSearch` returns the raw
  ranked list from the provider; we do not run a local re-rank or
  summarize via the model router.
- **NotebookEdit v3 (`worksheets`) support.** v1 supports nbformat 4
  only; v3 notebooks are rare in modern repos and a separate concern.
- **Notebook execution.** `NotebookEdit` edits cells; it does not run
  them. Cell execution is a future tool.
- **MultiEdit across files.** A single MultiEdit invocation touches one
  file; for multi-file refactors the agent composes MultiEdit calls.
- **Persistent background Bash across sessions.** Background shells live
  inside one caliban session and are killed on session exit. The
  supervisor daemon (ADR 0037) owns cross-session persistence; the
  per-session Bash registry intentionally doesn't.
- **Streaming `BashOutput`.** The tool returns a snapshot of the
  current buffer; the agent polls. We do not surface a live stream
  through the Tool trait (it doesn't fit the request/response shape).

## Architecture

```
crates/caliban-tools-builtin/src/
├── web_search.rs        # WebSearch tool + provider trait + Brave/Tavily/Exa
├── notebook_edit.rs     # NotebookEdit tool + nbformat 4 model
├── multi_edit.rs        # MultiEdit tool + atomic apply
├── bash.rs              # (modified) foreground entry point + Ctrl+B handoff
├── bash_bg.rs           # NEW: background registry + BashOutput + KillShell
└── lib.rs               # register_builtin extended with the four tools
```

All four tools register through the existing `register_builtin(&mut
ToolRegistry, &Config)` entry point so the binary just rebuilds.

## Tool 1: `WebSearch`

### Input schema

```jsonc
{
  "type": "object",
  "properties": {
    "query":   { "type": "string", "description": "Search query." },
    "count":   { "type": "integer", "minimum": 1, "maximum": 20, "default": 10 },
    "country": { "type": "string", "description": "ISO-3166 alpha-2; default US." },
    "freshness": {
      "type": "string",
      "enum": ["pd", "pw", "pm", "py", "any"],
      "default": "any",
      "description": "Past day / week / month / year / any."
    }
  },
  "required": ["query"]
}
```

### Output shape

A `Vec<ContentBlock::Text>` containing one block per result, formatted:

```
1. [Result title](https://example.com/path)
   Snippet text from the search provider, possibly multi-line.
   Domain: example.com · published: 2025-12-10
```

Plus a final block with `Searched "<query>" via Brave; <N> results in <T>ms.`
The agent ingests these as ordinary text; no custom IR.

### Provider abstraction

```rust
#[async_trait]
trait WebSearchProvider: Send + Sync {
    async fn search(&self, q: &WebSearchInput) -> Result<Vec<SearchHit>, WebSearchError>;
    fn name(&self) -> &'static str;
}

pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub domain: String,
    pub published: Option<DateTime<Utc>>,
}
```

Provider selection (priority order):

1. `CALIBAN_WEBSEARCH_PROVIDER` env var = `brave|tavily|exa`.
2. `[tools.web_search.provider]` in `caliban.toml`.
3. Default: `brave`.

Provider construction reads the per-provider key:

| Provider | Env var          | Endpoint                                       |
| -------- | ---------------- | ---------------------------------------------- |
| Brave    | `BRAVE_API_KEY`  | `https://api.search.brave.com/res/v1/web/search` |
| Tavily   | `TAVILY_API_KEY` | `https://api.tavily.com/search`                |
| Exa      | `EXA_API_KEY`    | `https://api.exa.ai/search`                    |

### Missing API key behavior

`WebSearch::invoke` checks for the configured provider's key at call
time and returns a structured `ToolError::execution`:

```
WebSearch is not configured. Set BRAVE_API_KEY (default provider) or
CALIBAN_WEBSEARCH_PROVIDER + the matching key (TAVILY_API_KEY / EXA_API_KEY).
See caliban.toml [tools.web_search] for in-file config.
```

The error is *the tool result*, not a registry-side failure — the agent
can read it and try a different approach (e.g., a `WebFetch` to a known
URL). Per-key-source documentation lives in `caliban.toml.example`.

### Permissions

Inherits the existing rule grammar. Default policy:
`WebSearch(<domain>)` → ask. A domain glob (`*.example.com`) can be
allowed/denied. Operators who want default-allow can write
`[[permissions.allow]] match = "WebSearch"`.

## Tool 2: `NotebookEdit`

### Input schema

```jsonc
{
  "type": "object",
  "properties": {
    "notebook_path": { "type": "string" },
    "action": {
      "type": "string",
      "enum": ["add", "edit", "delete", "move"]
    },
    "cell_index": { "type": ["integer", "null"], "minimum": 0 },
    "cell_type":  { "type": ["string", "null"], "enum": ["code", "markdown", "raw", null] },
    "source":     { "type": ["string", "null"] },
    "to_index":   { "type": ["integer", "null"], "minimum": 0 }
  },
  "required": ["notebook_path", "action"]
}
```

Action semantics:

- `add` — requires `cell_type`, `source`. If `cell_index` is set, insert
  before that index; otherwise append.
- `edit` — requires `cell_index`, `source`. Optionally changes `cell_type`.
  Preserves the cell's `metadata` and `outputs` (for code cells).
- `delete` — requires `cell_index`.
- `move` — requires `cell_index` + `to_index`.

### Parsing

`serde_json::from_str::<Notebook>(...)` against the nbformat-4 schema:

```rust
#[derive(Serialize, Deserialize)]
struct Notebook {
    nbformat: u32,                              // 4
    nbformat_minor: u32,
    metadata: serde_json::Value,                // opaque
    cells: Vec<Cell>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "cell_type", rename_all = "lowercase")]
enum Cell {
    Code     { id: Option<String>, source: SourceLines, execution_count: Option<u64>,
               outputs: Vec<serde_json::Value>, metadata: serde_json::Value },
    Markdown { id: Option<String>, source: SourceLines, metadata: serde_json::Value },
    Raw      { id: Option<String>, source: SourceLines, metadata: serde_json::Value },
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum SourceLines { Multi(Vec<String>), Single(String) }
```

If `nbformat != 4` we return
`ToolError::execution("NotebookEdit requires nbformat 4; found <N>")`.

### Write-back

After mutation, the notebook is re-serialized with `serde_json` (pretty,
2-space indent — what `jupyter nbconvert` emits) and written atomically:
write to `<path>.tmp`, fsync, rename. Cell ids are preserved on edit;
new cells receive a random ulid-style id (the `nbformat` spec accepts
any string, but Jupyter prefers stable ids).

### Output

`[ContentBlock::Text("Edited cell <i> in <path>.")]` with a diff-style
summary of the change. Operations that produce no diff (no-op edit)
return `[ContentBlock::Text("No change applied: <reason>")]`.

### Permissions

Reuses `Write(<path>)` rule patterns. NotebookEdit on `secrets.ipynb`
is denied by the same rules that would deny `Write` on it.

## Tool 3: `MultiEdit`

### Input schema

```jsonc
{
  "type": "object",
  "properties": {
    "file_path": { "type": "string" },
    "edits": {
      "type": "array",
      "minItems": 1,
      "items": {
        "type": "object",
        "properties": {
          "old":         { "type": "string" },
          "new":         { "type": "string" },
          "replace_all": { "type": "boolean", "default": false }
        },
        "required": ["old", "new"]
      }
    }
  },
  "required": ["file_path", "edits"]
}
```

### Atomic apply algorithm

1. **Pre-flight read** — open `file_path` and load into a `String`. If the
   file is binary (non-UTF-8), error: `ToolError::execution("MultiEdit
   requires UTF-8 text files")`.
2. **Backup** — write the current bytes to `<file_path>.caliban.bak`
   (overwriting any previous backup). The backup is removed on success.
3. **Sequential apply in memory** — for each edit in order:
   - `replace_all = false`: find the *first* occurrence of `old`. If
     `old` is not present, fail this edit.
     If `old` is present more than once and `replace_all` was not set,
     fail this edit (mirrors the `Edit` tool's uniqueness contract).
     Otherwise replace once.
   - `replace_all = true`: count occurrences (must be ≥ 1); replace all.
   - Any failure aborts: discard the in-memory string, leave the
     backup, return a structured error naming the failing edit.
4. **Atomic write** — write the final string to `<file_path>.tmp`, fsync,
   rename to `<file_path>`. Delete `<file_path>.caliban.bak`.
5. **Rollback** — if the rename fails (rare; disk full / permissions),
   restore from `<file_path>.caliban.bak` and surface the rename error.

### Output

A summary block listing each successful edit and the resulting line
counts:

```
MultiEdit applied 3 edits to src/agent.rs:
  • replaced "fn execute(" → "async fn execute(" (1 occurrence)
  • replaced "Result<()>"  → "Result<(), Error>" (all 4 occurrences)
  • replaced "// TODO"     → "// fixed" (1 occurrence)
Lines: 184 → 187.
```

### Permissions

`MultiEdit(<path>)` falls under the same permission predicate as `Edit`
and `Write`. The rule grammar matches by tool family
(`Edit|MultiEdit|Write`); an `allow Write(crates/**)` rule also allows
MultiEdit there.

## Tool 4: Background Bash + `Ctrl+B` + `BashOutput` + `KillShell`

Three tools and one TUI integration; all share the background registry.

### Shared registry — `bash_bg.rs`

```rust
pub type ShellId = String;       // ulid-style, 12-char prefix

pub struct BashJob {
    pub id: ShellId,
    pub command: String,
    pub started_at: Instant,
    pub status: BashStatus,                    // Running | Exited(code) | Killed
    pub stdout: BoundedRingBuffer,             // append-only, capped at 5 GiB
    pub stderr: BoundedRingBuffer,
    pub child: Mutex<Option<tokio::process::Child>>,
    pub cancel: CancellationToken,
}

pub struct BashBgRegistry {
    jobs: Arc<Mutex<HashMap<ShellId, Arc<BashJob>>>>,
    pub max_concurrent: usize,                 // default 8
}

pub fn registry() -> Arc<BashBgRegistry> { /* OnceLock singleton */ }
```

The registry is shared by all four touch-points (Bash tool when
backgrounding, `BashOutput` tool, `KillShell` tool, TUI status bar). It
is a tokio-friendly singleton (`OnceLock<Arc<BashBgRegistry>>`); lives
for the duration of the caliban process.

### `BashStatus` and ring-buffer

```rust
pub enum BashStatus { Running, Exited(i32), Killed, Timeout }

pub struct BoundedRingBuffer {
    cap_bytes: usize,    // 5 GiB == 5 * 1024 * 1024 * 1024
    written:   u64,      // monotonic total written (for "newer than offset" queries)
    head:      usize,    // virtual offset of byte 0 of `buf`
    buf:       VecDeque<u8>,
}
```

The 5 GiB cap matches Claude Code's documented background-bash buffer.
On overflow the oldest bytes are dropped; the `BashOutput` reply carries
a `[truncated: dropped N bytes]` notice.

### `Bash` (modified) — backgrounding entry point

The existing foreground `Bash` tool gains:

- An input flag `background: bool` (default false). When `true`, the
  command is enrolled in the registry immediately and the tool returns
  `[ContentBlock::Text("Started background shell <id>: <cmd>")]` without
  waiting.
- Internal: if the foreground execution path receives a "background
  this" signal (from the TUI's `Ctrl+B`, via a `CancellationToken`
  variant `Backgrounded`), the running future drops cleanly into the
  registry without restarting the child process — we own the `Child`
  handle already; we move it from the foreground task into a new
  registry-owned task.

### `BashOutput` tool

```jsonc
{
  "type": "object",
  "properties": {
    "shell_id":     { "type": "string" },
    "since_offset": { "type": ["integer", "null"], "description": "Return only bytes after this byte offset." }
  },
  "required": ["shell_id"]
}
```

Returns:

```
status: Running   started: 2026-05-24T14:02:11Z   age: 32s
stdout (bytes 0..12482):
<contents>

stderr (bytes 0..47):
<contents>
```

When `since_offset` is set, returns only the slice past that offset and
labels with the new offset. Used for incremental polling.

### `KillShell` tool

```jsonc
{
  "type": "object",
  "properties": {
    "shell_id": { "type": "string" },
    "signal":   { "type": "string", "enum": ["TERM", "KILL"], "default": "TERM" }
  },
  "required": ["shell_id"]
}
```

`TERM` is the default; falls through to `KILL` after a 10-second grace
window if the child still lives. Output:
`[ContentBlock::Text("Killed shell <id>; exit=Killed; consumed_stdout=12482 bytes")]`.

### TUI `Ctrl+B`

When the currently-running tool is a foreground `Bash`:

1. The TUI sends a `BackgroundRequest` over the agent's tool-control
   channel.
2. The Bash tool future returns `Ok(vec![ContentBlock::Text(...)])` with
   the registry id and immediately re-enrolls itself in the registry
   (handing off the `Child` handle).
3. The status bar gains a `[bg: N]` chip showing running-background
   count; clicking (or `/bg`) opens an overlay listing them.

When the currently-running tool is *not* Bash, `Ctrl+B` instead routes
to the sub-agent backgrounding path (ADR 0037). Resolution is by tool
name match — if `cx.tool_name == "Bash"`, do background-bash; if
`cx.tool_name == "AgentTool"`, do sub-agent background; otherwise
beep.

### TUI `/bg` overlay

```
┌─ Background shells ────────────────────────────────────────────────────┐
│ ● 01HXY… running  cargo test --workspace            stdout: 482 KiB    │
│ ✓ 01HXZ… exit 0   npm run build                     stdout: 12 KiB     │
│ ✗ 01HYA… killed   python long_pipeline.py           stdout: 4.0 GiB ⚠ │
└────────────────────────────────────────────────────────────────────────┘
[o] read output  [k] kill  [x] remove  [esc] close
```

### Auto-cleanup on session exit

On graceful caliban exit (Ctrl+C, `/quit`, normal end-of-stream), the
registry walks its jobs:

- `Running` jobs: send `TERM`, wait up to 5s, then `KILL`. Log the
  kill list in the session's final stderr.
- `Exited`/`Killed` jobs: drop the buffers, drop the entries.

The session's `manifest.toml` (sessions crate) records the final list
so post-mortem inspection knows what was running.

### Permissions

Background Bash respects the same rule grammar as foreground Bash.
`BashOutput` and `KillShell` are scoped to the current session's
registry; no extra permission predicates needed (they operate on
already-launched shells the agent itself produced).

## Crate Cargo deltas

```toml
# caliban-tools-builtin/Cargo.toml
reqwest    = { workspace = true }              # WebSearch
ulid       = "1"                                # ShellId + cell ids
tempfile   = { workspace = true }              # MultiEdit atomic swap
serde_json = { workspace = true }              # NotebookEdit
chrono     = { workspace = true }              # SearchHit.published
```

All four already exist in the workspace tree or are minor additions.

## Testing strategy

Six tests per tool, plus integration:

**`WebSearch` (6):**

1. `brave_provider_parses_minimal_response_into_search_hits`
2. `tavily_provider_parses_minimal_response_into_search_hits`
3. `missing_api_key_returns_structured_tool_error`
4. `count_clamp_caps_at_20`
5. `freshness_param_forwarded_correctly`
6. `permission_ask_default_blocks_without_allow_rule`

**`NotebookEdit` (6):**

7. `parses_nbformat_4_notebook_round_trip_preserves_unknown_metadata`
8. `add_appends_new_cell_when_index_omitted`
9. `edit_preserves_metadata_and_outputs_on_code_cell`
10. `delete_drops_cell_at_index_shifts_remainder`
11. `move_relocates_cell_preserving_id`
12. `rejects_nbformat_3_with_clear_error`

**`MultiEdit` (6):**

13. `applies_sequence_of_edits_in_order`
14. `rolls_back_on_missing_old_string_in_third_edit`
15. `replace_all_false_with_duplicate_old_fails_loudly`
16. `replace_all_true_replaces_every_occurrence`
17. `binary_file_returns_structured_error_before_backup_written`
18. `atomic_swap_via_tempfile_keeps_original_on_rename_failure`

**Background Bash + `BashOutput` + `KillShell` (6):**

19. `bash_with_background_true_returns_immediately_with_shell_id`
20. `bash_output_reads_streaming_stdout_with_since_offset`
21. `kill_shell_sends_term_then_kill_after_grace`
22. `ring_buffer_drops_oldest_bytes_at_5gib_cap`  *(test uses 1 MiB cap injected for speed)*
23. `session_exit_kills_all_running_jobs`
24. `tui_ctrl_b_handoff_preserves_child_pid`

Total: ~24 tests across the four tools.

## Risks

- **Search-API rate limits / cost.** A loop agent can spam `WebSearch`
  and burn an API budget. Mit: optional `[tools.web_search.daily_cap]`
  in `caliban.toml`; default `unlimited`; the tool tracks a per-process
  counter and surfaces "daily cap exceeded; see config" as a tool
  error after the cap.
- **NotebookEdit and merge conflicts.** Two parallel agents editing the
  same notebook step on each other. Mit: atomic write + sha256 check
  (read sha at start, compare at write — abort with "notebook changed
  on disk; re-read and retry").
- **MultiEdit backup races.** The `.caliban.bak` file is a side channel
  visible to `git status`. Mit: atomic create (`O_CREAT|O_EXCL`); delete
  on success; document that `.gitignore` should include
  `*.caliban.bak`. Default add to project `.gitignore` on first run via
  the same `caliban-skills`-style nudge.
- **5 GiB ring-buffer memory pressure.** Five jobs at the cap = 25 GiB
  resident. Mit: cap is per-job, and most jobs never hit it; the TUI
  `/bg` overlay shows a ⚠ on jobs that exceed (e.g.) 1 GiB so the
  operator can decide whether to KillShell. Configurable per-session.
- **`Ctrl+B` ambiguity.** When the foreground activity is *neither* Bash
  nor an AgentTool, `Ctrl+B` has no obvious meaning. Mit: beep + status
  line "Ctrl+B has no effect here"; document the two use sites.
- **Brave vs. Tavily vs. Exa schema drift.** Each provider's response
  shape differs. Mit: a thin Adapter trait per provider; record API
  responses against `wiremock` so changes are caught in CI.
- **NotebookEdit cell id collisions.** ulid generation is collision-safe
  but Jupyter accepts arbitrary strings — an operator-named cell id
  could collide with a new ulid. Mit: when creating a new cell, check
  the existing id set; on collision, regenerate.

## Acceptance criteria

- `cargo build --workspace` clean; `cargo clippy --workspace --all-targets
  -- -D warnings` clean; `cargo fmt --all -- --check` clean.
- ≥24 new tests passing across the four tools.
- `register_builtin` registers `WebSearch`, `NotebookEdit`, `MultiEdit`,
  `BashOutput`, `KillShell` (and the existing `Bash` gains the
  `background` flag).
- The TUI's `Ctrl+B` foreground-Bash handoff is functional, surfaces a
  shell id in the transcript, and the `/bg` overlay lists running
  jobs with read/kill/remove actions.
- WebSearch returns useful results against a live Brave key
  (manual smoke test documented in the PR description).
- MultiEdit atomic swap survives an injected rename failure
  (`fault-injection` fixture).
- NotebookEdit round-trips a real-world notebook (one fixture per
  test, plus a `tests/data/sample.ipynb` taken from the JupyterLab
  examples tree).
- Auto-cleanup terminates background jobs on `/quit`; the final
  `manifest.toml` records the kill list.
- Matrix rows F:WebSearch, F:NotebookEdit, F:MultiEdit, and
  E:"Background bash (`Ctrl+B`)" all move 🔴 → ✅ in the PR that
  lands this work.
