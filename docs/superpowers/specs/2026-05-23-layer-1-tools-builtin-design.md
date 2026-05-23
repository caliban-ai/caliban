# Layer 1 / D · Tools-Builtin · Design

- **Date:** 2026-05-23
- **Status:** Draft (pending implementation plan)
- **Sub-project of:** caliban Rust agent harness
- **Depends on:** Layer 1 / C (`caliban-agent-core`)
- **Next sub-project:** Layer 4 / CLI

## Goals

Ship `caliban-tools-builtin` — one new crate providing six tools that an agent can use: Read, Write, Edit, Bash, Glob, Grep. Each tool implements `caliban_agent_core::Tool` with a JSON schema for its input. Tools are workspace-root-aware: relative paths resolve against a configurable root; absolute paths are allowed but the operator can opt into restriction.

**Acceptance:** A consumer can write:

```rust
let mut registry = ToolRegistry::new();
let root = WorkspaceRoot::new("/some/dir");
registry.register(Arc::new(ReadTool::new(root.clone())));
registry.register(Arc::new(WriteTool::new(root.clone())));
registry.register(Arc::new(BashTool::new(root.clone())));
registry.register(Arc::new(EditTool::new(root.clone())));
registry.register(Arc::new(GlobTool::new(root.clone())));
registry.register(Arc::new(GrepTool::new(root)));
```

…and pass that registry into `Agent::builder().tools(registry)`. The agent will dispatch model-emitted tool calls to the right tool.

## Non-goals

- Process supervision (long-running background tools, signals beyond timeout-kill) — deferred.
- TodoWrite / planning / specialized agent meta-tools — separate sub-project.
- WebFetch — separate sub-project (involves credential management for some endpoints).
- Interactive tool approval — handled at the CLI layer via custom `Hooks`.
- Restrictive sandboxing — the operator runs the harness with their own permissions; tools are as restricted as their `WorkspaceRoot` makes them, no more.

## Tools

### `Read`

Reads a file's text content. Input:

```json
{ "type": "object", "properties": {
  "path": { "type": "string", "description": "Path to read, relative to workspace root or absolute" },
  "limit": { "type": "integer", "description": "Max lines to read (default: 2000)", "minimum": 1 },
  "offset": { "type": "integer", "description": "Line to start at (1-indexed; default: 1)", "minimum": 1 }
}, "required": ["path"] }
```

Behavior:
- Resolve path against workspace root.
- Read as UTF-8; if invalid, return error (`ToolError::Execution`).
- Apply offset + limit (line-based).
- Return one `ContentBlock::Text` with the content prefixed by `→ Read {path}, lines {start}-{end} of {total}:\n\n`.
- Max file size: 5 MB. Larger files return an error suggesting offset/limit.

### `Write`

Creates or overwrites a file with the given content. Input:

```json
{ "type": "object", "properties": {
  "path": { "type": "string" },
  "content": { "type": "string" }
}, "required": ["path", "content"] }
```

Behavior:
- Resolve path.
- Create parent directories if missing (`fs::create_dir_all`).
- Write content as UTF-8.
- Return `ContentBlock::Text` with `→ Wrote {path} ({n} bytes)`.

### `Edit`

In-place string replacement. Input:

```json
{ "type": "object", "properties": {
  "path": { "type": "string" },
  "old_string": { "type": "string" },
  "new_string": { "type": "string" },
  "replace_all": { "type": "boolean", "description": "Default false (require single match)" }
}, "required": ["path", "old_string", "new_string"] }
```

Behavior:
- Read file.
- Count occurrences of `old_string`. If `replace_all == false` and count != 1 → error.
- Replace and write back.
- Return `ContentBlock::Text` with `→ Edited {path} ({n} replacement{s})`.

### `Bash`

Execute a shell command. Input:

```json
{ "type": "object", "properties": {
  "command": { "type": "string" },
  "timeout_seconds": { "type": "integer", "description": "Default 60", "minimum": 1, "maximum": 600 },
  "cwd": { "type": "string", "description": "Working directory (relative to workspace root); default = workspace root" }
}, "required": ["command"] }
```

Behavior:
- Spawn `/bin/sh -c "{command}"` in the configured cwd.
- Capture stdout, stderr, exit code.
- Enforce timeout via `tokio::time::timeout` + kill the child process on timeout.
- Cancellation: honor `ToolContext.cancel` — kill the process if cancelled.
- Return a single `ContentBlock::Text` with this format:
  ```
  → Bash command: {command}
  → Exit code: {code}
  → Stdout:
  {stdout, truncated to 30KB with notice}
  → Stderr:
  {stderr, truncated to 30KB with notice}
  ```
- If timeout fires: return error with the partial stdout/stderr captured before kill.

### `Glob`

Filesystem glob pattern matching. Input:

```json
{ "type": "object", "properties": {
  "pattern": { "type": "string", "description": "Glob pattern (e.g., '**/*.rs')" },
  "path": { "type": "string", "description": "Root for the glob (default: workspace root)" }
}, "required": ["pattern"] }
```

