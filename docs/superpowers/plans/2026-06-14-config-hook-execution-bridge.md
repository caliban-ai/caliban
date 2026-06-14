# Config-hook runtime execution bridge (#121) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Construct executing `ShellCommandHook`/`HttpHook` trait objects from `HooksConfig.events`, compose them into the agent `Hooks` chain, and add `session_start` to the router handlers so external `[[hooks.SessionStart]]` hooks inject `additionalContext` via the #106 surface.

**Architecture:** A `build_config_hooks(cfg, client) -> Vec<Arc<dyn Hooks>>` bridge in `hooks_router.rs`, gated by `disable_all_hooks` / `allow_managed_hooks_only`. Router handlers gain a `run_capture`/`fetch_body` helper (factored out of `dispatch`, no behavior change) plus a `session_start` impl that parses `additionalContext`. `build_agent` threads the already-loaded `HooksConfig` and inserts the handlers between the headless sink and the permission gate.

**Tech Stack:** Rust, `async-trait`, `serde_json`, `tokio`, `reqwest`. Crate: `caliban-ai/caliban`.

**Spec:** `docs/superpowers/specs/2026-06-14-config-hook-execution-bridge-design.md`
**Follow-up:** #124 (precise `allow_managed_hooks_only` via scope provenance).

**Verification gate (CLAUDE.md / CI):**
```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --all-targets
cargo test --workspace
```

---

## File Structure

- **Modify** `crates/caliban-agent-core/src/hooks_router.rs` — factor `run_capture` (Shell) / `fetch_body` (Http) out of `dispatch`; add `session_start` to both; add `build_config_hooks`; drop `#[allow(dead_code)]` on `parse_session_start_context`; unit tests.
- **Modify** `crates/caliban-agent-core/src/lib.rs` — re-export `build_config_hooks`.
- **Modify** `caliban/src/startup.rs` — `build_agent` gains `hooks_cfg` param; compose config handlers into `layers`.
- **Modify** `caliban/src/main.rs` — pass `&hooks_cfg` into `build_agent`; drop the `let _ = hooks_cfg_summary` no-op if it becomes used (leave summary as-is otherwise).
- **Add** `crates/caliban-agent-core/tests/config_hooks.rs` — integration: SessionStart additionalContext, PreToolUse deny, gating.
- **Modify** `docs/parity-gap-matrix.md` — advance the §B SessionStart context-injection row to ✅ for config hooks; update the handler-types note.

---

## Task 1: Factor capture helpers + add `session_start` to router handlers

**Files:** Modify `crates/caliban-agent-core/src/hooks_router.rs`

- [ ] **Step 1: Add a `CaptureOutput` struct + `run_capture` for `ShellCommandHook`**

Add near the top of the ShellCommandHook impl. Move the spawn/stdin/wait/stdout-stderr logic out of `dispatch` into `run_capture`, returning the raw stdout + exit code (the existing decision logic stays in `dispatch`):

```rust
struct CaptureOutput {
    stdout: String,
    exit_code: i32,
}

impl ShellCommandHook {
    /// Spawn the child, send the envelope, capture stdout + exit code.
    /// `None` on spawn/wait/timeout failure (caller treats as no-op / Allow).
    async fn run_capture(&self, envelope: serde_json::Value) -> Option<CaptureOutput> {
        let payload = serde_json::to_string(&envelope)
            .map_err(|e| tracing::warn!(error = %e, "shell hook: failed to serialize envelope"))
            .ok()?;
        let mut cmd = tokio::process::Command::new(&self.command);
        cmd.args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        let mut child = match spawn_with_retry(&mut cmd, &self.command).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(command = %self.command, error = %e, "shell hook: spawn failed");
                return None;
            }
        };
        if let Some(mut stdin) = child.stdin.take()
            && let Err(e) = stdin.write_all(payload.as_bytes()).await
        {
            tracing::warn!(error = %e, "shell hook: stdin write failed");
        }
        let output = match tokio::time::timeout(self.timeout, child.wait_with_output()).await {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "shell hook: wait failed");
                return None;
            }
            Err(_) => {
                tracing::warn!(
                    command = %self.command,
                    timeout_ms = u64::try_from(self.timeout.as_millis()).unwrap_or(u64::MAX),
                    "shell hook: timeout exceeded"
                );
                return None;
            }
        };
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr_text = String::from_utf8_lossy(&output.stderr).into_owned();
        let truncated_stderr = truncate_kb(&stderr_text, 8);
        if !truncated_stderr.is_empty() {
            tracing::debug!(command = %self.command, hook_stderr = %truncated_stderr, "shell hook: stderr captured");
        }
        Some(CaptureOutput { stdout, exit_code: output.status.code().unwrap_or(0) })
    }
}
```

