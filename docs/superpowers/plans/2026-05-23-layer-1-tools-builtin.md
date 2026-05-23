# Layer 1 / D (Tools-Builtin) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** Ship `caliban-tools-builtin` — one crate with six `Tool` impls (Read, Write, Edit, Bash, Glob, Grep) + `WorkspaceRoot` + tests.

**Architecture:** Each tool is a struct holding `Arc<WorkspaceRoot>`. Tools implement `caliban_agent_core::Tool` returning `Vec<ContentBlock>`. `WorkspaceRoot` resolves paths and optionally restricts to root subtree.

**Tech Stack:** Rust 1.85.0, tokio (process + time + fs), `globset` (Glob), `ignore` (walking), `grep-regex` + `grep-searcher` (content search). Dev: `tempfile`.

**Spec:** [`docs/superpowers/specs/2026-05-23-layer-1-tools-builtin-design.md`](../specs/2026-05-23-layer-1-tools-builtin-design.md)

---

## File Structure

```
crates/caliban-tools-builtin/
├── Cargo.toml                Task 1
└── src/
    ├── lib.rs                Task 1
    ├── workspace.rs          Task 1
    ├── read.rs               Task 2
    ├── write.rs              Task 3
    ├── edit.rs               Task 4
    ├── bash.rs               Task 5
    ├── glob_.rs              Task 6
    └── grep.rs               Task 7
adrs/
└── 0010-workspace-root.md    Task 8
README.md                     Task 8 (modified)
adrs/README.md                Task 8 (modified — index)
Cargo.toml                    Task 1 (modified — workspace member + deps)
```

Tools register tests inline (`#[cfg(test)] mod tests` at the end of each file) so each task's tests live next to the code being tested.

---

## Task 1: Crate skeleton + `WorkspaceRoot`

**Files:**
- Modify: root `Cargo.toml`
- Create: `crates/caliban-tools-builtin/Cargo.toml`
- Create: `crates/caliban-tools-builtin/src/lib.rs`
- Create: `crates/caliban-tools-builtin/src/workspace.rs`

- [ ] **Step 1: Root Cargo.toml — add workspace member + deps**

Add `"crates/caliban-tools-builtin"` to workspace members.

Add to `[workspace.dependencies]`:
```toml
globset       = "0.4"
ignore        = "0.4"
grep-regex    = "0.1"
grep-searcher = "0.1"
grep-matcher  = "0.1"
tempfile      = "3"
```

- [ ] **Step 2: Crate Cargo.toml**

```toml
[package]
name        = "caliban-tools-builtin"
version     = "0.0.0"
description = "Built-in tools (Read/Write/Edit/Bash/Glob/Grep) for the caliban agent harness"
edition.workspace      = true
license.workspace      = true
authors.workspace      = true
rust-version.workspace = true
publish     = false

[dependencies]
caliban-agent-core = { path = "../caliban-agent-core" }
caliban-provider   = { path = "../caliban-provider" }
async-trait        = { workspace = true }
serde              = { workspace = true }
serde_json         = { workspace = true }
thiserror          = { workspace = true }
tokio              = { workspace = true, features = ["full"] }
tokio-util         = { workspace = true }
tracing            = { workspace = true }
globset            = { workspace = true }
ignore             = { workspace = true }
grep-regex         = { workspace = true }
grep-searcher      = { workspace = true }
grep-matcher       = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
tokio    = { workspace = true, features = ["macros", "rt-multi-thread"] }

[lints]
workspace = true
```

- [ ] **Step 3: `src/workspace.rs`**

