# Headless & Audit

## Headless mode and the `ask` verdict

When caliban runs without a TTY — in CI, in a script, or via `caliban -p` — there is no interactive modal to present. Any tool call that reaches an `ask` verdict is handled by `NonInteractiveAskHandler`:

- **Default behavior (no flags):** `ask` becomes a hard deny. The tool call fails with a permission error message that names a concrete remediation.
- **With `--auto-allow`:** every `ask` verdict becomes `allow`. This is equivalent to `dontAsk` mode for the duration of the run.

The deny message is tailored to the tool class:

| Tool class | Suggested remediation |
|------------|-----------------------|
| File-edit (`Write`, `Edit`, `MultiEdit`, `NotebookEdit`) | `--permission-mode acceptEdits` or a narrow `--allow` rule |
| `Bash` | `--allow 'Bash(<glob>)'` for a targeted rule, or `--auto-allow` (flagged dangerous) |
| Other tools | `--allow '<Tool>'` or `--auto-allow` |

### Opt-in strategies

Choose the least-permissive option that satisfies the task:

```bash
# Allow only file edits (most common CI use case)
caliban -p "update version in Cargo.toml" --permission-mode acceptEdits

# Allow specific git commands
caliban -p "commit and push" --allow "Bash:git *"

# Allow all ask-rule tools (use with care)
caliban -p "run the full refactor" --auto-allow
```

You can also set rules in the project's `permissions.toml` so they apply without CLI flags:

```toml
[[permissions.rules]]
pattern = "Bash:git *"
action  = "allow"
comment = "safe for CI"
```

## The JSONL audit log

Every tool-call decision (allow, deny, or ask) is appended to an append-only JSONL file.

### Log location

| Platform | Path |
|----------|------|
| Linux | `$XDG_STATE_HOME/caliban/permission-decisions.jsonl` (default: `~/.local/state/caliban/`) |
| macOS | `$XDG_DATA_HOME/caliban/permission-decisions.jsonl` (default: `~/.local/state/caliban/`) |

The `audit_log` setting controls whether logging is active:

```toml
[permissions]
audit_log = true   # default; set false to disable
```

### Log format

Each line is a JSON object:

```json
{
  "ts": "2026-06-01T14:23:01.123456Z",
  "session_id": "s_abc123",
  "turn_index": 4,
  "tool_use_id": "tu_xyz",
  "tool_name": "Bash",
  "input_excerpt": "{\"command\":\"git push origin main\"}",
  "action": "allow",
  "matched_rule": {
    "pattern": "Bash:git *",
    "action": "allow"
  }
}
```

`input_excerpt` is truncated to 256 characters and newlines are replaced with spaces.

### Log rotation

When the log file exceeds **100 MiB**, caliban automatically:

1. Renames the current file to `permission-decisions-YYYY-MM-DD.jsonl`.
2. Gzip-compresses the renamed file to `permission-decisions-YYYY-MM-DD.jsonl.gz`.
3. Removes the uncompressed renamed file.
4. Opens a fresh `permission-decisions.jsonl` for subsequent writes.

Rotated archives accumulate in the same directory. Remove old `.gz` files manually when disk space is a concern.

### Querying the log

Use `caliban perms audit` to filter and display log entries:

```bash
# All decisions since midnight UTC
caliban perms audit --since 2026-06-01T00:00:00Z

# Only denials for the Write tool
caliban perms audit --tool Write --action deny

# Most recent 50 entries
caliban perms audit --head 50

# Combine filters
caliban perms audit --tool Bash --action allow --since 2026-06-01T00:00:00Z --head 100
```

Exit code is always 0; an empty result prints `(empty)`.

## Hardening with `permissions.enforce`

The `permissions.enforce` flag prevents the bypass latch from being used, even when `--allow-dangerously-skip-permissions` is passed:

```toml
[permissions]
enforce = true
```

With `enforce = true`, caliban refuses to start if `--allow-dangerously-skip-permissions` is on the command line or if `permissions.default_mode` is set to `bypassPermissions`. This is useful for team or managed deployments where operators want to guarantee that static `deny` rules can never be overridden.

```admonish warning title="enforce is a deployment-level setting"
Set `permissions.enforce = true` in the managed or user scope, not project scope, so it cannot be overridden by project-level config. A project can always set a lower-priority rule, but only higher-priority scopes can lock out bypass.
```
