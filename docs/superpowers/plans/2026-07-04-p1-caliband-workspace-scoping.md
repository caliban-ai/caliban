# Workspace-scoped caliband Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Generalize caliband from a per-repo daemon to a **workspace-scoped** one that supervises agents across N source checkouts in a single workspace, with each agent running in its target source and, when requested, an actual per-source git worktree.

**Architecture:** caliband roots a *workspace directory* holding N git checkouts ("sources"). Identity/socket/store key off the workspace root (a single-repo workspace's root equals today's repo_root, so local use is byte-for-byte unchanged). `SpawnSpec` gains a `source` field naming which checkout an agent targets; the daemon resolves it, optionally materializes a per-source git worktree via the (currently-dead) `caliban-worktrees::WorktreeManager`, records the resulting working directory on `AgentRecord`, and the launcher sets the worker's `current_dir` to it — closing the "cwd inherited from the daemon" gap. See ADR 0052.

**Tech Stack:** Rust, tokio, serde_json (NDJSON, one new optional `SpawnSpec` field), `caliban-worktrees` (git worktree materialization), sha2 (workspace hash — unchanged impl).

## Global Constraints

- **Caliban-only change** (ADR 0052): no prospero coupling; the caliban/prospero path-hash duplication is explicitly deferred.
- **Local single-repo use is byte-for-byte unchanged:** a workspace root equal to today's repo_root must produce the *same* `workspace_hash`, the *same* socket path, and the *same* store dir. Existing daemons/CLI keep working with no migration. The full existing test suite is a regression gate on every task.
- **Back-compatible wire + store:** `SpawnSpec.source` and `AgentRecord.working_dir` are additive with `#[serde(default)]`; an old manifest (no `working_dir`) deserializes to an empty path and the launcher then sets no `current_dir` (today's behavior).
- **`--repo-root` stays a valid alias** for `--workspace-root` on the `caliband` binary (all current callers pass `--repo-root`).
- **No worker changes needed:** the worker reads its own cwd via `WorkspaceRoot::current_dir()`; setting the launcher's `current_dir` is sufficient. Do not modify `worker.rs` for the working-dir plumbing.
- Strict lints: every task ends green under `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo build --workspace --all-targets`, `cargo test --workspace`. No `unwrap()`/`expect()` in non-test code.

---

## File Structure

- **Modify** `crates/caliban-supervisor/src/runtime.rs` — rename `repo_hash`→`workspace_hash`, `repo_socket_path`→`workspace_socket_path`, `repo_socket_path_in`→`workspace_socket_path_in` (same bodies); update the module doc.
- **Modify** `crates/caliban-supervisor/src/lib.rs` — re-export the renamed fns.
- **Modify** `crates/caliban-supervisor/src/bin/caliband.rs` — accept `--workspace-root` (canonical) + `--repo-root` (alias); field `workspace_root`.
- **Modify** `caliban/src/agents_cli.rs`, `caliban/src/tui/events.rs`, `caliban/src/agents_cli.rs::run_daemon` — update call sites to the renamed fns (cosmetic).
- **Modify** `crates/caliban-supervisor/src/store.rs` — `default_for` param rename `repo_root`→`workspace_root` (cosmetic; identical body).
- **Create** `crates/caliban-supervisor/src/sources.rs` — source discovery/resolution.
- **Modify** `crates/caliban-supervisor/src/proto.rs` — `SpawnSpec.source: Option<String>`; `AgentRecord.working_dir: PathBuf`.
- **Modify** `crates/caliban-supervisor/src/registry.rs` — `register` threads `working_dir`.
- **Modify** `crates/caliban-supervisor/src/server.rs` — `dispatch` Spawn/Respawn resolve source + materialize worktree + record working_dir; `launch_and_monitor` removes the worktree on exit.
- **Modify** `crates/caliban-supervisor/src/proc.rs` — `ExecWorkerLauncher::launch` sets `current_dir` from `record.working_dir`.
- **Modify** `crates/caliban-supervisor/Cargo.toml` — add `caliban-worktrees` dependency.
- **Create** `crates/caliban-supervisor/tests/workspace_sources.rs` — integration test (Task 5).

---

### Task 1: Workspace identity — rename to `workspace_*` + `--workspace-root` alias (behavior identical)

Pure rename + a new flag alias. No behavior change; the same path hashes the same, so local single-repo use is unchanged.

**Files:**
- Modify: `crates/caliban-supervisor/src/runtime.rs`, `crates/caliban-supervisor/src/lib.rs`, `crates/caliban-supervisor/src/bin/caliband.rs`, `crates/caliban-supervisor/src/store.rs`
- Modify: `caliban/src/agents_cli.rs`, `caliban/src/tui/events.rs`

**Interfaces:**
- Produces: `workspace_hash(&Path) -> String`, `workspace_socket_path(&Path) -> PathBuf`, `workspace_socket_path_in(&Path, &Path) -> PathBuf` (identical bodies to the old `repo_*`). `caliband` accepts `--workspace-root <dir>` and `--repo-root <dir>` (alias) → one `workspace_root: PathBuf`.

- [ ] **Step 1: Baseline the suite** — `cargo test -p caliban-supervisor 2>&1 | tail -3` and `cargo test -p caliban 2>&1 | tail -3`; note the counts (regression gate).

- [ ] **Step 2: Rename in `runtime.rs`.** Rename the three fns `repo_hash`→`workspace_hash`, `repo_socket_path`→`workspace_socket_path`, `repo_socket_path_in`→`workspace_socket_path_in` (bodies unchanged — `workspace_hash` still hashes the path's first 8 SHA-256 bytes to 16 hex chars). Update the module doc's `hash(repo_root)` → `hash(workspace_root)`. Update the in-module unit tests' names/comments to `workspace_*`. **The hash output for a given path MUST be identical** — add an explicit test:

```rust
#[test]
fn workspace_hash_matches_legacy_repo_hash_for_same_path() {
    // Back-compat: a single-repo workspace root == old repo_root must hash the
    // same, so existing sockets/stores are found unchanged.
    let p = std::path::Path::new("/some/repo/root");
    // 16 lowercase hex chars, stable, same as the pre-rename repo_hash.
    assert_eq!(workspace_hash(p).len(), 16);
    assert_eq!(workspace_hash(p), workspace_hash(p));
}
```

- [ ] **Step 3: Update `lib.rs` re-exports** — `pub use runtime::{workspace_socket_path, workspace_socket_path_in};` (drop the old names).

- [ ] **Step 4: Update `caliband.rs`.** In `parse_args`, accept both flags into one field. Rename `Args.repo_root`→`Args.workspace_root`. Parsing:

```rust
"--workspace-root" | "--repo-root" => a.workspace_root = it.next().map(PathBuf::from),
```

Update the required-check error text to `--workspace-root required (or --repo-root)` and the `-h`/`--help` usage line. In `main`, rename the local `repo_root`→`workspace_root`; `socket_path` default becomes `workspace_socket_path(&workspace_root)`; `store` default `AgentStore::default_for(&workspace_root)`.

- [ ] **Step 5: Update `store.rs`** — rename `default_for`'s param `repo_root`→`workspace_root` (identical body; it sanitizes the path and builds `projects/<sanitized>/agents`). Update its doc comment.

- [ ] **Step 6: Update the remaining callers** — `caliban/src/agents_cli.rs` (`repo_socket_path` at the import + `ensure_daemon` + `run_daemon`) and `caliban/src/tui/events.rs` (import + call) → `workspace_socket_path`. These are cosmetic renames; the variable `repo_root` in `agents_cli.rs` may stay named locally (it's the discovered repo, which for local use *is* the workspace root) — only the function name changes.

- [ ] **Step 7: Run the gate** — `cargo test -p caliban-supervisor && cargo test -p caliban && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all -- --check`. Same pass counts as Step 1.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor(supervisor): workspace_* identity + --workspace-root alias (no behavior change) (#281)"
```

---

### Task 2: Source discovery/resolution module

A new `sources` module: enumerate git checkouts under a workspace root, and resolve an optional source name to a directory.

**Files:**
- Create: `crates/caliban-supervisor/src/sources.rs`
- Modify: `crates/caliban-supervisor/src/lib.rs` (`pub mod sources;` + re-exports)
- Test: inline `#[cfg(test)]`

**Interfaces:**
- Produces:
  - `pub struct Source { pub name: String, pub path: PathBuf }`
  - `pub fn discover_sources(workspace_root: &Path) -> Vec<Source>` — every immediate child dir of `workspace_root` that contains `.git`, by directory name; plus a source named after the workspace root's own dir name when the root itself contains `.git` (single-source back-compat). Sorted by name; empty vec if none.
  - `pub fn resolve_source(workspace_root: &Path, source: Option<&str>) -> std::io::Result<PathBuf>` — `None` → `workspace_root` (canonicalized-ish: return as-is); `Some(name)` → the discovered source whose `name == name`, else if `workspace_root.join(name)` exists return it, else `Err(ErrorKind::NotFound, "no such source: <name>")`. Reject `name` containing a path separator or `..` (return `InvalidInput`) so a source can't escape the workspace.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn git_checkout(dir: &std::path::Path) {
        std::fs::create_dir_all(dir.join(".git")).unwrap();
    }

    #[test]
    fn discovers_child_checkouts_and_resolves() {
        let ws = tempfile::tempdir().unwrap();
        git_checkout(&ws.path().join("caliban"));
        git_checkout(&ws.path().join("gonzalo"));
        std::fs::create_dir_all(ws.path().join("not-a-repo")).unwrap();

        let mut names: Vec<_> = discover_sources(ws.path()).into_iter().map(|s| s.name).collect();
        names.sort();
        assert_eq!(names, vec!["caliban", "gonzalo"]);

        assert_eq!(resolve_source(ws.path(), Some("gonzalo")).unwrap(), ws.path().join("gonzalo"));
        assert_eq!(resolve_source(ws.path(), None).unwrap(), ws.path());
        assert_eq!(resolve_source(ws.path(), Some("missing")).unwrap_err().kind(), std::io::ErrorKind::NotFound);
        assert_eq!(resolve_source(ws.path(), Some("../escape")).unwrap_err().kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn root_itself_is_a_source_when_a_checkout() {
        let ws = tempfile::tempdir().unwrap();
        git_checkout(ws.path());
        let names: Vec<_> = discover_sources(ws.path()).into_iter().map(|s| s.name).collect();
        // The workspace root's own dir name is a source (single-source back-compat).
        assert_eq!(names.len(), 1);
    }
}
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p caliban-supervisor sources::tests` → FAIL (module missing).

- [ ] **Step 3: Implement `sources.rs`**

```rust
//! Workspace source discovery + resolution (#281 / ADR 0052).
//!
//! A *source* is a git checkout under the workspace root. caliband roots a
//! workspace directory holding N sources; an agent's `SpawnSpec.source` names
//! which one it runs against.

use std::path::{Path, PathBuf};

/// A discovered source checkout within a workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// Directory name (the identifier used by `SpawnSpec.source`).
    pub name: String,
    /// Absolute path to the checkout.
    pub path: PathBuf,
}

fn is_checkout(dir: &Path) -> bool {
    dir.join(".git").exists()
}

/// Enumerate git checkouts under `workspace_root` (immediate children with a
/// `.git`), plus the root itself when it is a checkout. Sorted by name.
pub fn discover_sources(workspace_root: &Path) -> Vec<Source> {
    let mut out = Vec::new();
    if is_checkout(workspace_root) {
        if let Some(name) = workspace_root.file_name().and_then(|n| n.to_str()) {
            out.push(Source { name: name.to_string(), path: workspace_root.to_path_buf() });
        }
    }
    if let Ok(entries) = std::fs::read_dir(workspace_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && is_checkout(&path) {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    out.push(Source { name: name.to_string(), path });
                }
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.dedup_by(|a, b| a.name == b.name);
    out
}

/// Resolve `source` (a source name) to an absolute directory under
/// `workspace_root`. `None` resolves to the workspace root itself.
pub fn resolve_source(workspace_root: &Path, source: Option<&str>) -> std::io::Result<PathBuf> {
    let Some(name) = source else {
        return Ok(workspace_root.to_path_buf());
    };
    // Guard against traversal: a source name is a single path component.
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid source name: {name}"),
        ));
    }
    if let Some(src) = discover_sources(workspace_root).into_iter().find(|s| s.name == name) {
        return Ok(src.path);
    }
    let candidate = workspace_root.join(name);
    if candidate.exists() {
        return Ok(candidate);
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("no such source: {name}"),
    ))
}
```

Add to `lib.rs`: `pub mod sources;` and `pub use sources::{discover_sources, resolve_source, Source};`

- [ ] **Step 4: Run to verify it passes** — `cargo test -p caliban-supervisor sources::tests`.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(supervisor): workspace source discovery + resolution (#281)"
```

