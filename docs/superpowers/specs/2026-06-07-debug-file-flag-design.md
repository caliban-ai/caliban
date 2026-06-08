# Design: `--debug-file` flag (redirect debug log to a chosen path)

**Date:** 2026-06-07
**Status:** Approved (design)
**Topic:** observability — debug-log destination override
**Tracks:** caliban-ai/caliban#26

## Goal

`--debug` is wired (`args.rs:247`, drained by `startup::init_debug_tracing`,
`startup.rs:43-84`) and always logs to a fixed location:
`~/.cache/caliban/debug.log` (`~/Library/Caches/caliban/debug.log` on macOS).
There is no way to redirect that output. Add a `--debug-file <PATH>` flag that
sends the file-backed `tracing` log to an operator-chosen path instead.

## Background

`init_debug_tracing` does three things today:

1. Decides whether debug logging is on: `args.debug || CALIBAN_DEBUG is set`.
2. Computes the log path: `dirs::cache_dir()/caliban/debug.log`.
3. Installs a global `tracing` subscriber writing (append mode) to that file.

Only step 2 is hardcoded. The change threads an optional override into step 2
and lets the override also satisfy step 1 (specifying a file means you want
debug logging).

## Decisions

- **Flag shape:** `--debug-file <PATH>` (`value_name = "PATH"`,
  `env = "CALIBAN_DEBUG_FILE"`). The env var mirrors every other path/budget
  flag in `Args` (e.g. `--sessions-dir`, `--max-attach-bytes`) and lets CI set
  it without editing argv. Type: `Option<PathBuf>`.

- **`--debug-file` implies debug-on.** If the user names a destination they
  clearly want logging. So the enable predicate becomes:
  `args.debug || args.debug_file.is_some() || CALIBAN_DEBUG is set`.
  This avoids a confusing "I passed `--debug-file` but got no log" footgun and
  needs no `requires("debug")` clap constraint.

- **Override wins over the default.** When `debug_file` is `Some`, that exact
  path is used verbatim (relative paths resolve against CWD, as `OpenOptions`
  already does). When it is `None`, the existing cache-dir default is kept
  unchanged — fully backward compatible.

- **Parent-dir creation & append semantics are unchanged.** The existing code
  already `create_dir_all`s the parent and opens with `.create(true).append(true)`;
  both apply equally to an overridden path. Append (not truncate) is preserved
  so repeated runs accumulate, matching the current `--debug` contract.

- **Testability:** extract the enable + path decision into two small pure
  helpers — `debug_enabled(&Args) -> bool` and
  `resolve_debug_log_path(&Args) -> Option<PathBuf>` — so the logic is unit
  testable without installing the process-global subscriber (which can only
  init once per process and can't be asserted on directly).

## Non-goals

- No log rotation, size cap, or stdout/stderr streaming (separate issues:
  `--debug-file` is purely a destination override).
- No change to the `RUST_LOG` / `EnvFilter` behavior or the default filter.
- No new `--debug-file` handling in the `caliban router debug` subcommand
  family (unrelated namespace).

## Implementation sketch

`args.rs` — new field after `debug`:

```rust
/// Redirect `--debug` output to this path instead of the default
/// `~/.cache/caliban/debug.log`. Implies `--debug`. Relative paths
/// resolve against the current directory.
#[arg(long, value_name = "PATH", env = "CALIBAN_DEBUG_FILE")]
pub(crate) debug_file: Option<PathBuf>,
```

`startup.rs` — replace the inline enable + path computation in
`init_debug_tracing` with:

```rust
fn debug_enabled(args: &Args) -> bool {
    args.debug || args.debug_file.is_some() || std::env::var("CALIBAN_DEBUG").is_ok()
}

fn resolve_debug_log_path(args: &Args) -> Option<PathBuf> {
    if !debug_enabled(args) {
        return None;
    }
    if let Some(path) = args.debug_file.clone() {
        return Some(path);
    }
    dirs::cache_dir().map(|d| d.join("caliban").join("debug.log"))
}
```

`init_debug_tracing` then becomes: `let Some(log_path) = resolve_debug_log_path(args) else { return };`
followed by the existing parent-dir creation, file open, and subscriber install.

## Test plan (TDD)

Unit tests in `startup.rs`:

1. `resolve_debug_log_path` returns `None` with no debug flags (guarded on
   `CALIBAN_DEBUG` being unset to stay non-flaky).
2. `--debug-file /tmp/x.log` → `Some("/tmp/x.log")` exactly (override wins).
3. `--debug-file` alone enables logging without `--debug` (implies-debug).
4. `--debug` alone → `Some(path)` ending in `caliban/debug.log` (default kept).

Flag-parsing test in `args.rs` tests module:

5. `--debug-file path` parses into `Some(PathBuf)`.

CI gate: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
-- -D warnings`, `cargo build --workspace --all-targets`, `cargo test --workspace`.
```
