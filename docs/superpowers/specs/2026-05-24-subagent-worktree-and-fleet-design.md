# Sub-agent worktree isolation + background fleet — Design

**Date:** 2026-05-24
**Status:** Proposed
**Author:** john.ford2002@gmail.com
**Sub-project of:** caliban Rust agent harness
**Companion ADR:** `adrs/0037-subagent-isolation-and-background-fleet.md`
**Builds on:** ADR 0021 (sub-agent primitive), ADR 0024 (hook taxonomy — hook
inheritance finish), the unified-settings spec (daemon socket path).

## Goal

Two related capability lifts to the in-process `AgentTool` primitive:

1. **Per-sub-agent git worktree isolation.** When a sub-agent's frontmatter
   declares `isolation: worktree`, caliban materializes a dedicated working
   tree under `.caliban/worktrees/<name>` before spawning the sub-agent, with
   configurable `base_ref` (`fresh` or `head`), optional `symlink_directories`
   for things like `node_modules`/`target`, and `sparse_paths` to scope the
   checkout. The sub-agent's `cwd` points at the worktree; the parent agent
   is untouched.
2. **Background sub-agents.** A sub-agent flagged `background: true` (or
   spawned via `AgentTool` with `bg = true`, or backgrounded mid-run via
   `Ctrl+B` in the TUI) detaches from the calling agent. The parent's tool
   call returns immediately with an opaque agent id; lifecycle is handed off
   to a new `caliban-supervisor` daemon. Operators manage the fleet via
   `caliban agents list / attach / respawn / rm / stop`, and the TUI gains a
   `daemon status` overlay.

