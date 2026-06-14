# Config-hook runtime execution bridge (#121) — Design

**Status:** Design approved 2026-06-14
**Issue:** caliban-ai/caliban#121
**Unblocks:** end-to-end external `[[hooks.SessionStart]]` context injection via the #106 surface
**Follow-up:** #124 (thread scope provenance for precise `allow_managed_hooks_only`)

## Problem

Config-defined `[[hooks.*]]` handlers are parsed and counted but never executed.
`load_hooks_config` (`caliban/src/startup.rs`) produces a `HooksConfig`, but the result
feeds only the summary count + `disable_all_hooks` flag (`caliban/src/main.rs:332-334`).
The agent `Hooks` chain (`caliban/src/startup.rs:~1863`) is composed solely of
`HeadlessHookSink` + `PermissionsHook`. The router handler types (`ShellCommandHook`,
`HttpHook`) are complete but never constructed from `HooksConfig.events` nor added to the
chain. Net: no `[[hooks.*]]` handler fires, for any event.

Additionally, the router handlers only implement `before_tool` / `after_tool`, so even
once wired, a `SessionStart` handler would not fire (no `session_start` impl).

## Design

### 1. The bridge — `build_config_hooks`

New public function in `crates/caliban-agent-core/src/hooks_router.rs`:

```rust
pub fn build_config_hooks(
    cfg: &HooksConfig,
    http_client: reqwest::Client,
) -> Vec<Arc<dyn Hooks + Send + Sync>>
```

Behavior:
- If `cfg.disable_all_hooks` → return empty.
- If `cfg.allow_managed_hooks_only` → return empty + `tracing::warn!` (scope provenance is
  flattened in `HooksConfig`; we cannot prove a handler is managed, so we fire none — the
  safe choice. Precise managed-only firing is #124). This is strictly no worse than today
  (nothing fires now).
- Otherwise iterate `cfg.events` → for each `(event_name, handlers)`, per handler:
  - `Command` → `ShellCommandHook { command, args, timeout, env, matcher, event_name }`
    (skip + warn if `command` is `None`).
  - `Http` → `HttpHook { url, headers, timeout, allowed_url_globs: cfg.allowed_http_hook_urls.clone(), event_name, matcher, client: http_client.clone() }`
    (skip + warn if `url` is `None`).
  - `Mcp` / `Prompt` / `Agent` → **skip with `tracing::warn!`** (v1 stubs, not functional).
    Not silently dropped — the warn names the unsupported kind + event.

Handlers self-route by `event_name` inside each trait method, so all handlers go into one
flat `Vec`.

### 2. `session_start` on the router handlers

The decision (`HookDecision`) is irrelevant for `SessionStart`; we need the handler's raw
stdout / response body to extract `additionalContext`.

`ShellCommandHook`: extract the spawn-and-capture half of `dispatch` into

```rust
async fn run_capture(&self, envelope: serde_json::Value) -> Option<CaptureOutput>;
// CaptureOutput { stdout: String, exit_code: i32 }
```

`dispatch` keeps its current decision semantics by calling `run_capture` then applying
JSON-blob / exit-code rules (no behavior change for `before_tool`/`after_tool`). Add:

```rust
async fn session_start(&self, ctx: &SessionCtx<'_>) -> Result<SessionStartOutcome> {
    if self.event_name != "SessionStart" { return Ok(SessionStartOutcome::default()); }
    let envelope = build_envelope("SessionStart", json!({
        "session_id": ctx.session_id, "cwd": ctx.cwd.display().to_string(),
        "provider": ctx.provider, "model": ctx.model,
    }));
    let ctx_text = self.run_capture(envelope).await
        .and_then(|o| parse_session_start_context(&o.stdout));
    Ok(SessionStartOutcome { additional_context: ctx_text.into_iter().collect() })
}
```

`parse_session_start_context` is the parser shipped in #106. `make_pub` it (currently
`pub(crate)`; the bridge call site is in the same crate, so it stays `pub(crate)` — no
visibility change needed) and drop its `#[allow(dead_code)]` (now used).

`HttpHook`: analogous — add `run_capture` returning the response body, and a `session_start`
that parses `additionalContext` from it.

### 3. Wiring into the agent chain

`build_agent` (`caliban/src/startup.rs:~1801`) gains a `hooks_cfg: &caliban_agent_core::HooksConfig`
parameter. At the composition site (`~1863`), build config handlers and insert them into
`layers` **after `HeadlessHookSink`, before `PermissionsHook`**:

```rust
let mut layers = Vec::new();
if let Some(buf) = hook_event_buffer { layers.push(Arc::new(HeadlessHookSink::new(...))); }
for h in caliban_agent_core::build_config_hooks(hooks_cfg, http_client_for_hooks()) {
    layers.push(h);
}
if let Some(p) = permissions_hook { layers.push(p); }
```

Ordering rationale: a config `PreToolUse` deny should short-circuit before the permission
gate, and the headless sink still observes every event. The HTTP client is the shared
`reqwest::Client` already available in startup (reuse the connection pool).

`build_agent`'s call site in `main.rs` passes the already-loaded `hooks_cfg` (today it is
loaded then only used for the summary; thread the same value in).

### 4. Gating summary

| Condition | Effect |
|---|---|
| `--no-hooks` / `--bare` | `load_hooks_config` returns empty → no handlers |
| `disable_all_hooks` | bridge returns empty |
| `allow_managed_hooks_only` | bridge returns empty + warn (precise firing → #124) |
| `allowed_http_hook_urls` | threaded into each `HttpHook`; enforced at dispatch |

### 5. Testing

Unit (in `hooks_router.rs`):
- `build_config_hooks` returns the right count/types for a mixed config; `disable_all_hooks`
  and `allow_managed_hooks_only` each yield empty; `Mcp`/`Prompt`/`Agent` skipped.

Integration (new `caliban/tests/config_hooks.rs`, real shell scripts — mirror the existing
`crates/caliban-agent-core/tests/hooks_shell.rs` style):
- A `[[hooks.PreToolUse]]` command handler that prints a deny blob blocks a tool dispatch.
- A `[[hooks.SessionStart]]` command handler printing `{"additionalContext": "..."}` →
  the text appears in the resolved system prompt (drive `resolve_system_prompt` after
  `fire_session_start`, as the #106 unit test does, but with a real config + agent chain).
- `disable_all_hooks = true` → the same SessionStart handler contributes nothing.

## Scope / non-goals

- `Mcp` / `Prompt` / `Agent` handler kinds remain stubs (skipped + warned). Wiring them is
  out of scope (their dispatch is unimplemented).
- Precise `allow_managed_hooks_only` (fire managed, filter the rest) is #124.
- Only `session_start` is added to the router handlers here (the #106 end-to-end goal).
  Other events (`UserPromptSubmit`, `PreCompact`, …) for *external* handlers already have
  no router impl and are not required by #121's acceptance; `before_tool`/`after_tool`
  already work. Adding the remaining events is a mechanical follow-up if needed.

## Acceptance criteria (from #121)

- [x] A configured `[[hooks.PreToolUse]]` shell hook fires and its decision is honored.
- [x] A configured `[[hooks.SessionStart]]` shell hook returning `additionalContext`
      reaches the model on turn 1 via the #106 surface.
- [x] `disable_all_hooks` / managed-hooks gating respected (managed → conservative skip).
- [x] Integration coverage.