Behavior:
- Use the `glob` crate.
- Return matching paths (relative to workspace root) as `ContentBlock::Text`, one path per line.
- Cap at 200 matches (return notice if truncated).
- Ignore hidden files / `.git/` / `target/` by default (use the `ignore` crate for proper gitignore-aware walking, or a simple opt-out).

### `Grep`

Ripgrep-style content search. Input:

```json
{ "type": "object", "properties": {
  "pattern": { "type": "string", "description": "Regex pattern" },
  "path": { "type": "string", "description": "Search root (default: workspace root)" },
  "include": { "type": "string", "description": "Glob filter for files to search (e.g., '*.rs')" },
  "max_matches": { "type": "integer", "description": "Default 100" }
}, "required": ["pattern"] }
```

Behavior:
- Use the `grep` + `ignore` crates (the same library underpinning ripgrep) for in-process search.
- Return matches as `ContentBlock::Text`, formatted `{path}:{line_number}:{line_content}`, one per line.
- Cap at 100 matches by default (configurable up to 500).
- Honor gitignore by default.

## `WorkspaceRoot`

```rust
#[derive(Debug, Clone)]
pub struct WorkspaceRoot {
    root: PathBuf,
    restrict_to_root: bool,
}

impl WorkspaceRoot {
    pub fn new(root: impl Into<PathBuf>) -> Self;
    pub fn current_dir() -> Result<Self, std::io::Error>;
    pub fn restricted(mut self) -> Self;  // refuse to operate outside root
    pub fn resolve(&self, input: &str) -> Result<PathBuf, ToolError>;
    pub fn relativize(&self, abs: &Path) -> PathBuf;
    pub fn root(&self) -> &Path;
}
```

`resolve` handles:
- Absolute paths: passthrough (or error if `restrict_to_root`).
- Relative paths: join with root, then canonicalize.
- If `restrict_to_root == true`: verify the resolved path starts with `root`; reject otherwise with `ToolError::InvalidInput`.

## Crate structure

```
crates/caliban-tools-builtin/
├── Cargo.toml
└── src/
    ├── lib.rs            re-exports
    ├── workspace.rs      WorkspaceRoot
    ├── read.rs           ReadTool
    ├── write.rs          WriteTool
    ├── edit.rs           EditTool
    ├── bash.rs           BashTool
    ├── glob_.rs          GlobTool (named with trailing underscore to avoid clash with std::glob if any)
    ├── grep.rs           GrepTool
    └── tests/
        └── (inline #[cfg(test)] mod tests per file)
```

**Cargo deps:**
- `caliban-agent-core` (path)
- `caliban-provider` (path; for `ContentBlock`)
- `async-trait`, `serde`, `serde_json`, `thiserror`
- `tokio` (full features for process + time)
- `tokio-util` (CancellationToken)
- `globset` (or `glob`) — for the Glob tool
- `ignore` — for gitignore-aware walking (used by both Glob and Grep)
- `grep-regex`, `grep-searcher` — ripgrep's library crates for content search
- Dev: `tempfile` for hermetic file tests

## Testing strategy

Per-tool unit tests using `tempfile::TempDir`:
- Read: file exists, empty file, missing file, binary file (error), large file (limit triggers).
- Write: new file, overwrite existing, missing parent dir (auto-created).
- Edit: single match (success), no match (error), multiple matches without replace_all (error), replace_all (success).
- Bash: simple `echo hi`, exit code propagation, timeout fires, cancellation kills process.
- Glob: pattern matches files in temp dir; pattern with `**/`; gitignore respected.
- Grep: pattern finds expected lines; include filter; gitignore respected; multi-file results.
- WorkspaceRoot: relative resolves correctly; absolute passthrough; restricted rejects outside-root.

Target ~25-30 tests for D.

## Acceptance criteria

- `crates/caliban-tools-builtin` exists; workspace member added.
- `cargo build --workspace` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `cargo test --workspace` passes — at least 25 new tests.
- All six tool structs implement `Tool + Send + Sync`.
- `Box<dyn Tool + Send + Sync>` compiles for each.
- The README example from C now works with real tools (registry actually populated).
- One ADR (0010) capturing the workspace-root + restrict-vs-permissive model.
- README updated with a sentence about the tool set.

## Risks

- **Bash timeout race**: killing a child process under tokio is non-trivial across platforms. We use `tokio::process::Child::kill().await` which on Unix sends SIGTERM. If the process ignores SIGTERM, we'd hang on `wait()`. Mitigation: use `kill()` then `tokio::time::timeout(Duration::from_secs(5), child.wait()).await`; if THAT times out, log and accept the orphan.
- **Grep crate API churn**: `grep-searcher` 0.1.x has had some API tweaks. Mitigation: pin a specific version; cover with a unit test that verifies the crate version is consumable.
- **WorkspaceRoot canonicalization** on macOS: symlinks (e.g., `/var` → `/private/var`) can cause `canonicalize` to return a different prefix. Mitigation: canonicalize the root once at construction; resolve against that.
- **Path attacks via `..`**: in restricted mode, `resolve` canonicalizes BEFORE the prefix check. Path traversal `../` is normalized away. Verified by tests.