Foreground in-process sub-agents (today's behavior) remain the default. Both
new modes are opt-in via frontmatter or runtime flags.

Hook inheritance — deferred from ADR 0024's PR #9 — lands here as the
mechanism that ties parent flow context to a child agent regardless of
isolation mode.

## Non-goals

- **Remote sub-agents.** Worktrees live on the same machine; the supervisor
  IPC is local-only (Unix domain socket). Remote orchestration is out of
  scope (see ADR 0040 placeholder).
- **Multi-machine fleet coordination.** No leader election, no cross-host
  scheduling.
- **Container/VM isolation.** Worktree is a filesystem boundary, not a
  resource boundary. OS sandbox (Seatbelt / bubblewrap) is tracked
  separately under matrix row A.
- **Cross-sub-agent communication primitives.** Sub-agents talk to the
  supervisor and (on attach) to the user, not to each other. A future
  "rendezvous" primitive can layer on top.
- **Re-attaching a stopped sub-agent into the parent's context.** Once
  detached, a background sub-agent runs to completion (or is killed). The
  parent reads its final summary via `caliban agents attach` or the
  `/agents` overlay.

## Architecture

```
┌────────────────────────────────────────────────────────────────────┐
│  Parent Agent (caliban-agent-core)                                 │
│    AgentTool.invoke(input { isolation, background, … })            │
└──────────────┬──────────────────────────────────────────────────────┘
               │  factory(&input) → Agent + cwd + tool registry
               ▼
   ┌──────────────────────────────┐        ┌──────────────────────────────┐
   │  Foreground (default)        │   OR   │  Background (`bg = true`)    │
   │  - stream_until_done inline  │        │  - spawn supervisor handle   │
   │  - returns final text        │        │  - return { agent_id, sock } │
   └──────────────┬───────────────┘        └──────────────┬───────────────┘
                  │                                       │
                  ▼                                       ▼
   ┌──────────────────────────────┐        ┌──────────────────────────────┐
   │  WorktreeManager (optional)  │        │  caliban-supervisor (daemon) │
   │  .caliban/worktrees/<name>   │        │   per-agent Unix socket      │
   │  - git worktree add/remove   │        │   list / attach / kill /     │
   │  - sparse + symlinks         │        │   respawn / rm               │
   └──────────────────────────────┘        └──────────────────────────────┘
                  │                                       │
                  └───────────────── shared ──────────────┘
                              session.json
                          <base>/agents/<id>/
                          ├── manifest.toml
                          ├── session.json
                          ├── stdout.ndjson   (turn-event stream)
                          └── worktree -> ../worktrees/<name>  (if isolated)
```

The parent agent's tool dispatcher does not change: `AgentTool::invoke` is
still the single entry point. What changes is the factory closure (now
returns `Spawn { agent, cwd, lifecycle }`) and a new branch on
`lifecycle = Foreground | Background(SupervisorHandle)`.

## Crate structure

```
crates/
├── caliban-tools-builtin/         (modified)
│   └── src/agent_tool.rs           # input shape + lifecycle branch
├── caliban-worktrees/              (NEW)
│   └── src/
│       ├── lib.rs                  # WorktreeManager
│       ├── config.rs               # WorktreeSpec, BaseRef
│       ├── sparse.rs               # sparse-checkout shim
│       └── symlinks.rs             # link a fixed list of paths
└── caliban-supervisor/             (NEW)
    └── src/
        ├── lib.rs                  # Supervisor + client API (re-exports)
        ├── daemon.rs               # daemonize + socket accept loop
        ├── proto.rs                # IPC frame schema (serde-bincode)
        ├── registry.rs             # in-memory agent registry; persists to disk
        ├── store.rs                # <base>/agents/<id>/ layout I/O
        ├── client.rs               # SupervisorClient (used by CLI + AgentTool)
        └── bin/caliband.rs         # daemon binary entry point
```

The supervisor daemon is a *separate binary* (`caliband`) that the main
`caliban` binary auto-starts on first `bg = true` spawn (or first
`caliban agents` invocation) and then talks to over the socket.

### Workspace deps (deltas)

```toml
caliban-worktrees    = { path = "crates/caliban-worktrees" }
caliban-supervisor   = { path = "crates/caliban-supervisor" }

# In caliban-worktrees:
git2          = "0.19"          # libgit2 binding for worktree add/remove
tempfile      = "3"

# In caliban-supervisor:
tokio         = { workspace = true, features = ["net", "fs", "process", "signal"] }
serde         = { workspace = true }
bincode       = "2"             # IPC frames
nix           = { version = "0.29", features = ["signal", "process"] }
daemonize     = "0.5"            # POSIX double-fork
```

`git2` lets us use libgit2 in-process for `git worktree add/remove/list`
without shelling out — this matters because we want structured errors
(`AlreadyExists`, `LockedBranch`) rather than parsing stderr.

## Frontmatter additions

Sub-agent definition files (existing skill-style markdown, frontmatter +
body) gain three optional keys:

```yaml
---
name: refactor-explorer
description: Explore a refactor in isolation.
tools: [Read, Grep, Glob, Edit]
isolation: worktree          # one of: none | worktree
worktree:
  base_ref: fresh            # fresh | head | <git-ref-string>
  sparse_paths:              # optional; null means full checkout
    - crates/caliban-agent-core
    - crates/caliban-tools-builtin
  symlink_directories:       # paths under the parent worktree to symlink in
    - target
    - node_modules
background: false            # true → detach; default false
inherit_hooks: true          # default; false → child gets a fresh Hooks chain
---
```

Frontmatter is the *declarative* path. The same fields can be overridden
per-call by `AgentTool` input.

## `AgentTool` input — extended

```rust
#[derive(Debug, Deserialize)]
pub struct AgentToolInput {
    pub prompt: String,
    #[serde(default)] pub tool_allowlist: Option<Vec<String>>,
    #[serde(default)] pub model: Option<String>,

    /// Override the frontmatter isolation mode.
    #[serde(default)] pub isolation: Option<IsolationMode>,
    /// Override the frontmatter background flag.
    #[serde(default)] pub background: Option<bool>,
    /// Optional human-readable label that appears in `/agents` and logs.
    #[serde(default)] pub label: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum IsolationMode { None, Worktree }
```

### Factory signature changes

```rust
pub struct Spawn {
    pub agent: Agent,
    pub cwd: PathBuf,                 // worktree root if isolated, else parent cwd
    pub worktree: Option<WorktreeHandle>,
    pub inherit_hooks: bool,
}

pub type AgentFactory = Arc<dyn Fn(&AgentToolInput, &ParentContext) -> Result<Spawn, AgentToolError>
    + Send + Sync>;

pub struct ParentContext<'a> {
    pub parent_cwd: &'a Path,
    pub parent_hooks: Arc<dyn Hooks>,
    pub supervisor: Option<&'a SupervisorClient>,
    pub session_id: SessionId,
}
```

`WorktreeHandle` owns the on-disk worktree; foreground sub-agents `Drop` it
when `invoke` returns (with configurable retention — see "Cleanup" below);
background sub-agents transfer ownership to the supervisor.

## `WorktreeManager`

```rust
pub struct WorktreeManager { /* root: <parent_repo>/.caliban/worktrees */ }

pub struct WorktreeSpec {
    pub name: String,                       // becomes the directory name
    pub base_ref: BaseRef,
    pub sparse_paths: Option<Vec<PathBuf>>,
    pub symlink_directories: Vec<PathBuf>,
}

pub enum BaseRef {
    Fresh,                                  // detached HEAD on the empty tree
    Head,                                   // detached HEAD on current branch tip
    Ref(String),                            // any rev-parse-able ref
}

pub struct WorktreeHandle {
    pub path: PathBuf,
    pub name: String,
    /* private: git repo handle for clean removal */
}

impl WorktreeManager {
    pub fn new(repo_root: &Path) -> Result<Self, WorktreeError>;
    pub fn create(&self, spec: &WorktreeSpec) -> Result<WorktreeHandle, WorktreeError>;
    pub fn list(&self) -> Result<Vec<WorktreeRecord>, WorktreeError>;
    pub fn remove(&self, name: &str, force: bool) -> Result<(), WorktreeError>;
}
```

### Creation algorithm

1. Resolve `repo_root` by walking up from CWD until a `.git` directory or
   file is found. (If none, refuse `isolation: worktree`; surface a clear
   error.)
2. Compute `path = repo_root.join(".caliban/worktrees").join(&spec.name)`.
   If `path` already exists, error `WorktreeError::AlreadyExists`.
3. Resolve the base ref:
   - `Fresh` → `git2::Repository::find_remote_branch_or_head` then create a
     detached worktree at the *empty tree* (`4b825dc...`), so the worktree
     starts as an empty checkout but tracks the repo. Operator scripts that
     need files materialize them deliberately.
   - `Head` → resolve `HEAD` of the parent repo's currently-checked-out
     branch.
   - `Ref(name)` → `repo.revparse_single(name)`.
4. `repo.worktree(&spec.name, &path, &opts)` with `--detach`.
5. If `sparse_paths` is set: write `.git/info/sparse-checkout` inside the
   worktree, run `git read-tree -m -u HEAD` to materialize. (Via libgit2:
   `repo.set_sparse_checkout_patterns(&paths)`.)
6. For each entry in `symlink_directories`: create a symlink inside the
   worktree pointing at `parent_repo/<entry>`. Refuse to overwrite files;
   error if the parent path doesn't exist.

### Cleanup

- **Foreground:** by default, remove the worktree on `Drop`
  (`git worktree remove --force`). Operators can opt out via env var
  `CALIBAN_KEEP_WORKTREES=1` (debugging) or per-call frontmatter
  `keep_on_exit: true`.
- **Background:** the supervisor owns the handle. The worktree is removed
  on `caliban agents rm <id>` (and on supervisor startup pruning — see
  "Crash recovery" below).

## Supervisor daemon

### Process model

- Single daemon process per `repo_root` (keyed on the absolute path).
  Socket path: `${CALIBAN_DAEMON_RUNTIME_DIR:-$XDG_RUNTIME_DIR/caliban}/<hash(repo_root)>.sock`
  (see unified-settings spec for the env-var contract).
- Auto-spawned by the CLI on first need; daemonizes via the `daemonize`
  crate; writes its pid to `<runtime>/<hash>.pid`.
- `caliban agents` always reads the registry from the socket (single source
  of truth), never the on-disk store directly.

### IPC contract (`proto.rs`)

```rust
#[derive(Serialize, Deserialize)]
pub enum CtlRequest {
    Spawn   { spec: SpawnSpec },
    List,
    Attach  { id: AgentId },           // streams events back until detached
    Detach  { id: AgentId },
    Kill    { id: AgentId, signal: Signal },
    Respawn { id: AgentId },           // re-run with the same SpawnSpec
    Rm      { id: AgentId, force: bool },
    Status,
}

#[derive(Serialize, Deserialize)]
pub enum CtlReply {
    Spawned   { id: AgentId, socket: PathBuf },
    Listed    { agents: Vec<AgentRecord> },
    AttachAck { initial_session: SessionSnapshot },
    Event     (TurnEvent),
    Detached,
    Killed,
    Removed,
    Error     { kind: SupervisorError },
    DaemonStatus { pid: u32, agents: u32, uptime_secs: u64 },
}

pub struct SpawnSpec {
    pub label: Option<String>,
    pub frontmatter: AgentFrontmatter,    // including worktree + hooks
    pub initial_prompt: String,
    pub session_dir: PathBuf,             // <base>/agents/<id>/
    pub model: Option<String>,
    pub tool_allowlist: Option<Vec<String>>,
}
```

Frames are length-prefixed bincode over the Unix socket.

### Per-agent socket

Each running sub-agent also gets a *dedicated* Unix socket at
`<base>/agents/<id>/agent.sock`. This is the one `caliban attach` connects
to (via the supervisor's `Attach` reply, which returns its path). The
dedicated socket only carries `TurnEvent`s and inbound user messages —
this keeps the control-plane socket small and lets a hung sub-agent's
event stream be drained independently.

### Lifecycle states

```
Spawning → Running → (Idle | Working) → Completing → Done
                  ↘ Failed
                  ↘ Cancelled (by Kill)
```

Transitions are persisted (atomic write of `manifest.toml`) so a daemon
crash + restart can reconstruct without consulting the agents themselves.

## Session persistence per sub-agent

```
<base>/                       # $XDG_DATA_HOME/caliban or override
└── agents/
    └── <id>/
        ├── manifest.toml     # frontmatter copy + state + spawn time
        ├── session.json      # caliban-sessions format (existing)
        ├── stdout.ndjson     # one TurnEvent per line, append-only
        ├── stderr.log        # supervisor-captured stderr
        └── worktree → ../../<repo>/.caliban/worktrees/<name>
```

The `<id>` is a 12-char ULID prefix. The base dir defaults to the unified
settings' `data_dir` (project scope → `${repo}/.caliban/agents`, user scope →
`$XDG_DATA_HOME/caliban/agents`).

Reusing the existing `caliban-sessions` format means a finished background
sub-agent's `session.json` is replayable through `caliban resume <id>`,
which `caliban agents attach` is sugar for.

## TUI integration

### `Ctrl+B` — background a foreground sub-agent

When a foreground `AgentTool` invocation is in flight (the parent is
`await`ing the sub-agent stream), `Ctrl+B`:

1. Sends `cx.cancel.cancel()` to the *parent's* tool future. The parent
   sees a `ToolError::Backgrounded(agent_id)` and the assistant message in
   the parent transcript reads "[backgrounded sub-agent <id> — see /agents]".
2. Before cancelling, the runtime snapshots the in-flight sub-agent
   (`session.json` + pending stream events) and hands the snapshot +
   ownership to the supervisor over `CtlRequest::Spawn { resume_from: … }`.
3. The supervisor resumes from the snapshot in its own task; the
   sub-agent continues running detached.

This is implemented by adding `enum LifecycleAction { Continue, Background }`
to the foreground polling loop in `AgentTool::invoke`.

### `/agents` overlay

A new TUI overlay (replacing the placeholder slash) lists every agent the
supervisor knows about:

```
┌─ Agents ───────────────────────────────────────────────────────────────┐
│ ● 01HXY… refactor-explorer   running   12 turns  3m12s   [a] attach    │
│ ◐ 01HXZ… code-reviewer       idle        8 turns  1m04s   [a] attach   │
│ ✓ 01HYA… test-runner         done       18 turns  6m22s   [r] respawn  │
│ ✗ 01HYB… flaky-job           failed     turn 3   exit 1   [r] respawn  │
└────────────────────────────────────────────────────────────────────────┘
[a] attach  [k] kill  [r] respawn  [x] rm  [s] daemon status  [esc] close
```

Glyphs: `●` running, `◐` idle, `✓` done, `✗` failed.

### `daemon status` slash + CLI

`/daemon status` (TUI) and `caliban daemon status` (CLI) call
`CtlRequest::Status` and render `{ pid, agents, uptime }` plus the socket
path. Useful for debugging supervisor lifecycle.

## CLI surface

```
caliban agents list
caliban agents attach <id>          # streams transcript live; Ctrl+D detaches
caliban agents kill <id> [--sigkill]
caliban agents respawn <id>
caliban agents rm <id> [--force]
caliban agents spawn --frontmatter path/to/agent.md --prompt "..."  [--bg]
caliban daemon status
caliban daemon stop
```

`attach` uses the dedicated per-agent socket; it tees `stdout.ndjson` from
the tail and continues with live events, so attaching mid-run shows
history. Ctrl+D detaches without killing.

## Hook inheritance

Closes the deferred follow-up from ADR 0024 (PR #9).

- The parent's `Hooks` chain is captured at spawn time as
  `Arc<dyn Hooks>`. If `inherit_hooks: true` (default), the child
  `AgentBuilder::hooks` is set to that same chain.
- If `inherit_hooks: false`, the child gets the *binary's default* chain
  only (`PermissionsHook` + any in-process audit hooks), not the parent's
  flow-specific stack.
- For background sub-agents the same rule applies, but the inherited
  chain is serialized through the supervisor: hook config (`hooks.toml`
  path + the inherited in-process hook *list*, not the closures) is
  passed in the `SpawnSpec`; the supervisor reconstructs the chain in
  the child process. In-process closure-based hooks that aren't
  expressible in config get a synthetic `parent-flow-only` marker and
  *do not* run in the background child — this is documented loudly in
  the spec and surfaced as a warning when the supervisor sees one.
- `SubagentStart` / `SubagentStop` fire on the *parent's* chain
  regardless of inheritance, so observability stays consistent.

## Crash recovery

- On `caliband` startup, scan `<base>/agents/*/manifest.toml`. For each
  entry in state `Running` or `Working`: mark as `Crashed`. The
  `/agents` overlay shows them with `✗ crashed (recoverable)`. Operators
  can `respawn` (re-runs from the recorded prompt) or `rm`.
- Orphan worktrees (`.caliban/worktrees/<name>` with no live agent
  pointing at them) are pruned on startup if `prune_orphans = true` in
  daemon settings; otherwise listed as orphans by `caliban daemon status`.

## Concurrency limits

`SupervisorConfig` (defaults in unified settings):

```toml
[supervisor]
max_concurrent_agents = 8
max_agents_per_repo   = 32
respawn_backoff_ms    = 5000
default_kill_signal   = "SIGTERM"
kill_grace_secs       = 10
```

Concurrency cap is enforced at `Spawn` time; the request waits in a
priority queue (FIFO; future: priority by `purpose`). Per-repo cap is a
soft fence to prevent runaway parents from filling disk.

## Testing strategy

20 enumerated tests across the new crates:

**`caliban-worktrees`:**

1. `create_with_base_ref_head_materializes_files`
2. `create_with_base_ref_fresh_starts_empty`
3. `create_with_named_ref_resolves_to_commit`
4. `sparse_paths_restricts_checkout`
5. `symlink_directories_creates_symlinks_into_parent`
6. `create_refuses_when_name_already_exists`
7. `remove_force_drops_locked_worktree`
8. `list_returns_managed_worktrees_only`
9. `creating_outside_repo_returns_clear_error`

**`caliban-supervisor`:**

10. `daemon_starts_and_accepts_connections`
11. `spawn_records_manifest_and_returns_id`
12. `list_after_spawn_shows_running_state`
13. `attach_streams_history_then_live_events`
14. `kill_with_sigterm_drains_within_grace_window`
15. `respawn_reuses_spawnspec_and_creates_new_id`
16. `daemon_restart_recovers_running_as_crashed`
17. `concurrency_cap_queues_spawn_requests`
18. `orphan_worktree_pruning_runs_on_startup`

**`caliban-tools-builtin` (agent_tool integration):**

19. `isolation_worktree_spawns_sub_agent_with_correct_cwd`
20. `ctrl_b_background_transfers_ownership_to_supervisor`
21. `hook_inheritance_inherit_true_passes_parent_chain`
22. `hook_inheritance_inherit_false_uses_default_chain`

## Risks

- **libgit2 vs. CLI git divergence.** Some worktree edge cases (locked
  branches, submodule fan-out) behave subtly differently in libgit2. Mit:
  add an integration test against the real `git` binary on macOS + Linux
  CI runners.
- **Sparse + symlinks composition.** A symlinked `target/` inside a sparse
  checkout is fine, but a sparse path that excludes the symlink target's
  parent breaks. Mit: validate `symlink_directories` paths exist in the
  parent repo *before* creating the worktree; surface `BrokenLink` early.
- **Background-agent socket leakage.** Long-lived per-agent sockets can
  accumulate. Mit: prune on `rm`; on daemon startup remove any socket
  files for agents not in the recovered registry.
- **Hook inheritance type erasure.** Closure-based hooks can't be
  serialized to a child process. Mit: explicit warning + `parent-flow-only`
  marker; document loudly. Long-term, push more hooks into the
  config-driven `HookRouter` (ADR 0024) so they *are* serializable.
- **Disk usage for worktrees.** A fresh worktree per sub-agent at scale is
  expensive. Mit: `Fresh` base_ref is empty by default; sparse paths
  limit footprint; `caliban daemon status` reports total worktree bytes;
  retention policy via `keep_on_exit` defaults to `false`.
- **Symlink loop on Windows.** Symlink creation requires admin or
  developer-mode on Windows. Mit: detect, surface `SymlinksRequireElevation`
  with a docs pointer; Windows support is best-effort in v1 and gates
  the feature behind a feature flag.

## Acceptance criteria

- `cargo build --workspace` clean; `cargo clippy --workspace --all-targets
  -- -D warnings` clean; `cargo fmt --all -- --check` clean.
- ≥22 new tests passing across the three crates.
- `caliban agents list/attach/kill/respawn/rm/spawn` all functional;
  `caliban daemon status/stop` functional.
- Frontmatter `isolation: worktree` end-to-end: a sub-agent edits files in
  its worktree; the parent's working tree is unchanged; on `Drop` the
  worktree is removed.
- Frontmatter `background: true` end-to-end: sub-agent detaches; parent
  receives an id; `caliban agents attach <id>` streams the transcript.
- `Ctrl+B` in the TUI backgrounds an in-flight sub-agent; the parent's
  transcript records the handoff; the supervisor reports the new agent in
  `/agents`.
- Hook inheritance: parent flow `Hooks` are inherited by default; the
  `inherit_hooks: false` knob disables it; `SubagentStart`/`SubagentStop`
  fire on the parent chain in both modes.
- Matrix G rows for worktree isolation, background sub-agents, subagent-
  local memory dir, hook inheritance, and the supervisor daemon move
  🔴 → ✅ in the PR that lands this work.
- ADR 0037 in `accepted` status.
