# Permissions — Design

**Date:** 2026-05-23
**Status:** Proposed
**Target branch:** `jf/docs/roadmap-post-webfetch`
**Sub-project of:** caliban Rust agent harness
**Depends on:** `caliban-agent-core` (`Hooks` trait), `caliban-tui`,
`caliban-cli`
**Related ADR:** [0020 — Permission rules](../../../adrs/0020-permission-rules.md)

## Goal

Give the operator rule-based control over which tools the agent may
invoke and with which arguments. Plug into the existing
`Hooks::before_tool` extension point so we add a feature, not a parallel
gating system. Match Claude Code's familiar rule format
(`Tool`, `Tool:prefix`, `*`) without inheriting its
classifier-heavy approach.

## Non-goals

- **Classifier-based approval.** Claude Code uses LLM-graded
  command-intent classifiers (`bashClassifier`, `yoloClassifier`).
  Deferred. We start with glob rules and add classification only if
  glob proves insufficient in practice.
- **Shadowed-rule detection.** No warning when a higher-priority rule
  fully covers a lower-priority one. Defer to a future linter.
- **TUI rule editor.** The TUI shows the matched rule but does not
  let operators edit `permissions.toml` from within caliban. They edit
  the file in their editor, or use the "allow permanently / deny
  permanently" Ask flow which appends to the user file.
- **Per-host network ACLs as a separate system.** Host policy for
  `WebFetch` is just a rule like `WebFetch:https://internal.*`. No
  dedicated DSL.
- **Capability tokens, cosign-signed allowlists, etc.** Out of scope.

## Rule schema (TOML)

Both project (`<workspace>/.caliban/permissions.toml`) and user
(`~/.config/caliban/permissions.toml`) files use the same format:

```toml
[[rule]]
tool = "Bash:git *"
action = "allow"

[[rule]]
tool = "Bash:rm *"
action = "deny"
comment = "Refuse rm; the agent should use the Write tool or git revert"

[[rule]]
tool = "WebFetch"
action = "allow"

[[rule]]
tool = "Bash"
action = "ask"
comment = "Anything else bash-related needs a human in the loop."

[[rule]]
tool = "*"
action = "ask"
```

Fields:

- `tool` (string, required) — pattern; see Pattern matching.
- `action` (string, required) — `"allow"`, `"deny"`, or `"ask"`
  (case-insensitive on read; lowercase on write).
- `comment` (string, optional) — shown verbatim in the Ask modal so
  the operator remembers why a rule exists.

Anything else is a parse error (rejected at load time, not silently
ignored).

## Rule resolution order

```
CLI flags  ── (highest priority)
  ↓
project file (`<workspace>/.caliban/permissions.toml`)
  ↓
user file (`~/.config/caliban/permissions.toml`)
  ↓
built-in defaults  ── (lowest priority)
```

Within a single source, first match wins. The final effective rule set
is the concatenation of all sources in priority order, with later
sources appended after earlier ones. Built-in defaults:

```rust
// read-only tools
Rule { tool: "Read",     action: Allow },
Rule { tool: "Grep",     action: Allow },
Rule { tool: "Glob",     action: Allow },
Rule { tool: "WebFetch", action: Ask   }, // network egress: ask
// dangerous tools
Rule { tool: "Bash",     action: Ask   },
Rule { tool: "Write",    action: Ask   },
Rule { tool: "Edit",     action: Ask   },
Rule { tool: "AgentTool",action: Ask   },
// catch-all (covers MCP tools, future builtins)
Rule { tool: "*",        action: Ask   },
```

## Pattern matching

The pattern is `tool_name` optionally followed by `:<first-arg-glob>`.

| Pattern         | Matches                                                  |
| --------------- | -------------------------------------------------------- |
| `Bash`          | any `Bash` invocation                                    |
| `Bash:git *`    | `Bash` whose `command` starts with `git `                |
| `Bash:git push` | `Bash` whose `command` is exactly `git push` (no glob)   |
| `Bash:*`        | same as `Bash`                                           |
| `WebFetch:https://github.com/*` | `WebFetch` to a `github.com` URL          |
| `*`             | any tool                                                 |

"First arg" is tool-defined:

- `Bash` → `command`
- `WebFetch` → `url`
- `Read` / `Write` / `Edit` → `path`
- MCP tools / others → no first-arg by default; only the bare tool
  name pattern matches.

Glob syntax is intentionally narrow: `*` (zero-or-more chars) and `?`
(one char). No character classes, no `**`, no regex. Implemented with
a hand-rolled matcher (~30 LOC) so we don't pull in `globset` for this
one job — but `globset` is a reasonable swap if requirements grow.

## Interactive Ask flow (TUI)

A centered modal overlay (re-uses the overlay infrastructure from
ADR 0013):

```
┌──────────────────────────────────────────────────────────────────┐
│  Permission request                                              │
│                                                                  │
│  Tool:   Bash                                                    │
│  Input:  { "command": "rm -rf node_modules" }                    │
│                                                                  │
│  Matched rule: (default) *  →  ask                               │
│                                                                  │
│  [y] allow once     [Y] allow permanently                        │
│  [n] deny once      [N] deny permanently                         │
│  [Esc] deny                                                      │
└──────────────────────────────────────────────────────────────────┘
```

Keys:

- `y` — `Allow` for this call.
- `Y` — `Allow` for this call **and** append a rule to the user file
  matching the exact `tool` + first-arg as a literal (no glob
  inserted; operator can broaden the rule by hand later).