```rust
//! WorkspaceRoot — resolves and optionally restricts paths for built-in tools.

use std::path::{Path, PathBuf};

use caliban_agent_core::ToolError;

/// Path resolver for built-in tools.
#[derive(Debug, Clone)]
pub struct WorkspaceRoot {
    root: PathBuf,
    restrict_to_root: bool,
}

impl WorkspaceRoot {
    /// Construct from an absolute (canonicalized) root path.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        Self { root, restrict_to_root: false }
    }

    /// Construct from the current working directory.
    ///
    /// # Errors
    /// Returns an io::Error if the cwd cannot be obtained.
    pub fn current_dir() -> std::io::Result<Self> {
        let cwd = std::env::current_dir()?;
        Ok(Self::new(cwd))
    }

    /// Mark this root as restricted; subsequent `resolve` calls will reject
    /// paths outside the root.
    #[must_use]
    pub fn restricted(mut self) -> Self {
        self.restrict_to_root = true;
        self
    }

    /// Get the canonical root path.
    #[must_use]
    pub fn root(&self) -> &Path { &self.root }

    /// Whether resolution rejects out-of-root paths.
    #[must_use]
    pub fn is_restricted(&self) -> bool { self.restrict_to_root }

    /// Resolve an input string into an absolute path.
    ///
    /// Relative paths are joined with the root; absolute paths pass through
    /// (or are rejected in restricted mode if outside root).
    ///
    /// # Errors
    /// Returns `ToolError::InvalidInput` if the path is empty or, in restricted mode,
    /// if the resolved path is outside the workspace root.
    pub fn resolve(&self, input: &str) -> Result<PathBuf, ToolError> {
        if input.is_empty() {
            return Err(ToolError::invalid_input("empty path"));
        }
        let candidate = PathBuf::from(input);
        let abs = if candidate.is_absolute() {
            candidate
        } else {
            self.root.join(&candidate)
        };
        // Canonicalize parent (file may not exist yet for Write tool).
        let canon = canonicalize_existing_ancestor(&abs);
        if self.restrict_to_root && !canon.starts_with(&self.root) {
            return Err(ToolError::invalid_input(format!(
                "path {} is outside workspace root {}",
                canon.display(),
                self.root.display(),
            )));
        }
        Ok(canon)
    }

    /// Make an absolute path relative to the workspace root if it lies within;
    /// otherwise return the input unchanged.
    #[must_use]
    pub fn relativize(&self, abs: &Path) -> PathBuf {
        abs.strip_prefix(&self.root).map_or_else(|_| abs.to_path_buf(), Path::to_path_buf)
    }
}

/// Canonicalize as much of the path as exists, then append the rest. This
/// lets us check restriction even for paths that don't yet exist.
fn canonicalize_existing_ancestor(p: &Path) -> PathBuf {
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    let mut cur = p;
    loop {
        if let Ok(canon) = std::fs::canonicalize(cur) {
            let mut full = canon;
            for seg in tail.iter().rev() {
                full.push(seg);
            }
            return full;
        }
        match (cur.file_name(), cur.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name);
                cur = parent;
            }
            _ => return p.to_path_buf(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn resolve_relative() {
        let tmp = TempDir::new().unwrap();
        let root = WorkspaceRoot::new(tmp.path());
        let resolved = root.resolve("foo.txt").unwrap();
        assert!(resolved.starts_with(root.root()));
    }

    #[test]
    fn resolve_absolute_unrestricted() {
        let tmp = TempDir::new().unwrap();
        let root = WorkspaceRoot::new(tmp.path());
        let resolved = root.resolve("/tmp").unwrap();
        // /tmp may canonicalize differently on macOS, but resolution should succeed.
        let _ = resolved;
    }

    #[test]
    fn restricted_rejects_outside() {
        let tmp = TempDir::new().unwrap();
        let root = WorkspaceRoot::new(tmp.path()).restricted();
        let err = root.resolve("/etc/passwd").unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[test]
    fn restricted_allows_inside() {
        let tmp = TempDir::new().unwrap();
        let root = WorkspaceRoot::new(tmp.path()).restricted();
        let resolved = root.resolve("foo.txt").unwrap();
        assert!(resolved.starts_with(root.root()));
    }

    #[test]
    fn restricted_rejects_traversal() {
        let tmp = TempDir::new().unwrap();
        let inner = tmp.path().join("inner");
        std::fs::create_dir_all(&inner).unwrap();
        let root = WorkspaceRoot::new(&inner).restricted();
        let err = root.resolve("../escape.txt").unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[test]
    fn empty_path_errors() {
        let tmp = TempDir::new().unwrap();
        let root = WorkspaceRoot::new(tmp.path());
        let err = root.resolve("").unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }
}
```

- [ ] **Step 4: `src/lib.rs`**

```rust
//! Built-in tools for the caliban agent harness.
//!
//! Each tool implements `caliban_agent_core::Tool` with a JSON Schema for its
//! input. All tools share a `WorkspaceRoot` for path resolution.

pub mod workspace;

pub use workspace::WorkspaceRoot;
```

- [ ] **Step 5: Build + test + commit**

