# Pattern Grammar

A pattern is the `pattern` field in a `[[permissions.rules]]` entry (or the argument to `--allow`/`--deny`/`--ask` on the CLI). It encodes the tool name and an optional argument specifier separated by a colon.

## Forms at a glance

| Form | Description |
|------|-------------|
| `Tool` | Match any invocation of `Tool`, regardless of arguments. |
| `Tool:<glob>` | Match `Tool` when its first argument matches `<glob>`. |
| `Bash:~<glob>` | Match `Bash` when `<glob>` appears **anywhere** in the command string. |
| `Tool:key=<glob>` | Match `Tool` when the named input field matches `<glob>` (dotted keys supported). |
| `Tool:k1=<g1>,k2=<g2>` | Multiple key=glob pairs, **AND-combined**. |
| `*` | Catch-all — matches every tool. |

## Glob characters

The argument-side glob uses [`globset`](https://docs.rs/globset) semantics:

| Character | Meaning |
|-----------|---------|
| `*` | Zero or more characters (does **not** cross `/` in path patterns). |
| `**` | Zero or more path segments (crosses `/`; use in file-edit patterns). |
| `?` | Exactly one character. |

Non-path patterns (Bash command strings, URLs, MCP string fields) use `literal_separator = false`, so `*` matches slashes too.

## `Tool:<glob>` — first-argument matching

The "first arg" is a per-tool field extracted from the JSON input:

| Tool | First-arg field |
|------|-----------------|
| `Bash` | `command` |
| `Read`, `Write`, `Edit`, `MultiEdit`, `NotebookEdit` | `path` |
| `WebFetch` | `url` |
| MCP tools with no known accessor | _(no first arg; pattern can't match)_ |

If the tool has no known accessor, only the bare `Tool` form can match; `Tool:<glob>` never fires for that tool.

## `Bash:~<glob>` — anywhere-in-command match

Prefix the argument glob with `~` to perform a sliding-window search over the full command string rather than matching from the start. This catches commands invoked via wrappers or subshells:

```toml
# Deny any use of rm, even via sudo or bash -c "rm …"
[[permissions.rules]]
pattern = "Bash:~rm *"
action  = "deny"
reason  = "no rm — use git revert or Write"
```

The `~` prefix is only meaningful for `Bash`. On other tools it does not match.

## `Tool:key=<glob>` — structured (dotted-key) matching

For MCP tools or built-ins whose input has named fields, use `key=glob` to match a specific field. Dots traverse nested objects:

```toml
# Allow creating GitHub issues only in the anthropic org
[[permissions.rules]]
pattern = "mcp__github__create_issue:repo=anthropic/*"
action  = "allow"

# AND-combined: repo must match AND title must start with "feat"
[[permissions.rules]]
pattern = "mcp__github__create_issue:repo=anthropic/*,title=feat*"
action  = "allow"
```

## File-edit path normalization

For `Read`, `Write`, `Edit`, `MultiEdit`, and `NotebookEdit`, the file path in the tool call is **workspace-normalized** before pattern matching:

- Absolute paths are used as-is.
- Relative paths are resolved against the workspace root (the `git rev-parse --show-toplevel` result, or the current working directory when outside a repo).
- A relative pattern like `src/**/*.rs` is automatically anchored with `**/` so it matches at any depth under the repo.

```toml
# Allow editing any Markdown file anywhere in the repo
[[permissions.rules]]
pattern = "Edit:**/*.md"
action  = "allow"

# Allow editing files only in a specific directory (absolute path)
[[permissions.rules]]
pattern = "Write:/tmp/*"
action  = "allow"
```

## Examples table

| Pattern | Matches | Does not match |
|---------|---------|----------------|
| `Bash` | Any `Bash` call | — |
| `Bash:git *` | `git push`, `git commit -m "…"` | `gitk`, `sudo git push` |
| `Bash:~git *` | `sudo git push`, `bash -c "git fetch"` | commands with no `git ` substring |
| `Bash:rm *` | `rm -rf /tmp` | `sudo rm -rf /tmp` (use `~rm *` for that) |
| `Edit:**/*.rs` | `/repo/src/main.rs`, `/repo/crates/x/lib.rs` | `/tmp/scratch.py` |
| `Write:/tmp/*` | `/tmp/out.txt` | `/home/user/file.txt` |
| `WebFetch:https://docs.*` | `https://docs.rs/…`, `https://docs.anthropic.com/…` | `https://api.example.com/…` |
| `mcp__gh__create_issue:repo=acme/*` | `{"repo":"acme/frontend"}` | `{"repo":"other/repo"}` |
| `*` | Every tool | — |

```admonish note title="Unknown MCP tools"
MCP tools that declare no known first-arg accessor can only be matched by their full name (`mcp__server__tool_name`) or the `*` catch-all. A pattern like `mcp__server__tool_name:<glob>` will never fire for such tools because there is no field to extract.
```