- [ ] **Step 2: Rewrite `dispatch` on top of `run_capture` (no behavior change)**

Replace the body of the existing `dispatch` with:

```rust
    async fn dispatch(&self, envelope: serde_json::Value) -> HookDecision {
        let Some(out) = self.run_capture(envelope).await else {
            return HookDecision::Allow;
        };
        // Prefer JSON on stdout when present.
        let from_json = parse_decision_blob(&out.stdout);
        if !matches!(from_json, HookDecision::Allow) || out.stdout.trim().starts_with('{') {
            return from_json;
        }
        // Fall back to exit-code semantics.
        match out.exit_code {
            0 => HookDecision::Allow,
            2 => HookDecision::Deny(format!("hook `{}` exited 2", self.command)),
            other => {
                tracing::warn!(command = %self.command, exit_code = other, "shell hook: non-zero exit treated as Allow");
                HookDecision::Allow
            }
        }
    }
```

Note: the exit-2 deny message no longer includes stderr (stderr is now logged in `run_capture`, not returned). If the existing `hooks_shell.rs` test `exit_two_is_deny_with_stderr_reason` asserts the stderr text in the reason, preserve that behavior instead: have `run_capture` also return `stderr` and keep the original message. Check that test first (Step 3) and choose the variant that keeps it green.

- [ ] **Step 3: Check the existing shell tests' expectations before finalizing dispatch**

Run:
```bash
sed -n '/exit_two_is_deny_with_stderr_reason/,/^}/p' crates/caliban-agent-core/tests/hooks_shell.rs
```
If it asserts the stderr string is in the `Deny` reason, extend `CaptureOutput` with `stderr: String` and restore the original deny message (`if truncated_stderr.is_empty() { format!(...) } else { truncated_stderr }`). Re-run `cargo test -p caliban-agent-core --test hooks_shell` and confirm all pass.

- [ ] **Step 4: Add `session_start` to `ShellCommandHook`**