```bash
cargo build  -p caliban-tools-builtin
cargo test   -p caliban-tools-builtin
cargo clippy -p caliban-tools-builtin --all-targets -- -D warnings
cargo fmt --all -- --check
```

```bash
git add Cargo.toml crates/caliban-tools-builtin/
git commit -m "$(cat <<'EOF'
feat(tools-builtin): crate skeleton + WorkspaceRoot

Adds the caliban-tools-builtin crate scaffold and WorkspaceRoot — the
path-resolver every tool uses. Relative paths resolve against the root;
absolute paths pass through. Restricted mode (opt-in) rejects out-of-
root paths AFTER canonicalization (defeats `../` traversal).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: ReadTool

**Files:**
- Create: `crates/caliban-tools-builtin/src/read.rs`
- Modify: `crates/caliban-tools-builtin/src/lib.rs`

- [ ] **Step 1: `src/read.rs`**

```rust
//! Read tool — read a file's text contents.

use std::sync::Arc;
use std::sync::OnceLock;

use async_trait::async_trait;
use caliban_agent_core::{Tool, ToolContext, ToolError};
use caliban_provider::{ContentBlock, TextBlock};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::workspace::WorkspaceRoot;

const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
const DEFAULT_LIMIT: usize = 2000;

/// File reader tool.
#[derive(Debug)]
pub struct ReadTool {
    root: Arc<WorkspaceRoot>,
    schema: OnceLock<Value>,
}

impl ReadTool {
    /// Construct a Read tool using the given workspace root.
    #[must_use]
    pub fn new(root: WorkspaceRoot) -> Self {
        Self { root: Arc::new(root), schema: OnceLock::new() }
    }
}

#[derive(Debug, Deserialize)]
struct ReadInput {
    path: String,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    offset: Option<usize>,
}

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str { "Read" }

    fn description(&self) -> &str {
        "Read a UTF-8 text file. Returns the file's contents prefixed with a header line. Use offset+limit to read large files in chunks."
    }

    fn input_schema(&self) -> &Value {
        self.schema.get_or_init(|| json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to read (relative to workspace root or absolute)" },
                "limit": { "type": "integer", "description": "Maximum number of lines to return (default 2000)", "minimum": 1 },
                "offset": { "type": "integer", "description": "1-indexed line to start at (default 1)", "minimum": 1 }
            },
            "required": ["path"]
        }))
    }

    async fn invoke(
        &self,
        input: Value,
        _cx: ToolContext,
    ) -> Result<Vec<ContentBlock>, ToolError> {
        let parsed: ReadInput = serde_json::from_value(input)
            .map_err(|e| ToolError::invalid_input(format!("invalid input: {e}")))?;

        let path = self.root.resolve(&parsed.path)?;
        let metadata = tokio::fs::metadata(&path).await
            .map_err(ToolError::execution)?;
        if metadata.len() > MAX_FILE_BYTES {
            return Err(ToolError::execution(std::io::Error::other(format!(
                "file {} is {} bytes, larger than 5MB max; use offset+limit",
                path.display(),
                metadata.len(),
            ))));
        }

        let content = tokio::fs::read_to_string(&path).await
            .map_err(ToolError::execution)?;

        let total = content.lines().count();
        let offset = parsed.offset.unwrap_or(1).saturating_sub(1);
        let limit = parsed.limit.unwrap_or(DEFAULT_LIMIT);
        let end = offset.saturating_add(limit).min(total);

        let chunk: String = content
            .lines()
            .skip(offset)
            .take(limit)
            .enumerate()
            .map(|(i, line)| format!("{:>5}  {}\n", offset + i + 1, line))
            .collect();

        let header = format!(
            "→ Read {}, lines {}-{} of {}\n\n",
            self.root.relativize(&path).display(),
            offset + 1,
            end,
            total,
        );

        Ok(vec![ContentBlock::Text(TextBlock {
            text: format!("{header}{chunk}"),
            cache_control: None,
        })])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    fn ctx() -> ToolContext {
        ToolContext { tool_use_id: "t1".into(), cancel: CancellationToken::new() }
    }

    #[tokio::test]
    async fn reads_existing_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("foo.txt");
        std::fs::write(&path, "hello\nworld\n").unwrap();
        let tool = ReadTool::new(WorkspaceRoot::new(tmp.path()));
        let out = tool.invoke(json!({"path": "foo.txt"}), ctx()).await.unwrap();
        let ContentBlock::Text(t) = &out[0] else { panic!() };
        assert!(t.text.contains("hello"));
        assert!(t.text.contains("world"));
    }

    #[tokio::test]
    async fn missing_file_errors() {
        let tmp = TempDir::new().unwrap();
        let tool = ReadTool::new(WorkspaceRoot::new(tmp.path()));
        let err = tool.invoke(json!({"path": "nope.txt"}), ctx()).await.unwrap_err();
        assert!(matches!(err, ToolError::Execution(_)));
    }

    #[tokio::test]
    async fn empty_file_succeeds() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("empty.txt");
        std::fs::write(&path, "").unwrap();
        let tool = ReadTool::new(WorkspaceRoot::new(tmp.path()));
        let out = tool.invoke(json!({"path": "empty.txt"}), ctx()).await.unwrap();
        let ContentBlock::Text(t) = &out[0] else { panic!() };
        assert!(t.text.contains("lines 1-0 of 0") || t.text.contains("0 of 0"));
    }

    #[tokio::test]
    async fn offset_and_limit() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("many.txt");
        std::fs::write(&path, "a\nb\nc\nd\ne\n").unwrap();
        let tool = ReadTool::new(WorkspaceRoot::new(tmp.path()));
        let out = tool.invoke(json!({"path": "many.txt", "offset": 2, "limit": 2}), ctx()).await.unwrap();
        let ContentBlock::Text(t) = &out[0] else { panic!() };
        assert!(t.text.contains("b"));
        assert!(t.text.contains("c"));
        assert!(!t.text.contains("d") || t.text.matches('d').count() == 0);  // d shouldn't be in content (might be in header)
    }
}
```

- [ ] **Step 2: Modify `lib.rs`** — add `pub mod read; pub use read::ReadTool;`.

- [ ] **Step 3: Build + test + commit**

```bash
cargo test  -p caliban-tools-builtin
cargo clippy -p caliban-tools-builtin --all-targets -- -D warnings
git add crates/caliban-tools-builtin/
git commit -m "feat(tools-builtin): Read tool

