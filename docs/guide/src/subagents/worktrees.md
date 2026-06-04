# Worktree Isolation

When a sub-agent writes files, those writes land in the parent's working
tree by default. That is fine for read-only investigation, but it mixes the
sub-agent's diff into yours and gives you no clean way to discard it.
Worktree isolation solves this: caliban materializes a dedicated git worktree
for the sub-agent so its file operations are completely separate from the
parent's tree.

## How it works

When `isolation: worktree` is requested, caliban uses the
`caliban-worktrees` crate to:

1. Create a new git branch named `caliban/<name>` off the chosen base ref.
2. Materialize a worktree at `.caliban/worktrees/<name>/` in the repo root.
3. Optionally apply sparse-checkout patterns to limit which paths are
   checked out.
4. Optionally symlink heavy directories (e.g. `target/`, `node_modules/`)
   from the parent repo into the worktree so they are shared rather than
   duplicated.
5. Run the sub-agent with its working directory set to the worktree root.

The sub-agent's git history (commits, diffs) lives on the `caliban/<name>`
branch. You can inspect, cherry-pick, or discard it with standard git
commands after the run.

## Base ref options

The `worktree.base_ref` field controls what the new branch is rooted on:

| Value | Effect |
|---|---|
| `head` (default) | Branch off the current HEAD commit |
| `fresh` | Branch off HEAD, but start with a near-empty sparse checkout (only a sentinel pattern is checked out) |
| Any rev-parse-able string | Branch off that specific commit, tag, or branch name |

## Sparse checkout

Set `worktree.sparse_paths` to a list of path patterns to limit which files
are materialized in the worktree. Patterns follow git's sparse-checkout cone
format. An empty list (the default) checks out all files.

```json
{
  "prompt": "refactor crates/caliban-tools-builtin",
  "isolation": "worktree",
  "worktree": {
    "base_ref": "head",
    "sparse_paths": ["crates/caliban-tools-builtin/", "Cargo.toml"]
  }
}
```

## Symlinked directories

Large directories that should be shared — not copied — go in
`worktree.symlink_directories`. Each path is relative to the parent repo
root. The directory must exist in the parent at creation time.

```json
{
  "prompt": "run the test suite and summarize failures",
  "isolation": "worktree",
  "worktree": {
    "symlink_directories": ["target", "node_modules"]
  }
}
```

```admonish warning title="Windows symlinks"
Worktree symlink support on Windows requires Developer Mode or elevated
privileges. On Windows, `symlink_directories` is best-effort and may fall
back to copying on systems where symlinks are restricted.
```

## Cleanup behavior

| Context | When the worktree is removed |
|---|---|
| Foreground sub-agent | When the sub-agent's task completes (the handle drops) |
| Background sub-agent | When `caliban agents rm <id>` is run |
| Daemon restart with orphans | On next daemon startup (configurable) |

Set `CALIBAN_KEEP_WORKTREES=1` to disable automatic removal for debugging.
The worktree (and its `caliban/<name>` branch) will then persist until you
remove it manually with `git worktree remove` and `git branch -d`.

## Operator notes

- **Disk usage.** Each worktree is a full checkout of the matched paths.
  Use `sparse_paths` and `symlink_directories` to keep sizes manageable.
  The default `head` base ref shares git objects with the parent repo, so
  only working-tree files consume extra disk.
- **One worktree per sub-agent.** Two concurrent sub-agents with the same
  `name` will conflict. Background fleet agents receive auto-generated names
  based on their id, so fleet-level collisions are not a concern. For
  foreground parallel agents (a future feature), use distinct names.
- **Branch visibility.** `git branch --list 'caliban/*'` shows all active
  sub-agent branches. You can merge, rebase, or delete them like any other
  branch.

For how worktree isolation relates to background agents and the `caliband`
daemon, see [The Background Fleet](background-fleet.md).