In `impl Hooks for ShellCommandHook`, add (import `SessionCtx`, `SessionStartOutcome`, `build_envelope` as needed — they're in `crate::hooks`):

```rust
    async fn session_start(
        &self,
        ctx: &crate::hooks::SessionCtx<'_>,
    ) -> Result<crate::hooks::SessionStartOutcome> {
        if self.event_name != "SessionStart" {
            return Ok(crate::hooks::SessionStartOutcome::default());
        }
        let envelope = crate::hooks::build_envelope(
            "SessionStart",
            serde_json::json!({
                "session_id": ctx.session_id,
                "cwd": ctx.cwd.display().to_string(),
                "provider": ctx.provider,
                "model": ctx.model,
            }),
        );
        let additional_context: Vec<String> = self
            .run_capture(envelope)
            .await
            .and_then(|o| parse_session_start_context(&o.stdout))
            .into_iter()
            .collect();
        Ok(crate::hooks::SessionStartOutcome { additional_context })
    }
```

- [ ] **Step 5: Add `fetch_body` + `session_start` to `HttpHook`**

Factor the send + body-read half of `HttpHook::dispatch` into `async fn fetch_body(&self, envelope) -> Option<String>` (returns the response body on a 2xx; `None` otherwise — mirror the existing warn/Allow branches but return `None`). Rewrite `dispatch` to `match self.fetch_body(envelope).await { Some(b) => parse_decision_blob(&b), None => HookDecision::Allow }`. Then add `session_start` mirroring Step 4 but using `fetch_body` and the same envelope.

- [ ] **Step 6: Drop the dead-code allowance on `parse_session_start_context`**

Remove `#[allow(dead_code)] // invoked by the config-hook execution bridge (#121)` above `parse_session_start_context` — it is now called by the handlers.

- [ ] **Step 7: Build + run existing shell tests**

```bash
cargo build -p caliban-agent-core
cargo test -p caliban-agent-core --test hooks_shell
```
Expected: builds; all existing shell tests pass.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(hooks): add session_start to router handlers; factor run_capture/fetch_body (#121)"
```

---

## Task 2: `build_config_hooks` bridge + unit tests

**Files:** Modify `crates/caliban-agent-core/src/hooks_router.rs`, `crates/caliban-agent-core/src/lib.rs`

- [ ] **Step 1: Write the bridge**

Add to `hooks_router.rs` (imports: `std::sync::Arc`, `crate::hooks::Hooks`, `crate::hooks_config::{HooksConfig, HookHandlerType}`):

```rust
/// Build executing hook trait objects from a parsed [`HooksConfig`], for
/// composition into the agent `Hooks` chain. Returns an empty vec when hooks
/// are globally disabled.
///
/// `allow_managed_hooks_only` currently yields an empty vec + a warning: the
/// flattened `HooksConfig` has lost per-handler scope, so we cannot prove a
/// handler is managed and conservatively fire none (precise firing → #124).
///
/// `Mcp` / `Prompt` / `Agent` handler kinds are v1 stubs and are skipped with a
/// warning (not silently dropped).
#[must_use]
pub fn build_config_hooks(
    cfg: &crate::hooks_config::HooksConfig,
    http_client: reqwest::Client,
) -> Vec<Arc<dyn crate::hooks::Hooks + Send + Sync>> {
    if cfg.disable_all_hooks {
        return Vec::new();
    }
    if cfg.allow_managed_hooks_only {
        tracing::warn!(
            "allow_managed_hooks_only is set but handler scope is not tracked; \
             firing no config hooks (see #124)"
        );
        return Vec::new();
    }
    let mut out: Vec<Arc<dyn crate::hooks::Hooks + Send + Sync>> = Vec::new();
    for (event_name, handlers) in &cfg.events {
        for h in handlers {
            match h.kind {
                crate::hooks_config::HookHandlerType::Command => {
                    let Some(command) = h.command.clone() else {
                        tracing::warn!(event = %event_name, "command hook missing `command`; skipping");
                        continue;
                    };
                    out.push(Arc::new(ShellCommandHook {
                        command,
                        args: h.args.clone(),
                        timeout: h.timeout,
                        env: h.env.clone(),
                        matcher: h.matcher.clone(),
                        event_name: event_name.clone(),
                    }));
                }
                crate::hooks_config::HookHandlerType::Http => {
                    let Some(url) = h.url.clone() else {
                        tracing::warn!(event = %event_name, "http hook missing `url`; skipping");
                        continue;
                    };
                    out.push(Arc::new(HttpHook {
                        url,
                        headers: h.headers.clone(),
                        timeout: h.timeout,
                        allowed_url_globs: cfg.allowed_http_hook_urls.clone(),
                        event_name: event_name.clone(),
                        matcher: h.matcher.clone(),
                        client: http_client.clone(),
                    }));
                }
                crate::hooks_config::HookHandlerType::Mcp
                | crate::hooks_config::HookHandlerType::Prompt
                | crate::hooks_config::HookHandlerType::Agent => {
                    tracing::warn!(
                        event = %event_name,
                        kind = ?h.kind,
                        "config hook kind not yet executable at runtime; skipping"
                    );
                }
            }
        }
    }
    out
}
```

- [ ] **Step 2: Re-export from lib.rs**

In `crates/caliban-agent-core/src/lib.rs`, add `build_config_hooks` to the `hooks_router::{...}` re-export line (alongside `ShellCommandHook`, `HttpHook`):

```rust
pub use hooks_router::{AgentHook, HttpHook, McpHook, PromptHook, ShellCommandHook, build_config_hooks};
```

- [ ] **Step 3: Write unit tests for the bridge**

In the `hooks_router.rs` `#[cfg(test)]` module, add (build configs via `HooksConfig::from_str` with TOML):

```rust
fn test_client() -> reqwest::Client {
    reqwest::Client::builder().build().unwrap()
}

#[test]
fn bridge_builds_command_and_http_skips_stubs() {
    let toml = r#"
[[hooks.PreToolUse]]
matcher = "Bash"
[[hooks.PreToolUse.handlers]]
type = "command"
command = "/bin/true"
[[hooks.SessionStart]]
[[hooks.SessionStart.handlers]]
type = "mcp"
mcp_server = "srv"
mcp_tool = "t"
"#;
    let cfg = crate::hooks_config::HooksConfig::from_str(toml, std::path::Path::new("test")).unwrap();
    let hooks = build_config_hooks(&cfg, test_client());
    // 1 command handler built; the mcp stub is skipped.
    assert_eq!(hooks.len(), 1);
}

#[test]
fn bridge_disable_all_hooks_is_empty() {
    let toml = r#"
disable_all_hooks = true
[[hooks.PreToolUse]]
[[hooks.PreToolUse.handlers]]
type = "command"
command = "/bin/true"
"#;
    let cfg = crate::hooks_config::HooksConfig::from_str(toml, std::path::Path::new("test")).unwrap();
    assert!(build_config_hooks(&cfg, test_client()).is_empty());
}

#[test]
fn bridge_managed_only_is_empty() {
    let toml = r#"
allow_managed_hooks_only = true
[[hooks.PreToolUse]]
[[hooks.PreToolUse.handlers]]
type = "command"
command = "/bin/true"
"#;
    let cfg = crate::hooks_config::HooksConfig::from_str(toml, std::path::Path::new("test")).unwrap();
    assert!(build_config_hooks(&cfg, test_client()).is_empty());
}
```

Verify the TOML field names (`type`, `command`, `mcp_server`, `mcp_tool`) against the `RawConfig`/`RawHandler` shapes in `hooks_config.rs` (read the `#[serde(rename = ...)]` attributes ~line 257+); adjust the test TOML to match exactly.

- [ ] **Step 4: Run the bridge tests**

```bash
cargo test -p caliban-agent-core --lib build_config_hooks bridge_
```
Expected: 3 tests pass. (If field names were wrong, `from_str` returns `Invalid`; fix the TOML.)

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(hooks): build_config_hooks bridge from HooksConfig with gating (#121)"
```

---

## Task 3: Wire the bridge into the agent chain

**Files:** Modify `caliban/src/startup.rs`, `caliban/src/main.rs`

- [ ] **Step 1: Add `hooks_cfg` param to `build_agent`**

In `caliban/src/startup.rs:~1801`, add a parameter (after `settings_snapshot`):

```rust
    settings_snapshot: &caliban_settings::Settings,
    hooks_cfg: &caliban_agent_core::HooksConfig,
```

- [ ] **Step 2: Compose config handlers into the chain**

At the hook-composition block (`~1863`), insert config handlers after the sink, before permissions:

```rust
    {
        let mut layers: Vec<Arc<dyn caliban_agent_core::Hooks>> = Vec::new();
        if let Some(buf) = hook_event_buffer {
            layers.push(Arc::new(headless::HeadlessHookSink::new(Arc::clone(buf))));
        }
        for h in caliban_agent_core::build_config_hooks(hooks_cfg, web_fetch_client()) {
            layers.push(h);
        }
        if let Some(p) = permissions_hook {
            layers.push(p as Arc<dyn caliban_agent_core::Hooks>);
        }
        if !layers.is_empty() {
            let composite: Arc<dyn caliban_agent_core::Hooks + Send + Sync> =
                Arc::new(caliban_agent_core::CompositeHooks::new(layers));
            builder = builder.hooks(composite);
        }
    }
```

(`web_fetch_client()` is the shared `reqwest::Client` builder already in `startup.rs:504`.)

- [ ] **Step 3: Pass `&hooks_cfg` at the call site**

In `caliban/src/main.rs:394`, add `&hooks_cfg,` to the `build_agent(...)` argument list (after `&settings_snapshot,`). `hooks_cfg` is already in scope (loaded at ~328). The existing `let _ = hooks_cfg_summary;` line can stay.

- [ ] **Step 4: Build the workspace**

```bash
cargo build --workspace --all-targets
```
Expected: success. Fix any other `build_agent(` call sites the compiler flags (grep: `rg -n "build_agent\(" caliban/src` — expect main.rs + any test; update tests to pass `&HooksConfig::default()`).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(startup): compose config-defined hooks into the agent chain (#121)"
```

---

## Task 4: Integration tests (real shell scripts)

**Files:** Create `crates/caliban-agent-core/tests/config_hooks.rs`

- [ ] **Step 1: Write the integration test**

Mirror `tests/hooks_shell.rs` (unix-gated, `write_script` helper). Drive the bridge output through `CompositeHooks`:

```rust
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use caliban_agent_core::{
    CompositeHooks, HookDecision, Hooks, HooksConfig, SessionCtx, ToolCtx, build_config_hooks,
};
use tempfile::TempDir;

fn write_script(dir: &TempDir, name: &str, body: &str) -> std::path::PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, body).unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().build().unwrap()
}

#[tokio::test]
async fn session_start_hook_injects_additional_context() {
    let dir = TempDir::new().unwrap();
    let script = write_script(
        &dir,
        "ctx.sh",
        "#!/bin/sh\necho '{\"additionalContext\": \"INJECTED-FROM-HOOK\"}'\n",
    );
    let toml = format!(
        "[[hooks.SessionStart]]\n[[hooks.SessionStart.handlers]]\ntype = \"command\"\ncommand = \"{}\"\n",
        script.display()
    );
    let cfg = HooksConfig::from_str(&toml, Path::new("test")).unwrap();
    let composite = CompositeHooks::new(build_config_hooks(&cfg, client()));
    let cwd = std::env::current_dir().unwrap();
    let ctx = SessionCtx { session_id: "s", cwd: &cwd, provider: "test", model: "m" };
    let out = composite.session_start(&ctx).await.unwrap();
    assert_eq!(out.additional_context, vec!["INJECTED-FROM-HOOK".to_string()]);
}

#[tokio::test]
async fn pretooluse_hook_denies() {
    let dir = TempDir::new().unwrap();
    let script = write_script(
        &dir,
        "deny.sh",
        "#!/bin/sh\necho '{\"hookSpecificOutput\": {\"permissionDecision\": \"deny\", \"permissionDecisionReason\": \"nope\"}}'\n",
    );
    let toml = format!(
        "[[hooks.PreToolUse]]\nmatcher = \"Bash\"\n[[hooks.PreToolUse.handlers]]\ntype = \"command\"\ncommand = \"{}\"\n",
        script.display()
    );
    let cfg = HooksConfig::from_str(&toml, Path::new("test")).unwrap();
    let composite = CompositeHooks::new(build_config_hooks(&cfg, client()));
    let input = serde_json::json!({});
    let ctx = ToolCtx { turn_index: 0, tool_use_id: "t1", tool_name: "Bash", input: &input };
    let d = composite.before_tool(&ctx).await.unwrap();
    assert!(matches!(d, HookDecision::Deny(_)));
}

#[tokio::test]
async fn disable_all_hooks_fires_nothing() {
    let dir = TempDir::new().unwrap();
    let script = write_script(&dir, "ctx.sh", "#!/bin/sh\necho '{\"additionalContext\": \"X\"}'\n");
    let toml = format!(
        "disable_all_hooks = true\n[[hooks.SessionStart]]\n[[hooks.SessionStart.handlers]]\ntype = \"command\"\ncommand = \"{}\"\n",
        script.display()
    );
    let cfg = HooksConfig::from_str(&toml, Path::new("test")).unwrap();
    let composite = CompositeHooks::new(build_config_hooks(&cfg, client()));
    let cwd = std::env::current_dir().unwrap();
    let ctx = SessionCtx { session_id: "s", cwd: &cwd, provider: "test", model: "m" };
    let out = composite.session_start(&ctx).await.unwrap();
    assert!(out.additional_context.is_empty());
}
```

Confirm `SessionCtx`, `ToolCtx`, `CompositeHooks`, `HooksConfig` are all re-exported from `caliban_agent_core` (they are — verify against `lib.rs`).

- [ ] **Step 2: Run the integration tests**

```bash
cargo test -p caliban-agent-core --test config_hooks
```
Expected: 3 tests pass.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "test(hooks): integration coverage for config-hook execution + session_start (#121)"
```

---

## Task 5: Verification gate, parity matrix, PR

- [ ] **Step 1: Full gate**

```bash
cargo fmt --all -- --check   # or `cargo fmt --all` then re-check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --all-targets
cargo test --workspace
```
All four pass.

- [ ] **Step 2: Advance the parity matrix**

In `docs/parity-gap-matrix.md`, update the §B SessionStart context-injection row (added in #106) from 🟡 → ✅ (config hooks now execute end-to-end), and update the handler-types row note if it still implies config hooks don't run. Commit.

- [ ] **Step 3: Push + PR**

```bash
git push -u origin worktree-issue-121-execute-config-hooks
gh pr create --repo caliban-ai/caliban --base main \
  --title "feat(hooks): execute config-defined [[hooks.*]] handlers at runtime (#121)" \
  --body "Closes #121. Builds executing ShellCommandHook/HttpHook from HooksConfig.events and composes them into the agent Hooks chain (after the headless sink, before the permission gate). Adds session_start to the router handlers so external [[hooks.SessionStart]] hooks inject additionalContext via the #106 surface end-to-end. Gating: disable_all_hooks and allow_managed_hooks_only honored (managed → conservative skip; precise firing tracked in #124). Mcp/Prompt/Agent kinds skipped with a warning (v1 stubs).

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```

---

## Self-Review

**Spec coverage:** bridge (Task 2) ✓; session_start on handlers (Task 1) ✓; wiring (Task 3) ✓; gating (Task 2 + spec table) ✓; testing (Tasks 2, 4) ✓; parity (Task 5) ✓.

**Placeholder scan:** Step 3 of Task 1 and Step 3 of Task 2 instruct verifying existing test expectations / TOML field names against the real source before finalizing — these are deliberate guards (the exact deny-message contract and `RawHandler` serde names must be read), not placeholders.

**Type consistency:** `CaptureOutput { stdout, exit_code[, stderr] }`, `run_capture -> Option<CaptureOutput>`, `fetch_body -> Option<String>`, `build_config_hooks(&HooksConfig, reqwest::Client) -> Vec<Arc<dyn Hooks + Send + Sync>>`, `build_agent(..., hooks_cfg: &HooksConfig)` — consistent across tasks.