Reads UTF-8 text files. Supports offset+limit for large files (line-
indexed, 1-based). Enforces a 5MB file-size cap.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: WriteTool

**Files:**
- Create: `crates/caliban-tools-builtin/src/write.rs`
- Modify: `crates/caliban-tools-builtin/src/lib.rs`

Implement following the same pattern as Read. Schema: `path`, `content` (both required strings). Behavior:
1. Resolve path.
2. `tokio::fs::create_dir_all(parent_of_path)` if parent doesn't exist.
3. `tokio::fs::write(path, content)`.
4. Return one Text block `→ Wrote {relativized path} ({n} bytes)`.

Tests: write new file; overwrite existing; missing-parent autocreate; error on permission denied (use a mode-0 dir for the test).

Commit: `feat(tools-builtin): Write tool`.

---

## Task 4: EditTool

**Files:** `src/edit.rs` + lib.rs update.

Schema: `path` (string), `old_string` (string), `new_string` (string), `replace_all` (boolean, default false).

Behavior:
1. Resolve + read file.
2. Count occurrences of `old_string`.
3. If `replace_all == false` and count != 1 → error with `"old_string matched {count} times; expected exactly one (use replace_all=true to replace all)"`.
4. If count == 0 → error with `"old_string not found in file"`.
5. Replace occurrences (`str::replace` if replace_all, else `str::replacen(old, new, 1)`).
6. Write back.
7. Return Text block `→ Edited {path} ({n} replacement{s})`.

Tests: single match success; multiple matches without replace_all errors; no match errors; replace_all replaces multiple.

Commit: `feat(tools-builtin): Edit tool`.

---

## Task 5: BashTool

**Files:** `src/bash.rs` + lib.rs update.

This is the trickiest. Schema: `command` (string, required), `timeout_seconds` (integer, default 60, min 1, max 600), `cwd` (string, optional — relative to workspace root).

Implementation outline:

```rust
async fn invoke(&self, input: Value, cx: ToolContext) -> Result<Vec<ContentBlock>, ToolError> {
    let parsed: BashInput = serde_json::from_value(input).map_err(...)?;
    let timeout = Duration::from_secs(parsed.timeout_seconds.unwrap_or(60));
    let cwd = match parsed.cwd {
        Some(c) => self.root.resolve(&c)?,
        None => self.root.root().to_path_buf(),
    };

    let mut child = tokio::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(&parsed.command)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(ToolError::execution)?;

    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");

    let read_stdout = read_capped(stdout);
    let read_stderr = read_capped(stderr);

    let run = async {
        let (out, err, status) = tokio::join!(read_stdout, read_stderr, child.wait());
        (out, err, status)
    };

    let outcome = tokio::select! {
        outcome = run => Ok(outcome),
        () = tokio::time::sleep(timeout) => {
            child.start_kill().ok();
            Err("timeout")
        }
        () = cx.cancel.cancelled() => {
            child.start_kill().ok();
            return Err(ToolError::Cancelled);
        }
    };

    // Build the formatted output Text block per spec.
}

async fn read_capped(mut reader: impl AsyncRead + Unpin) -> Result<String, std::io::Error> {
    const CAP: usize = 30 * 1024;
    let mut buf = Vec::with_capacity(CAP);
    let mut chunk = [0u8; 4096];
    while buf.len() < CAP {
        let n = reader.read(&mut chunk).await?;
        if n == 0 { break; }
        buf.extend_from_slice(&chunk[..n.min(CAP - buf.len())]);
        if n + buf.len() >= CAP { break; }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}
```

(The implementer should refine this — the structure is the key idea.)

Tests: `echo hi` returns "hi" in stdout; exit code propagation (`exit 7` returns 7 in the formatted output); timeout fires (`sleep 5` with timeout=1 returns timeout error); cancellation kills process (test by spawning a sleep 30, cancel after 100ms, verify error within 500ms).

Commit: `feat(tools-builtin): Bash tool (timeout, cancellation, output capture)`.

---

## Task 6: GlobTool

**Files:** `src/glob_.rs` + lib.rs update.

Schema: `pattern` (string, required), `path` (string, optional — defaults to workspace root).

Behavior:
1. Use the `ignore::Walk` builder with `.gitignore` respected by default.
2. Filter by pattern using `globset::GlobBuilder::new(&pattern).literal_separator(true).build()`.
3. Cap at 200 results.
4. Return one Text block with paths (relative to workspace root) one per line. If truncated, add a notice line at the end.

Tests: pattern matches expected files in tempdir; `**/*.rs` semantics; `.gitignore` honored.

Commit: `feat(tools-builtin): Glob tool`.

---

## Task 7: GrepTool

**Files:** `src/grep.rs` + lib.rs update.

Schema: `pattern` (string), `path` (string, optional), `include` (string, glob filter, optional), `max_matches` (integer, default 100, max 500).

Implementation uses `grep-regex` (for the regex) + `grep-searcher` (for the actual searching) + `ignore::Walk` (for filesystem walking with gitignore).

Outline:
```rust
let matcher = RegexMatcher::new(&pattern).map_err(ToolError::execution)?;
let mut searcher = SearcherBuilder::new()
    .line_number(true)
    .build();

let walk = WalkBuilder::new(search_root).build();
let mut results = Vec::new();
for entry in walk {
    let entry = entry?;
    if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) { continue; }
    if let Some(inc) = &include_glob {
        if !inc.is_match(entry.path()) { continue; }
    }
    searcher.search_path(&matcher, entry.path(), &mut sink_that_collects_matches)?;
    if results.len() >= max_matches { break; }
}
```

Tests: pattern finds a known line in tempdir files; gitignore honored; include filter applied.

Commit: `feat(tools-builtin): Grep tool (ripgrep-based)`.

---

## Task 8: ADR 0010 + README update

ADR 0010 — workspace-root path resolution + restricted mode.

README: mention the tool set in the project status.

```bash
git add adrs/ README.md
git commit -m "docs: ADR 0010 + README update for caliban-tools-builtin"
```

---

## Self-Review

Coverage: All 6 tools + WorkspaceRoot + 25+ tests. Each task is self-contained (single tool file + tests). The trickiest task (Bash) has explicit guidance on the tokio select/timeout/cancel pattern. Type consistency: all tools take `WorkspaceRoot` in `::new`, expose `&str` name/description, return `Result<Vec<ContentBlock>, ToolError>`.

Risks documented in the spec apply. No type/method-signature divergence between tasks.
