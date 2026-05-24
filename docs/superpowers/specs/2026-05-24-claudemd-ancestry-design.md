# CLAUDE.md ancestor walk + `@`-imports — Design

**Date:** 2026-05-24
**Author:** john.ford2002@gmail.com
**Status:** Proposed
**Sub-project of:** caliban Rust agent harness
**ADR:** `adrs/0036-claudemd-ancestry-and-imports.md`

## Goal

Replace `caliban-memory`'s single-file project tier (workspace-root
`CLAUDE.md` only) with the full Claude Code project-tier loader:

1. **Ancestor walk** — start at cwd, walk upward until the git root or
   filesystem root, concatenate every `CLAUDE.md` / `AGENTS.md` /
   `.caliban.md` found along the way (broad → narrow ordering).
2. **`@`-imports** — `@path/to/file` lines inside any CLAUDE.md import
   the referenced file's content inline; recursion depth ≤5; first
   external import gets an approval dialog.
3. **Nested-on-demand** — after the model `Read`s a file under
   subdirectory X, any `CLAUDE.md` in X (or its ancestors *between*
   the workspace and X that weren't yet loaded) is added as a
   system-prompt addendum for the rest of the session.
4. **`.caliban/rules/<topic>.md`** — topic rules with optional `paths:`
   glob frontmatter; activate only when matching files are read or
   edited.
5. **`claude_md_excludes`** — gitignore-style excludes for monorepos.

## Non-goals

- **No CLAUDE.md `@`-imports across HTTP.** Imports resolve to local
  filesystem paths only; `@https://...` is rejected.
- **No diff-aware re-load.** Nested-on-demand triggers once per
  `(file, session)` pair; we don't re-splice when CLAUDE.md changes
  mid-session.
- **No subagent-local memory dir overrides.** Subagents inherit the
  ancestor walk of the parent's cwd. Subagent-local memory is a
  separate matrix row (G. Sub-agents).
- **No `~/CLAUDE.md` discovery beyond the existing global tier.** The
  global tier (`~/.config/caliban/CLAUDE.md`) is unchanged; ancestor
  walk applies only to the project tier.

## Architecture

```
caliban-memory
  MemoryConfig (existing)
    project_path: Option<PathBuf>     ← deprecated single-file path
    project_walk_root: Option<PathBuf> ← NEW: ancestor-walk start (= cwd)
    project_walk_stop: WalkStop        ← NEW: GitRoot | FsRoot | Both
    additional_dirs: Vec<PathBuf>      ← NEW: --add-dir paths
    claude_md_excludes: Vec<String>    ← NEW: gitignore patterns

  loader (existing) + new modules:
    project_walk.rs         walk_ancestors(cwd) -> Vec<PathBuf>
    project_imports.rs      resolve_imports(body, &mut state)
    rules.rs                scan_caliban_rules(workspace) -> Vec<Rule>
    rules_activator.rs      maintain active rule set on Read/Edit/Glob

  ProjectTier (replaces single TierFile in MemoryPrefix::project)
    base_files:    Vec<TierFile>      ← walk results, broad → narrow
    imports:       Vec<TierFile>      ← resolved @-imports (recursive)
    nested:        Vec<TierFile>      ← added during session as model reads files
    active_rules:  Vec<TierFile>      ← path-glob-matched rules

caliban-tools-builtin
  Read / Edit / Glob hooks:
    on success → notify rules_activator + ancestry_addendum

caliban-core / agent-core
  system-prompt assembly:
    1. global CLAUDE.md (unchanged)
    2. ProjectTier::base_files (concat, broad → narrow)
    3. ProjectTier::imports (interleaved at import points)
    4. ProjectTier::active_rules (path-scoped, dynamic)
    5. ProjectTier::nested (dynamic addendums)
    6. auto-memory (existing)
```

## Algorithm: ancestor walk

```rust
fn walk_ancestors(cwd: &Path, stop: WalkStop, excludes: &GlobSet) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen_inodes = BTreeSet::new();
    let mut p = cwd.to_path_buf();
    loop {
        for name in ["CLAUDE.md", "AGENTS.md", ".caliban.md"] {
            let candidate = p.join(name);
            if !candidate.exists() { continue; }

            // dedupe by inode (symlink loops)
            let inode = inode_key(&candidate);
            if !seen_inodes.insert(inode) { continue; }

            // monorepo excludes
            if excludes.is_match(&candidate.strip_prefix(start).unwrap_or(&candidate)) {
                continue;
            }
            out.push(candidate);
        }
        // stop rule
        if reached_stop(&p, stop) { break; }
        match p.parent() {
            Some(parent) => p = parent.to_path_buf(),
            None => break,
        }
    }
    out.reverse();      // broad → narrow concatenation order
    out
}
```

`reached_stop` returns true when:

- `WalkStop::GitRoot` and `p` contains a `.git/` entry, OR
- `WalkStop::FsRoot` and `p == /`, OR
- `WalkStop::Both` and either of the above (default).

Walking *includes* the directory that hosts `.git/` (it's typical for
the canonical project CLAUDE.md to live at the git root).

### Filename precedence at a single directory

If both `CLAUDE.md` and `.caliban.md` exist at the same directory, both
are loaded. `AGENTS.md` is loaded if present even when neighboring
`CLAUDE.md` exists. Order within a directory: `.caliban.md` →
`CLAUDE.md` → `AGENTS.md` (most-specific → most-general; this lets a
`.caliban.md` override an `AGENTS.md` while both contribute). All
three files use the same body syntax; `@`-imports work in all of them.

### `--add-dir` paths

When the caliban CLI is invoked with `--add-dir PATH` (one or many)
and `CALIBAN_ADDITIONAL_DIRECTORIES_CLAUDE_MD=1` is set, each
additional directory contributes its own ancestor walk **as a separate
walk rooted at that dir**. Walks are concatenated in `--add-dir`
declaration order *after* the cwd walk. Without
`CALIBAN_ADDITIONAL_DIRECTORIES_CLAUDE_MD=1` (the default), `--add-dir`
contributes filesystem access only — no CLAUDE.md is loaded from those
paths.

## Algorithm: `@`-imports

```rust
const MAX_IMPORT_DEPTH: u8 = 5;

fn resolve_imports(
    body: String,
    importer: &Path,
    state: &mut ImportState,
) -> ResolvedBody {
    let mut out = String::new();
    for line in body.lines() {
        if let Some(rel) = parse_import_directive(line) {
            if state.depth >= MAX_IMPORT_DEPTH {
                tracing::warn!(target: "caliban::memory", "@-import depth cap at {importer:?}");
                out.push_str(line); out.push('\n');
                continue;
            }
            let resolved = resolve_path(rel, importer);
            if !approval_allows(&resolved, state) { /* prompt or skip */ continue; }
            let imported = read_capped(&resolved, IMPORT_MAX_BYTES)?;
            state.depth += 1;
            state.import_stack.push(resolved.clone());
            let sub = resolve_imports(imported, &resolved, state);
            state.import_stack.pop();
            state.depth -= 1;
            out.push_str(&format!("\n<!-- imported from {resolved:?} -->\n"));
            out.push_str(&sub.body);
            out.push_str(&format!("\n<!-- end {resolved:?} -->\n"));
        } else {
            out.push_str(line); out.push('\n');
        }
    }
    ResolvedBody { body: out, files: state.files_loaded.clone() }
}

fn parse_import_directive(line: &str) -> Option<&str> {
    // matches a line whose first non-whitespace token starts with '@' and is
    // followed by a path-like token (no spaces). Lines like `@some-mention`
    // without a slash are NOT imports — must contain `/`, `~`, or a `.`
    // extension to count.
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix('@')?;
    let token = rest.split_whitespace().next()?;
    if !(token.contains('/') || token.starts_with('~') || token.contains('.')) {
        return None;
    }
    Some(token)
}
```

`resolve_path` rules:

| Input              | Resolution |
| ------------------ | ---------- |
| `./foo.md`         | `importer.parent().join("foo.md")` |
| `../shared/x.md`   | normalized join |
| `~/notes/x.md`     | `dirs::home_dir()?.join("notes/x.md")` |
| `/abs/path.md`     | passthrough |
| `@http://…`        | **rejected** (returns ImportError::UnsupportedScheme) |

The same file imported twice in one body is loaded once (deduplicated
by canonicalized path). Cycle detection: if `resolved` is already in
`state.import_stack`, skip with a `tracing::warn!` and an inline
`<!-- cycle: skipped -->` marker.

### Approval dialog for first-time external paths

Definition of "external": resolved path is **not** under the
workspace root (the start of the ancestor walk) **and** not under
`~/.config/caliban/`.

On first encounter:

```
┌─ Approve CLAUDE.md @-import ────────────────────────────────────────┐
│                                                                      │
│   File:    /Users/john/dev/personal/notes/api-conventions.md        │
│   From:    /Users/john/dev/personal/caliban/CLAUDE.md                │
│                                                                      │
│   [a] Always allow this path                                        │
│   [o] Allow once (this session only)                                │
│   [d] Deny (skip the import)                                        │
│                                                                      │
└──────────────────────────────────────────────────────────────────────┘
```

Approvals persist in `~/.caliban/imports-allowlist.json`:

```json
{
  "version": 1,
  "approved": [
    {
      "path": "/Users/john/dev/personal/notes/api-conventions.md",
      "approved_at": "2026-05-24T14:22:10Z",
      "approved_session": "0193f8a2-..."
    }
  ]
}
```

In non-interactive mode (`--print`, CI, `--bare`), all external
imports are denied (logged but skipped). `--approve-imports` /
`CALIBAN_APPROVE_IMPORTS=1` accepts everything for one invocation
(useful for non-interactive parity tests).

### Import size cap

Each individual imported file is capped at 64 KB (`IMPORT_MAX_BYTES`).
Total per-tier imports (summed) cap at 256 KB — past that, further
imports are skipped with a `tracing::warn!`.

## Algorithm: nested-on-demand

After the model successfully completes a `Read` / `Edit` / `Glob` /
`MultiEdit` (any tool that surfaces a concrete path), the agent
notifies the ancestry-addendum subsystem:

```rust
pub struct AncestryAddendum {
    workspace_root: PathBuf,
    walk_stop:      WalkStop,
    excludes:       GlobSet,
    loaded:         Mutex<BTreeSet<PathBuf>>,  // dedupe per session
    addendum_files: Mutex<Vec<TierFile>>,
}

impl AncestryAddendum {
    pub fn on_path_touched(&self, path: &Path) -> Option<Vec<TierFile>> {
        let mut new_files = Vec::new();
        let mut p = path.parent().map(|x| x.to_path_buf());
        while let Some(dir) = p {
            // stop when we hit a dir whose CLAUDE.md was already loaded
            // by the initial walk or a prior on-demand load.
            for name in ["CLAUDE.md", "AGENTS.md", ".caliban.md"] {
                let candidate = dir.join(name);
                if candidate.exists() && self.loaded.lock().insert(candidate.clone()) {
                    new_files.push(read_tier(&candidate)?);
                }
            }
            if dir == self.workspace_root { break; }
            p = dir.parent().map(|x| x.to_path_buf());
        }
        (!new_files.is_empty()).then_some(new_files)
    }
}
```

When new files are returned, the agent-core appends them to its
running system prompt under a fresh `<project-claude-md
path="…" added-mid-session="true">…</project-claude-md>` block. The
addition is irreversible for the rest of the session; we don't try to
shed an addendum even if the model `Read`s back out of that subtree.

Imports inside an on-demand-loaded file are resolved with the same
rules as initial-walk imports (depth ≤5, approval dialog, etc.).

## Algorithm: `.caliban/rules/<topic>.md`

Rules are topic-scoped CLAUDE.md fragments that activate only when the
model interacts with matching paths. Layout:

```
<workspace>/.caliban/rules/
├── python-style.md
├── rust-clippy.md
├── frontend-react.md
└── README.md             ← excluded by convention
```

Each rule file has YAML frontmatter:

```markdown
---
name: python-style
paths:
  - "**/*.py"
  - "scripts/**"
description: "Python formatting conventions: black + ruff, 100-col, double quotes."
---

When editing Python files in this repo:
- Format with `black` + `ruff check --fix`.
- Max line length 100.
- Prefer double quotes.
- Always type-annotate function signatures.
```

`paths:` is optional. When absent, the rule is **always active** (just
loaded into the system prompt at startup like a tiny CLAUDE.md
addendum). When present, the rule is loaded into the system prompt as
soon as the model touches a matching file via `Read`/`Edit`/`Glob`.

Activation uses the workspace-wide `globset::GlobSet` built from all
rules' patterns; matching is O(1) per touched path.

Rules participate in `@`-imports the same way as CLAUDE.md.

Once activated, a rule stays loaded for the rest of the session (no
deactivation).

## `claude_md_excludes`

A list of gitignore-style patterns scoped to the workspace root.
Lives in:

- `~/.config/caliban/settings.toml` → `[memory] claude_md_excludes = [...]`
- `<workspace>/.caliban/settings.toml` → same
- Env: `CALIBAN_CLAUDE_MD_EXCLUDES` (newline- or `:`-separated)

Patterns merge across sources (broadest wins for a deny-style match).
Examples:

```toml
[memory]
claude_md_excludes = [
    "node_modules/**",
    "vendor/**",
    "third_party/**/CLAUDE.md",     # don't load nested deps' CLAUDE.md
    "**/.git/**",
]
```

Matching uses `globset` (already a workspace dep) with the gitignore
semantics adapter (`!pattern` negates).

## Public API sketches

```rust
// crates/caliban-memory/src/lib.rs

pub use project_walk::{WalkStop, walk_ancestors};
pub use project_imports::{ImportApproval, ImportState, resolve_imports};
pub use rules::{Rule, RuleSet};

// crates/caliban-memory/src/prefix.rs (additions)

pub struct ProjectTier {
    pub base_files:   Vec<TierFile>,         // ancestor walk
    pub imports:      Vec<TierFile>,         // resolved @-imports
    pub active_rules: Vec<TierFile>,         // path-glob-matched
    pub nested:       Vec<TierFile>,         // session-grown
}

// MemoryPrefix.project becomes Option<ProjectTier> (was Option<TierFile>).
```

```rust
// crates/caliban-memory/src/config.rs (additions)

pub struct MemoryConfig {
    /* existing fields */
    pub project_walk_root: Option<PathBuf>,
    pub project_walk_stop: WalkStop,
    pub additional_dirs:   Vec<PathBuf>,
    pub claude_md_excludes: GlobSet,
    pub approve_imports:   bool,             // CALIBAN_APPROVE_IMPORTS
}
```

```rust
// crates/caliban-tools-builtin/src/hooks.rs (additions)

pub trait PathTouchHook: Send + Sync {
    fn on_path_touched(&self, path: &Path);
}

// AncestryAddendum and RulesActivator both impl PathTouchHook;
// the registry composes them.
```

## Splice format

The `<project-claude-md>` block in the system prompt becomes a series:

```
<project-claude-md path="/abs/path/CLAUDE.md" order="0" source="walk">
…body with @-imports inlined as <!-- imported from … --> markers…
</project-claude-md>

<project-claude-md path="/abs/path/sub/CLAUDE.md" order="1" source="walk">
…
</project-claude-md>

<project-rule name="python-style" paths="**/*.py" source="rule" activated="lazy">
…body…
</project-rule>

<project-claude-md path="/abs/path/sub/foo/CLAUDE.md" source="nested" added-mid-session="true">
…
</project-claude-md>
```

Attributes (`order`, `source`, `activated`, `added-mid-session`) give
the model + the debug log a clear provenance trail.

## Settings / env

| Setting / env                                  | Default                       | Effect |
| ---------------------------------------------- | ----------------------------- | ------ |
| `[memory] project_walk_stop`                   | `"both"`                      | `"git_root"` / `"fs_root"` / `"both"` |
| `[memory] claude_md_excludes`                  | `[]`                          | gitignore patterns |
| `[memory] additional_directories`              | `[]`                          | `--add-dir` defaults |
| `CALIBAN_ADDITIONAL_DIRECTORIES_CLAUDE_MD`     | `0`                           | `1` enables CLAUDE.md load from `--add-dir` paths |
| `CALIBAN_APPROVE_IMPORTS`                      | _unset_                       | `1` approves every external import non-interactively |
| `CALIBAN_DISABLE_CLAUDE_MD_WALK`               | _unset_                       | `1` reverts to single-file project tier (regression switch) |
| `CALIBAN_CLAUDE_MD_EXCLUDES`                   | _unset_                       | colon-separated overrides for `[memory] claude_md_excludes` |

## Testing strategy

18 enumerated tests:

1. `walk_ancestors` from a 4-deep cwd inside a git repo finds only the in-repo ancestors, stopping at git root.
2. `walk_ancestors` returns files in broad → narrow order after the reverse step.
3. `walk_ancestors` dedupes by inode (symlink to ancestor's `CLAUDE.md` not double-loaded).
4. `walk_ancestors` honors `claude_md_excludes` (e.g. `node_modules/**`).
5. `walk_ancestors` includes both `CLAUDE.md` and `AGENTS.md` in the same directory.
6. `parse_import_directive` recognizes `@./foo.md`, `@~/bar/baz.md`, `@/abs/path.md`; rejects `@some-mention` and `@user`.
7. `resolve_imports` recurses correctly and inlines content under `<!-- imported from … -->` markers.
8. `resolve_imports` enforces `MAX_IMPORT_DEPTH=5`; depth-6 import logs a warning and leaves the directive literal.
9. `resolve_imports` rejects `@https://…` (UnsupportedScheme).
10. `resolve_imports` detects cycles (A imports B which imports A) and emits `<!-- cycle: skipped -->`.
11. Approval dialog flow: external import on first encounter prompts; `Always` persists to `~/.caliban/imports-allowlist.json`; second encounter does not prompt.
12. `--approve-imports` / `CALIBAN_APPROVE_IMPORTS=1` skips the dialog and approves silently.
13. Non-interactive mode (`--print`) auto-denies external imports.
14. `AncestryAddendum::on_path_touched` returns the new CLAUDE.md when a `Read` happens in a subtree whose CLAUDE.md wasn't yet loaded.
15. `AncestryAddendum::on_path_touched` returns `None` after a second `Read` in the same subtree (dedupe).
16. `Rule` activation: `Read(scripts/foo.py)` activates `python-style` rule once.
17. Rule without `paths:` is loaded at startup (always-active).
18. `claude_md_excludes` matches against the path relative to the workspace root, not the absolute path.

Integration test (`tests/walk_imports_roundtrip.rs`):

- Build a tempdir hierarchy:
  ```
  /tmp/root/CLAUDE.md             ("ROOT — @./shared/conv.md")
  /tmp/root/shared/conv.md        ("CONV — @../detail.md")
  /tmp/root/detail.md             ("DETAIL")
  /tmp/root/sub/CLAUDE.md         ("SUB")
  /tmp/root/sub/deep/foo.py
  ```
- Init `MemoryConfig { project_walk_root: /tmp/root/sub/deep, walk_stop: GitRoot, … }`.
- `load()` → `ProjectTier.base_files == [root/CLAUDE.md, sub/CLAUDE.md]`,
  `imports == [shared/conv.md (depth1), detail.md (depth2)]`.
- `AncestryAddendum::on_path_touched(/tmp/root/sub/deep/foo.py)` → no new
  (sub's CLAUDE.md already loaded by walk).

## Risks

- **Approval-dialog UX in long sessions.** A CLAUDE.md with 12
  `@`-imports prompts 12 times on first run. Mitigation: dialog
  offers `Approve all imports from this file` shortcut that
  bulk-approves the entire transitive closure rooted at that file.
- **Cycle detection by canonical path.** Symlink chicanery (A → B
  → C → symlink-back-to-A) requires canonicalize on each resolved
  path; we use `dunce::canonicalize` for Windows-compatibility.
- **Glob performance on large monorepos.** `claude_md_excludes` is
  evaluated for every candidate during ancestor walk; with 50 patterns
  + 10 directories, that's 500 globset matches per startup.
  Mitigation: `globset` is fast (~µs per match); not a real concern
  until pattern count exceeds 1000.
- **`@`-imports inside imported files create surprise.** A CLAUDE.md
  importing a file that itself imports yet another file is a
  user-experience challenge. Mitigation: provenance markers + the
  `/memory` overlay shows the full import tree.
- **Nested-on-demand cardinality.** A model that wanders through 50
  subdirectories during a session can grow the system prompt
  significantly. Mitigation: addendum tier-files count toward the same
  `CALIBAN_MEMORY_BUDGET_TOKENS` budget; the existing truncation logic
  applies. `/memory` shows current addendum count.
- **`.caliban.md` vs `CLAUDE.md` confusion.** Operators may write the
  same content in both and double-prompt themselves. Mitigation:
  `/memory` shows both with their roles tagged; one-line README guidance
  ("`.caliban.md` is for caliban-specific overrides; `CLAUDE.md` is the
  Claude-Code-compatible one").
- **Excludes ambiguity.** A gitignore-style pattern that *negates*
  (`!keep/path/**`) interacts with the include set in surprising
  ways. Mitigation: document semantics (deny wins absent a `!`
  negation matching after); test #18 covers a positive case.

## Acceptance criteria

- `cargo build --workspace` clean; `clippy --workspace --all-targets -- -D warnings` clean; `fmt --check` clean.
- All 18 unit tests + the walk-and-imports integration test pass.
- `MemoryPrefix.project` is now a `ProjectTier` with `base_files`,
  `imports`, `active_rules`, `nested` populated correctly.
- `/memory` overlay shows the ancestor-walk file list with
  source/order/import-depth columns.
- `Read`/`Edit`/`Glob` hooks notify `AncestryAddendum` and
  `RulesActivator` on success.
- `~/.caliban/imports-allowlist.json` populated by the approval
  dialog; persists across sessions; respected on second run.
- `CALIBAN_DISABLE_CLAUDE_MD_WALK=1` falls back to the legacy
  single-file project tier (regression escape).
- `docs/parity-gap-matrix.md` rows under **C. Memory & checkpointing** —
  `CLAUDE.md ancestor walk + nested-on-demand` and
  ``@path/file imports inside CLAUDE.md (recursion-bounded)`` and
  `claudeMdExcludes for monorepos` — move 🟡 / 🔴 → ✅.
- README's Memory section gains an "Ancestor walk + @-imports"
  subsection with a worked example.
- ADR 0036 in `accepted` status (this spec's prerequisite).