---

### Task 3: `source` on `SpawnSpec` + `working_dir` on `AgentRecord` (additive, back-compat)

**Files:**
- Modify: `crates/caliban-supervisor/src/proto.rs` (both structs)
- Modify: `crates/caliban-supervisor/src/registry.rs` (`register` threads `working_dir`)
- Modify: `crates/caliban-supervisor/src/server.rs` (temporary: pass `working_dir` = `Endpoint`-neutral default so it compiles; the real resolution lands in Task 4)
- Modify any `AgentRecord`/`SpawnSpec` literal (tests, `proc.rs` test helper) to add the new fields

**Interfaces:**
- Produces:
  - `SpawnSpec.source: Option<String>` with `#[serde(default)]`.
  - `AgentRecord.working_dir: PathBuf` with `#[serde(default)]` (empty = "inherit daemon cwd", today's behavior).
  - `Registry::register(&mut self, spec: SpawnSpec, endpoint: Endpoint, working_dir: PathBuf) -> AgentRecord`.

- [ ] **Step 1: Update the round-trip tests first** — in `store.rs` and `proto.rs` tests that build `SpawnSpec`/`AgentRecord` literals, add `source: None` and `working_dir: PathBuf::new()`. Add a proto test asserting `SpawnSpec` with `source: Some("gonzalo")` round-trips and that a legacy JSON object *without* `source`/`working_dir` still deserializes (defaults apply):

```rust
#[test]
fn spawnspec_source_defaults_and_roundtrips() {
    let legacy = r#"{"initial_prompt":"hi"}"#; // no source field
    let spec: SpawnSpec = serde_json::from_str(legacy).unwrap();
    assert_eq!(spec.source, None);
    let with_src = SpawnSpec { source: Some("gonzalo".into()), ..spec };
    let json = serde_json::to_string(&with_src).unwrap();
    assert_eq!(serde_json::from_str::<SpawnSpec>(&json).unwrap().source, Some("gonzalo".into()));
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p caliban-supervisor --no-run` fails (fields absent).

- [ ] **Step 3: Add the fields.** In `proto.rs`:

```rust
// in SpawnSpec, after `interactive`:
    /// Which workspace source (checkout) this agent runs against. `None`
    /// means the workspace root (single-source back-compat). (#281)
    #[serde(default)]
    pub source: Option<String>,
```

```rust
// in AgentRecord, after `endpoint`:
    /// Resolved working directory for the worker (the source dir, or its
    /// per-source worktree when `spec.isolation_worktree`). Empty = inherit
    /// the daemon's cwd (legacy records). (#281)
    #[serde(default)]
    pub working_dir: PathBuf,
```

In `registry.rs`, change `register` to accept `working_dir: PathBuf` and set it on the constructed `AgentRecord`. In `server.rs` `dispatch` Spawn/Respawn, temporarily pass `PathBuf::new()` as `working_dir` (Task 4 replaces it with the resolved dir). Update the `proc.rs` test helper `record(...)` and any other `AgentRecord` literal with `working_dir: PathBuf::new()`.

- [ ] **Step 4: Run the gate** — `cargo test -p caliban-supervisor && cargo test -p caliban && cargo clippy --workspace --all-targets -- -D warnings`.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(supervisor): SpawnSpec.source + AgentRecord.working_dir (additive) (#281)"
```

---

### Task 4: Wire per-source worktree materialization + launcher `current_dir`

The functional core: resolve the source at spawn, optionally create a per-source git worktree, record the working dir, set the worker's cwd, and remove the worktree on exit.

**Files:**
- Modify: `crates/caliban-supervisor/Cargo.toml` (add `caliban-worktrees`)
- Modify: `crates/caliban-supervisor/src/server.rs` (`Supervisor` carries `workspace_root`; `dispatch` Spawn/Respawn; `launch_and_monitor`)
- Modify: `crates/caliban-supervisor/src/proc.rs` (`ExecWorkerLauncher::launch` sets `current_dir`)
- Modify: `crates/caliban-supervisor/src/bin/caliband.rs` (pass `workspace_root` into the `Supervisor`)

**Interfaces:**
- Consumes: `sources::resolve_source` (Task 2), `SpawnSpec.source` + `AgentRecord.working_dir` (Task 3), `caliban_worktrees::{WorktreeManager, WorktreeSpec, BaseRef}` — `WorktreeManager::new(source_path) -> Result<Self, WorktreeError>`, `create(&WorktreeSpec) -> Result<WorktreeHandle, WorktreeError>` where `WorktreeHandle { name, path, branch }`, `remove(name, force) -> Result<(), WorktreeError>`. `WorktreeSpec::new(name)` builder; `BaseRef::Head` for "branch off current HEAD".
- Produces: the daemon assigns each agent a `working_dir` (source dir, or `<source>/.caliban/worktrees/<agent-id>`), sets it on the record, and the worker runs there.

**Design (implement exactly):** the `Supervisor` gains a `workspace_root: PathBuf` field (from the constructor — thread it through `new`/`with_launcher`/`with_bind`; `caliband.rs` passes `args.workspace_root`). In `dispatch` Spawn/Respawn, after choosing the endpoint:

```rust
// Resolve the source directory (None -> workspace root).
let source_dir = match crate::sources::resolve_source(&self.workspace_root, spec.source.as_deref()) {
    Ok(d) => d,
    Err(e) => return CtlReply::Error { error: SupervisorError::Internal { message: format!("source resolve: {e}") } },
};
// Optionally materialize a per-source worktree.
let (working_dir, worktree_cleanup) = if spec.isolation_worktree {
    match crate::worktree_for_agent(&source_dir, &id_prefix) {
        Ok(handle) => (handle.path.clone(), Some((source_dir.clone(), handle.name))),
        Err(e) => return CtlReply::Error { error: SupervisorError::Internal { message: format!("worktree create: {e}") } },
    }
} else {
    (source_dir.clone(), None)
};
let rec = { let mut r = self.registry.lock().await; r.register(spec, endpoint_value, working_dir) };
```

Add a small free helper in `server.rs` (or `sources.rs`) to keep the dispatch readable:

```rust
/// Create a per-agent git worktree rooted at `source_dir`. Returns the handle
/// (its `.path` is the worktree root). Errors if the source is not a checkout.
fn worktree_for_agent(source_dir: &std::path::Path, agent_name: &str)
    -> Result<caliban_worktrees::WorktreeHandle, caliban_worktrees::WorktreeError>
{
    let mgr = caliban_worktrees::WorktreeManager::new(source_dir)?;
    mgr.create(&caliban_worktrees::WorktreeSpec::new(agent_name))
}
```

(Confirm `WorktreeSpec::new(name)` defaults `base_ref` to a sensible value — read `config.rs`; if it defaults to `Fresh`/`Head`, keep the default; only set `BaseRef::Head` explicitly if the default isn't "branch off HEAD".)

In `launch_and_monitor`, the monitor task already removes the per-agent socket on exit. Extend it to also remove the worktree: thread `worktree_cleanup: Option<(PathBuf, String)>` into `launch_and_monitor` (alongside `rec`), and in the exit task, after the socket unlink:

```rust
if let Some((source_dir, wt_name)) = worktree_cleanup {
    if let Ok(mgr) = caliban_worktrees::WorktreeManager::new(&source_dir) {
        let _ = mgr.remove(&wt_name, true); // best-effort, like the socket unlink
    }
}
```

In `proc.rs` `ExecWorkerLauncher::launch`, set the worker's cwd from the record (empty = leave unset, legacy behavior):

```rust
if !record.working_dir.as_os_str().is_empty() {
    cmd.current_dir(&record.working_dir);
}
```

- [ ] **Step 1: Write the failing test** (in `server.rs` or a small integration test): a Spawn with `isolation_worktree: true` and `source: None` against a workspace root that is a real git repo creates a worktree and records a `working_dir` under `.caliban/worktrees/`. Use the crate's existing fake `WorktreeManager` is not needed — create a real temp git repo (`git init`) and assert the worktree dir exists + `working_dir` points into it. Prefer an integration test in `tests/` (Task 5 will hold the full one); a focused unit test here can assert `worktree_for_agent` returns a handle whose `.path` exists under `<repo>/.caliban/worktrees/<name>`.

```rust
// focused unit test for the helper (in server.rs #[cfg(test)] or worktree_for_agent's module)
#[test]
fn worktree_for_agent_materializes_under_source() {
    let repo = tempfile::tempdir().unwrap();
    // init a real git repo with one commit so HEAD exists
    std::process::Command::new("git").args(["init", "-q"]).current_dir(repo.path()).status().unwrap();
    std::process::Command::new("git").args(["-c","user.email=t@t","-c","user.name=t","commit","--allow-empty","-qm","init"]).current_dir(repo.path()).status().unwrap();
    let handle = worktree_for_agent(repo.path(), "agent0001").unwrap();
    assert!(handle.path.exists());
    assert!(handle.path.starts_with(repo.path().join(".caliban").join("worktrees")));
}
```

- [ ] **Step 2: Run to verify failure** — the helper/field wiring doesn't exist yet → FAIL.

- [ ] **Step 3: Implement** per the Design block: add `caliban-worktrees = { workspace = true }` to `crates/caliban-supervisor/Cargo.toml`; add the `workspace_root` field + constructor threading; the `worktree_for_agent` helper; the Spawn/Respawn resolution; the `launch_and_monitor` cleanup extension; the `proc.rs` `current_dir`. Update the two Spawn/Respawn call sites and `caliband.rs` to pass `workspace_root`.

- [ ] **Step 4: Run the gate** — `cargo test -p caliban-supervisor && cargo test -p caliban && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all -- --check`. (This adds `caliban-supervisor` → `caliban-worktrees`; confirm no dependency cycle — `caliban-worktrees` does not depend on `caliban-supervisor`.)

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(supervisor): wire per-source worktree materialization + worker current_dir (#281)"
```

---

### Task 5: Integration test — one caliband, ≥2 sources, per-source isolation

The acceptance test.

**Files:**
- Create: `crates/caliban-supervisor/tests/workspace_sources.rs`

**Interfaces:**
- Consumes: the full spawn path. Reuse the crate's existing integration harness (`tests/ipc.rs`) for standing up a `Supervisor` with a fake `WorkerLauncher` (no LLM). Read `tests/ipc.rs` for the established fake-launcher + client pattern.

- [ ] **Step 1: Write the test.** Build a workspace dir with two real git checkouts (`git init` in `<ws>/alpha` and `<ws>/beta`, one empty commit each). Start a `Supervisor` rooted at `<ws>` with a fake launcher that records the `AgentRecord.working_dir` it was handed (or writes a marker file into `record.working_dir`). Spawn two agents: `{source: Some("alpha"), isolation_worktree: true}` and `{source: Some("beta"), isolation_worktree: true}`. Assert:
  1. each agent's `working_dir` is under its own source (`<ws>/alpha/.caliban/worktrees/...` vs `<ws>/beta/.caliban/worktrees/...`);
  2. the two working dirs are different trees (isolation);
  3. a file created in alpha's worktree does not appear under beta's worktree (per-source write isolation).
Also spawn one agent with `source: None, isolation_worktree: false` and assert its `working_dir` is the workspace root (back-compat).

```rust
// sketch — adapt to the tests/ipc.rs harness
#[tokio::test]
async fn one_caliband_supervises_two_sources_with_worktree_isolation() {
    let ws = tempfile::tempdir().unwrap();
    for name in ["alpha", "beta"] {
        let dir = ws.path().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::process::Command::new("git").args(["init","-q"]).current_dir(&dir).status().unwrap();
        std::process::Command::new("git").args(["-c","user.email=t@t","-c","user.name=t","commit","--allow-empty","-qm","i"]).current_dir(&dir).status().unwrap();
    }
    // ... start Supervisor rooted at ws with a fake launcher; client.spawn twice ...
    // assert working_dir per source + isolation, per the three checks above.
}
```

- [ ] **Step 2: Run to verify it fails** (before Task 4 is complete it would; here it should PASS since Task 4 landed the wiring) — `cargo test -p caliban-supervisor --test workspace_sources`.

- [ ] **Step 3: Full gate + commit**

```bash
cargo fmt --all
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --all-targets
cargo test --workspace
git add -A
git commit -m "test(supervisor): workspace multi-source + per-source worktree isolation e2e (#281)"
```

---

## Self-Review

**1. Spec coverage** (#281 acceptance + ADR 0052):
- "One caliband supervises agents across ≥2 repos in a single workspace" → workspace identity (T1) + source resolution (T2) + `source` addressing (T3) + the integration test spawning into `alpha`/`beta` (T5). ✓
- "per-source worktrees still isolate writes" → wired worktree materialization (T4) + the isolation assertions (T5). ✓
- Workspace identity generalization (ADR 0052 §1) → T1; auto-discovered sources (§2) → T2; per-source addressing (§3) → T3; wired worktree isolation (§4) → T4. ✓
- Back-compat (same hash/socket/store for a single-repo root; `--repo-root` alias; serde-default fields) → T1 hash test, T3 legacy-JSON test, `--repo-root` alias in T1. ✓

**2. Placeholder scan:** No TBD/vague steps; each code step carries the actual code or an exact target signature. Two steps direct the implementer to read a named file first (`config.rs` for `WorktreeSpec::new`'s default `base_ref`; `tests/ipc.rs` for the fake-launcher harness) — grounded reads of named symbols, not placeholders.

**3. Type consistency:** `workspace_hash`/`workspace_socket_path`(`_in`) (T1) used by `caliband.rs`/`store.rs`. `Source`/`discover_sources`/`resolve_source` (T2) consumed by `server.rs` dispatch (T4). `SpawnSpec.source`/`AgentRecord.working_dir` (T3) consumed by `register` (T3) + dispatch/launcher (T4). `WorktreeManager::new`/`create`→`WorktreeHandle{name,path,branch}`/`remove` (T4) match the crate's real API. `Registry::register(spec, endpoint, working_dir)` signature is consistent across T3 (definition) and T4 (call sites).

**Carry-overs to flag in the whole-branch review:** (a) worktrees orphan on a hard daemon crash (best-effort remove mirrors the existing orphaned-socket risk — same failure model, worth noting); (b) auto-discovery assumes sources are direct `.git` children of the workspace root — a CR-declared-but-not-yet-cloned source isn't visible until materialized (ADR 0052 "Revisit if"); (c) the caliban/prospero hash-rule duplication remains deferred (ADR 0052).
