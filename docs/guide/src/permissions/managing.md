# Managing Rules

Rules can be created and edited through three surfaces: the interactive Ask modal that appears when a tool call reaches an `ask` verdict, the `/permissions` overlay inside the TUI, and the `caliban perms` CLI for scripted or headless management.

## The Ask modal

When a tool call hits an `ask` verdict during an interactive session, the TUI pauses and presents a modal with four choices (navigate with arrow keys, confirm with Enter):

| Choice | Effect |
|--------|--------|
| Allow once | Permit this specific tool call and continue; no rule is written. |
| Always allow | Permit this call and append a new `allow` rule to the chosen scope file. |
| Reject once | Deny this specific call; no rule is written. |
| Always reject | Deny this call and append a new `deny` rule to the chosen scope file. |

Press **Esc** to dismiss the modal and deny the current call without writing any rule.

When you choose "Always allow" or "Always reject", caliban opens a sub-prompt with a suggested narrow pattern (e.g., `Bash:git push` rather than `Bash`), a scope picker (`project` / `user`), and an optional comment field. The rule is atomically appended to the appropriate TOML file and takes effect immediately for the rest of the session.

## The `/permissions` overlay

Type `/permissions` in the TUI input bar to open the interactive permissions overlay. It shows:

- The full effective rule list (runtime rules, then config rules by scope, then built-in defaults), each tagged with its origin.
- Runtime-only rules added by "Always allow/reject" during this session.
- Keybind `d` deletes the selected rule: a session rule is dropped from the live store immediately; a file-scoped rule is removed from its TOML file; built-in defaults are read-only.

Use the overlay to inspect the live rule list and verify which rule would match a given tool call before running it.

```admonish note title="Live vs. persisted changes"
Adding a rule through the Ask modal applies to the running session immediately — the next matching tool call won't re-prompt. **Removing** a file-scoped rule (via the overlay's `d` key or `caliban perms remove`), or editing a `permissions.toml` outside caliban, does **not** retroactively change the current session's decisions; those changes take effect at the next session start. Deleting a *session* rule with `d` is the exception — it takes effect live.
```

## `caliban perms` CLI

The `caliban perms` subcommand provides a complete headless management surface. All verbs accept an optional `--scope` flag (`managed` | `user` | `project` | `local` | `cli`; defaults vary by verb).

### `list` — show rules

```bash
# Show the effective merged rule list across all scopes
caliban perms list --effective

# Show only project-scope rules in JSON
caliban perms list --scope project --json
```

Output (human-readable): `  1  allow  Bash:git *`

### `test` — check a tool call

Returns exit code 0 (allow), 1 (deny), or 2 (ask) so it's scriptable.

```bash
# Would `git push` be allowed?
caliban perms test Bash '{"command":"git push"}'
# MATCH: pattern=Bash:git * action=allow

# Would rm be allowed?
caliban perms test Bash '{"command":"rm -rf /tmp"}'
# MATCH: pattern=Bash:rm * action=deny
```

### `explain` — show the full match walk

Prints every rule in evaluation order with a `MATCH` marker next to the first rule that fires. Useful for diagnosing unexpected allow/deny outcomes.

```bash
caliban perms explain Bash '{"command":"sudo rm -rf /"}'
# Rule list (source order; first match wins):
#     1       allow   Bash:git *
#     2 MATCH deny    Bash:~rm *
#     3       ask     Bash
#     ...
```

### `add` — append a rule

Appends a rule to the target scope file (default: `project`).

```bash
# Allow all cargo commands at project scope
caliban perms add "Bash:cargo *" allow --comment "cargo is safe"

# Deny curl at user scope with a reason for the model
caliban perms add "Bash:curl *" deny --scope user --reason "use WebFetch instead"
```

### `remove` — delete a rule

Remove by exact pattern match. Index-based removal is reserved for a future release.

```bash
caliban perms remove --pattern "Bash:cargo *" --scope project
```

### `import` — import rules from another config

Import rules from a Claude Code `settings.json`, a legacy caliban JSON file, or a foreign TOML. Defaults to `user` scope.

```bash
# Dry-run first
caliban perms import --from ~/.claude/settings.json --dry-run

# Actually import into user scope
caliban perms import --from ~/.claude/settings.json --scope user
```

### `export` — export rules to stdout

Outputs the current scope's rules in TOML (default) or JSON format, suitable for redirecting into a new file or piping to another tool.

```bash
# Export project rules as TOML
caliban perms export --scope project

# Export as JSON (three-bucket format for interop)
caliban perms export --scope project --format json
```

### `audit` — inspect the decision log

Reads the JSONL audit log and prints matching entries. See [Headless & Audit](./headless-and-audit.md) for log location and rotation details.

```bash
# Show all deny decisions in the last hour
caliban perms audit --action deny --since 2026-06-01T00:00:00Z

# Show the 20 most recent decisions for the Write tool
caliban perms audit --tool Write --head 20
```

### `lint` — check for duplicate rules

Scans a scope's rule list for duplicate `(pattern, action)` pairs and prints them. Exits 0 if clean, 1 if duplicates are found.

```bash
caliban perms lint --scope project
# OK (no duplicate patterns)

caliban perms lint --scope user
# duplicate: pattern="Bash:git *" action=allow
```

```admonish tip title="Scopes quick reference"
Rules are read from `managed → user → project → local` (earlier scopes shadow later). The `caliban perms add` default scope is `project`; `caliban perms import` defaults to `user`. Use `--scope` to override.
```