- `n` — `Deny` for this call.
- `N` — `Deny` for this call **and** append a `deny` rule the same
  way.
- `Esc` / `q` — `Deny` for this call (no rule write).

"Permanently" writes are best-effort. If the user file can't be
written (permission denied, missing parent dir), we log a warning to
the debug log and apply the choice for the current session only.

## Non-interactive fallback

In `--no-tty` mode or when stdout is not a TTY:

- `Allow` and `Deny` rules fire as normal.
- `Ask` rules become `Deny` and the agent sees a synthetic tool result
  `permission denied: would require interactive approval`.
- `--auto-allow` flips `Ask` → `Allow` instead. The flag is documented
  loudly: the help text reads "DANGEROUS: allows the model to run any
  tool, including arbitrary shell commands, without human review.
  Use only in trusted, sandboxed environments."

There is no `--ask=stdin` mode in v1. Prompting on stdin while the
agent is also reading streaming events is fiddly; if real demand
materialises we can add it later as a small `stdin_prompt` adapter.

## CLI flag wiring

```
--allow <PAT>     Add an Allow rule at top priority. Repeatable.
--deny  <PAT>     Add a Deny rule at top priority.  Repeatable.
--ask   <PAT>     Add an Ask rule at top priority.  Repeatable.
--no-permissions  Disable permissions entirely (all calls Allow).
--auto-allow      Treat Ask as Allow in non-interactive mode.
```

`--no-permissions` exists for tests and CI smoke runs. It is mutually
exclusive (via clap) with `--allow`, `--deny`, `--ask`, `--auto-allow`.

## Crate location

Extend `caliban-agent-core` with a `permissions` module
(`crates/caliban-agent-core/src/permissions/{mod,rule,matcher,store,hook}.rs`).
Permissions are small enough that a new crate would be more friction
than value. The Ask prompt is a trait `AskHandler` whose
implementations live where they belong: the TUI provides one
(`caliban-tui::permissions::TuiAskHandler`), CLI wires the
non-interactive variant in `caliban-cli`.

```rust
pub struct PermissionsHook {
    rules: Vec<Rule>,
    ask: Arc<dyn AskHandler + Send + Sync>,
}

#[async_trait]
impl Hooks for PermissionsHook {
    async fn before_tool(&self, ctx: &ToolCtx<'_>) -> Result<HookDecision> {
        match self.match_rule(ctx) {
            Action::Allow => Ok(HookDecision::Allow),
            Action::Deny  => Ok(HookDecision::Deny("permission denied".into())),
            Action::Ask   => self.ask.prompt(ctx).await,
        }
    }
}
```

## Testing strategy

Unit tests in `caliban-agent-core::permissions::tests`:

1. Default rules: `Read` allowed, `Bash` asks, `WebFetch` asks.
2. CLI `--allow Bash` overrides default `Ask` → `Allow`.
3. Project file beats user file when both define `Bash:git *`.
4. First-match-wins inside one source (narrow before catch-all).
5. Pattern: `Bash:git *` matches `git push` but not `gitk`.
6. Pattern: `Bash:rm *` does NOT match `sudo rm -rf /`.
7. Pattern: `*` matches a tool with no first-arg accessor.
8. Glob `?` matches exactly one char.
9. Invalid `action` value → parse error mentions file + line.
10. Empty rules file → empty rule list (no error).
11. Missing user file → silently skipped, no error.
12. `--no-permissions` short-circuits to `Allow` for all tools.
13. Non-interactive + `Ask` → `Deny` synthesis when no `--auto-allow`.
14. Non-interactive + `--auto-allow` + `Ask` → `Allow`.
15. `AskHandler::prompt` is awaited (not bypassed) on `Ask`.
16. `Allow permanently` writes a rule the next session loads.

Integration test via the `caliban-tui` test harness: simulate a
`Bash` invocation, drive the modal with `y` / `N` / `Esc`, assert the
correct hook decision in each case.

## Risks

- **Pattern surprises.** As noted above, `Bash:rm *` does not match
  `sudo rm`. The matched-rule line in the Ask modal mitigates this.
  Operators can write narrower rules; we don't try to be clever.
- **TUI prompt is the long pole.** The Ask modal reuses overlay
  infrastructure, but the input router needs to be aware that the
  agent loop is *blocked* on the prompt. Mitigation: the
  `PermissionsHook::before_tool` simply awaits a `oneshot::Receiver`
  the TUI fills in.
- **Write-on-Allow-permanently races.** If two `Ask` prompts fire
  concurrently and both write to the user file, we could clobber.
  Acceptable v1 — concurrent tool calls are uncommon and we use
  exclusive open + atomic-rename for the write.
- **No audit log.** Every decision is debug-logged via `tracing` but
  there's no persistent permission-decision log. Add later if needed
  via a second `Hooks` impl.

## Acceptance criteria

- `cargo build --workspace` clean; `cargo clippy --workspace
  --all-targets -- -D warnings` clean; `cargo fmt --all -- --check`
  clean.
- `cargo test --workspace` passes — adds the 16 unit tests above plus
  ≥ 2 TUI integration tests.
- `PermissionsHook` registered in the default `caliban` binary; can be
  disabled with `--no-permissions`.
- README's safety section documents the default `Ask`-everything
  posture and points at the TOML format.
- A `permissions.toml` example file exists at
  `docs/examples/permissions.example.toml`.
