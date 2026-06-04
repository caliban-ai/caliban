# Permission Modes

Permission modes control what happens when the rule evaluator produces an `ask` verdict. They do **not** override a static `allow` or `deny` — those always win. A mode is just a post-pass filter on top of the rule pipeline.

## The six modes

| Mode | camelCase | What changes |
|------|-----------|-------------|
| Default | `default` | Rules apply unchanged; `ask` routes to the interactive Ask modal. |
| Accept Edits | `acceptEdits` | `Write`, `Edit`, `MultiEdit`, and `NotebookEdit` are auto-allowed; all other tools honor rules normally. |
| Plan | `plan` | Read-only tools are allowed; write and execute tools are blocked from the loop (legacy plan-mode allowlist). |
| Auto | `auto` | A fast classifier model labels each `ask`-rule tool call as allow / soft-deny / hard-deny. Soft-deny routes to the Ask modal with the classifier's reason. |
| Don't Ask | `dontAsk` | Every `ask` verdict becomes `allow`. Static `deny` rules still apply. |
| Bypass Permissions | `bypassPermissions` | All rules ignored — every tool call is allowed. Requires an explicit confirmation flag (see below). |

The status bar shows a chip when the active mode is not `default`:

| Mode | Chip |
|------|------|
| `acceptEdits` | `✎ accept edits` |
| `plan` | `📋 plan` |
| `auto` | `🤖 auto` |
| `dontAsk` | `⏭ don't ask` |
| `bypassPermissions` | `⚠ bypass` |

## Cycling modes with Shift+Tab

In the interactive TUI, press **Shift+Tab** to cycle forward through the modes:

```text
default → acceptEdits → plan → auto → dontAsk → bypassPermissions → default
```

Cycling into `bypassPermissions` without the confirmation flag (see below) fires a warning toast and snaps back to `default`.

## Setting the mode at startup

Use `--permission-mode` on the command line:

```bash
caliban --permission-mode acceptEdits "add docstrings to all public functions"
```

Valid values are the camelCase mode names: `default`, `acceptEdits`, `plan`, `auto`, `dontAsk`, `bypassPermissions`.

The mode is also resolved from the environment variable `CALIBAN_DEFAULT_PERMISSION_MODE` and the `permissions.default_mode` setting (see below), with this precedence:

1. `--permission-mode` CLI flag
2. `CALIBAN_DEFAULT_PERMISSION_MODE` env var
3. `permissions.default_mode` in settings
4. Built-in default (`default`)

## The `default_mode` setting

Set a persistent default mode in your project or user settings file:

```toml
[permissions]
default_mode = "acceptEdits"
```

This is overridden by the CLI flag and env var as shown above.

## Auto-mode and `--disable-auto-mode`

When the mode is `auto`, the classifier is consulted for each tool call whose rule verdict is `ask`. The classifier dispatches via the router's `FastClassifier` purpose — configure it to use a small, fast model (e.g., Haiku, GPT-4o-mini, a local Ollama model). Results are cached for the session by `(tool_name, sha256(input))`.

To disable the classifier (all `ask` verdicts stay as-is, routing to the modal), pass:

```bash
caliban --disable-auto-mode
```

or set `CALIBAN_DISABLE_AUTO_MODE=1`. When disabled, `auto` mode behaves identically to `default`.

## Bypass permissions latch

`bypassPermissions` overrides **all** rules, including static `deny`. Because this is a footgun, caliban refuses to enter the mode without an explicit confirmation flag:

```bash
caliban --allow-dangerously-skip-permissions --permission-mode bypassPermissions
```

Without `--allow-dangerously-skip-permissions`:
- Starting with `--permission-mode bypassPermissions` aborts at startup with an error.
- Configuring `permissions.default_mode = "bypassPermissions"` also aborts at startup.
- Cycling to `bypassPermissions` via Shift+Tab fires a warning toast and reverts to `default`.

```admonish danger title="Bypass is not for routine use"
In bypass mode the model can execute any tool call without restriction. Use it only in fully sandboxed, disposable environments where you control the entire execution context. Prefer `dontAsk` or `acceptEdits` for typical automation.
```

## `--no-permissions`

`--no-permissions` disables the permission system entirely — no rules are evaluated and every tool call is allowed. It conflicts with `--allow`, `--deny`, `--ask`, and `--auto-allow`. The resolved mode surfaces as `"disabled"` in the `system/init` stream-json frame.
