//! External hook handler types: `ShellCommandHook`, `HttpHook`, `PromptHook`,
//! `AgentHook`, `McpHook`. Each implements [`Hooks`] so they compose into the
//! existing pipeline via [`crate::hooks::CompositeHooks`].
//!
//! v1 scope: `ShellCommandHook` + `HttpHook` are fully wired; `PromptHook`,
//! `AgentHook`, `McpHook` are registered as stubs that log a warning and
//! return `Allow`. The real wiring lands with ADRs 0023 (MCP v2) and 0037
//! (sub-agent supervisor).
//!
//! All external handlers observe the `camelCase` event-JSON contract:
//! `hookEventName` and `hookSpecificOutput` are `camelCase`; everything else
//! (`session_id`, `tool_use_id`, `turn_index`) is `snake_case`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::io::AsyncWriteExt as _;

use crate::error::Result;
use crate::hooks::{HookDecision, Hooks, ToolCtx};

// ---------------------------------------------------------------------------
// Parsed decision JSON (`hookSpecificOutput`)
// ---------------------------------------------------------------------------

/// Decision JSON shape emitted by external handlers. `permission_decision`
/// drives the resulting [`HookDecision`]; `updated_input` is honored when
/// `permission_decision` is `Allow` or absent.
#[derive(Debug, Deserialize, Default)]
struct HookSpecificOutput {
    #[serde(rename = "permissionDecision")]
    permission_decision: Option<String>,
    #[serde(rename = "permissionDecisionReason")]
    permission_decision_reason: Option<String>,
    #[serde(rename = "updatedInput")]
    updated_input: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Default)]
struct DecisionEnvelope {
    #[serde(rename = "hookSpecificOutput", default)]
    hook_specific_output: Option<HookSpecificOutput>,
}

/// Parse a decision blob. Empty / unparseable input yields `Allow`.
fn parse_decision_blob(text: &str) -> HookDecision {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return HookDecision::Allow;
    }
    let Ok(env) = serde_json::from_str::<DecisionEnvelope>(trimmed) else {
        return HookDecision::Allow;
    };
    let Some(out) = env.hook_specific_output else {
        return HookDecision::Allow;
    };
    match out.permission_decision.as_deref() {
        Some("deny") => HookDecision::Deny(
            out.permission_decision_reason
                .unwrap_or_else(|| "denied by hook".into()),
        ),
        Some("ask") => HookDecision::Deny(
            out.permission_decision_reason
                .unwrap_or_else(|| "ask path not yet wired".into()),
        ),
        // "allow" (default) — honor updatedInput if present.
        _ => match out.updated_input {
            Some(v) => HookDecision::UpdatedInput(v),
            None => HookDecision::Allow,
        },
    }
}

// ---------------------------------------------------------------------------
// SessionStart additionalContext parsing
// ---------------------------------------------------------------------------

/// Stdout JSON shapes that can carry `SessionStart` `additionalContext`.
#[derive(Debug, Deserialize, Default)]
struct SessionStartBlob {
    #[serde(rename = "additionalContext")]
    additional_context: Option<String>,
    #[serde(rename = "hookSpecificOutput", default)]
    hook_specific_output: Option<SessionStartNested>,
}

#[derive(Debug, Deserialize, Default)]
struct SessionStartNested {
    #[serde(rename = "additionalContext")]
    additional_context: Option<String>,
}

/// Extract `SessionStart` `additionalContext` from a handler's stdout JSON.
/// Accepts the flat (`{"additionalContext": ...}`) and nested
/// (`{"hookSpecificOutput": {"additionalContext": ...}}`) shapes. Returns
/// `None` for empty / non-JSON / absent input.
///
/// Invoked by the router handlers' `session_start` (and the config-hook bridge,
/// #121) to extract context from a handler's stdout / response body.
pub(crate) fn parse_session_start_context(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let blob = serde_json::from_str::<SessionStartBlob>(trimmed).ok()?;
    blob.additional_context
        .or_else(|| blob.hook_specific_output.and_then(|n| n.additional_context))
}

// ---------------------------------------------------------------------------
// ShellCommandHook
// ---------------------------------------------------------------------------

/// External hook handler that spawns a child process per event. Stdin gets
/// the event envelope as JSON; stdout/exit code yields the decision.
#[derive(Debug, Clone)]
pub struct ShellCommandHook {
    /// Command (absolute path or PATH-resolvable name).
    pub command: String,
    /// Extra argv.
    pub args: Vec<String>,
    /// Hard timeout. On expiry, the hook is treated as Allow + warning.
    pub timeout: Duration,
    /// Extra env passed to the child (in addition to inherited env).
    pub env: BTreeMap<String, String>,
    /// Tool-name glob filter (`"*"` matches all). Only relevant for tool events.
    pub matcher: String,
    /// Event name this hook fires on (used to filter dispatch in fan-out).
    pub event_name: String,
}

/// Raw capture from a shell hook invocation.
struct CaptureOutput {
    stdout: String,
    /// stderr, truncated to 8 KiB.
    stderr: String,
    exit_code: i32,
}

impl ShellCommandHook {
    /// Spawn the child, send the envelope, capture stdout/stderr + exit code.
    /// `None` on spawn / wait / timeout failure (callers treat as Allow / no-op).
    async fn run_capture(&self, envelope: serde_json::Value) -> Option<CaptureOutput> {
        let payload = match serde_json::to_string(&envelope) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "shell hook: failed to serialize envelope");
                return None;
            }
        };

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

        let wait_output = tokio::time::timeout(self.timeout, child.wait_with_output()).await;
        let output = match wait_output {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "shell hook: wait failed");
                return None;
            }
            Err(_) => {
                tracing::warn!(
                    command = %self.command,
                    timeout_ms = u64::try_from(self.timeout.as_millis()).unwrap_or(u64::MAX),
                    "shell hook: timeout exceeded; treating as Allow"
                );
                return None;
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = truncate_kb(&String::from_utf8_lossy(&output.stderr), 8);
        if !stderr.is_empty() {
            tracing::debug!(
                command = %self.command,
                hook_stderr = %stderr,
                "shell hook: stderr captured",
            );
        }
        Some(CaptureOutput {
            stdout,
            stderr,
            exit_code: output.status.code().unwrap_or(0),
        })
    }

    /// Spawn the child, send the envelope, return the decision.
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
            2 => HookDecision::Deny(if out.stderr.is_empty() {
                format!("hook `{}` exited 2", self.command)
            } else {
                out.stderr
            }),
            other => {
                tracing::warn!(
                    command = %self.command,
                    exit_code = other,
                    "shell hook: non-zero exit treated as Allow"
                );
                HookDecision::Allow
            }
        }
    }
}

/// Spawn `cmd`, retrying a few times on *transient* failures.
///
/// On loaded CI runners `fork`/`exec` can intermittently fail with EAGAIN
/// (temporary resource exhaustion) or ETXTBSY (a freshly-written hook script
/// not yet fully closed). Without a retry these surfaced as a misleading
/// `Allow` and an intermittently-failing test (caliban-ai/caliban#41).
/// Non-transient errors (missing binary, permission denied) return
/// immediately — retrying them only burns the timeout budget.
async fn spawn_with_retry(
    cmd: &mut tokio::process::Command,
    command: &str,
) -> std::io::Result<tokio::process::Child> {
    const MAX_ATTEMPTS: u32 = 4;
    let mut attempt = 1;
    loop {
        match cmd.spawn() {
            Ok(child) => return Ok(child),
            Err(e) if attempt < MAX_ATTEMPTS && is_transient_spawn_error(&e) => {
                // Exponential-ish backoff: 5ms, 10ms, 20ms.
                let backoff = Duration::from_millis(5 * (1 << (attempt - 1)));
                tracing::debug!(
                    command = %command,
                    error = %e,
                    attempt,
                    backoff_ms = u64::try_from(backoff.as_millis()).unwrap_or(u64::MAX),
                    "shell hook: transient spawn failure; retrying",
                );
                tokio::time::sleep(backoff).await;
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

/// True for `spawn()` errors that are transient under load and worth a brief
/// retry: EAGAIN (decodes to [`std::io::ErrorKind::WouldBlock`]) and ETXTBSY
/// ([`std::io::ErrorKind::ExecutableFileBusy`]).
fn is_transient_spawn_error(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::ExecutableFileBusy
    )
}

fn truncate_kb(s: &str, kib: usize) -> String {
    let max = kib * 1024;
    if s.len() <= max {
        s.to_string()
    } else {
        format!(
            "{}\n[truncated to {kib} KiB]",
            &s[..max.min(s.len() - (s.len() - max))]
        )
    }
}

#[async_trait]
impl Hooks for ShellCommandHook {
    async fn before_tool(&self, ctx: &ToolCtx<'_>) -> Result<HookDecision> {
        if self.event_name != "PreToolUse" {
            return Ok(HookDecision::Allow);
        }
        if !crate::permissions::matches_glob(&self.matcher, ctx.tool_name) {
            return Ok(HookDecision::Allow);
        }
        let envelope = crate::hooks::build_envelope(
            "PreToolUse",
            serde_json::json!({
                "session_id": "",
                "turn_index": ctx.turn_index,
                "tool": {
                    "name": ctx.tool_name,
                    "useId": ctx.tool_use_id,
                    "input": ctx.input,
                }
            }),
        );
        Ok(self.dispatch(envelope).await)
    }

    async fn after_tool(
        &self,
        ctx: &ToolCtx<'_>,
        _result: &std::result::Result<Vec<caliban_provider::ContentBlock>, crate::tool::ToolError>,
    ) -> Result<()> {
        if self.event_name != "PostToolUse" {
            return Ok(());
        }
        if !crate::permissions::matches_glob(&self.matcher, ctx.tool_name) {
            return Ok(());
        }
        let envelope = crate::hooks::build_envelope(
            "PostToolUse",
            serde_json::json!({
                "session_id": "",
                "turn_index": ctx.turn_index,
                "tool": {
                    "name": ctx.tool_name,
                    "useId": ctx.tool_use_id,
                    "input": ctx.input,
                }
            }),
        );
        let _ = self.dispatch(envelope).await; // Observer-only on PostToolUse.
        Ok(())
    }

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
}

// ---------------------------------------------------------------------------
// HttpHook
// ---------------------------------------------------------------------------

/// External hook handler that POSTs the event envelope to a URL.
#[derive(Debug, Clone)]
pub struct HttpHook {
    /// URL to POST.
    pub url: String,
    /// Static headers to send.
    pub headers: BTreeMap<String, String>,
    /// Hard timeout.
    pub timeout: Duration,
    /// Allowed URL globs (defense in depth — also enforced at config load).
    pub allowed_url_globs: Vec<String>,
    /// Event this hook fires on.
    pub event_name: String,
    /// Tool-name matcher.
    pub matcher: String,
    /// Shared `reqwest::Client` (lets callers reuse a connection pool).
    pub client: reqwest::Client,
}

impl HttpHook {
    fn is_url_allowed(&self) -> bool {
        self.allowed_url_globs
            .iter()
            .any(|g| crate::permissions::matches_glob(g, &self.url))
    }

    /// POST the envelope, returning the response body on a 2xx. `None` when the
    /// URL is disallowed / the request fails / non-2xx / body read fails.
    async fn fetch_body(&self, envelope: serde_json::Value) -> Option<String> {
        if !self.is_url_allowed() {
            tracing::warn!(
                url = %self.url,
                "http hook: URL not in allowed_http_hook_urls; skipping (Allow)"
            );
            return None;
        }
        let mut req = self.client.post(&self.url).json(&envelope);
        for (k, v) in &self.headers {
            req = req.header(k, v);
        }
        let resp = match tokio::time::timeout(self.timeout, req.send()).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                tracing::warn!(url = %self.url, error = %e, "http hook: request failed");
                return None;
            }
            Err(_) => {
                tracing::warn!(url = %self.url, "http hook: timeout exceeded; Allow");
                return None;
            }
        };
        if !resp.status().is_success() {
            tracing::warn!(
                url = %self.url,
                status = resp.status().as_u16(),
                "http hook: non-2xx; Allow"
            );
            return None;
        }
        match tokio::time::timeout(self.timeout, resp.text()).await {
            Ok(Ok(b)) => Some(b),
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "http hook: body read failed; Allow");
                None
            }
            Err(_) => {
                tracing::warn!("http hook: body read timeout; Allow");
                None
            }
        }
    }

    async fn dispatch(&self, envelope: serde_json::Value) -> HookDecision {
        match self.fetch_body(envelope).await {
            Some(body) => parse_decision_blob(&body),
            None => HookDecision::Allow,
        }
    }
}

#[async_trait]
impl Hooks for HttpHook {
    async fn before_tool(&self, ctx: &ToolCtx<'_>) -> Result<HookDecision> {
        if self.event_name != "PreToolUse" {
            return Ok(HookDecision::Allow);
        }
        if !crate::permissions::matches_glob(&self.matcher, ctx.tool_name) {
            return Ok(HookDecision::Allow);
        }
        let envelope = crate::hooks::build_envelope(
            "PreToolUse",
            serde_json::json!({
                "session_id": "",
                "turn_index": ctx.turn_index,
                "tool": {
                    "name": ctx.tool_name,
                    "useId": ctx.tool_use_id,
                    "input": ctx.input,
                }
            }),
        );
        Ok(self.dispatch(envelope).await)
    }

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
            .fetch_body(envelope)
            .await
            .and_then(|b| parse_session_start_context(&b))
            .into_iter()
            .collect();
        Ok(crate::hooks::SessionStartOutcome { additional_context })
    }
}

// ---------------------------------------------------------------------------
// PromptHook (v1 stub)
// ---------------------------------------------------------------------------

/// LLM-driven hook handler. v1: stub that warns and returns Allow; the real
/// wiring lands when the model router lands in a follow-up PR.
#[derive(Debug, Clone)]
pub struct PromptHook {
    /// Prompt text.
    pub prompt: String,
    /// Optional JSON schema for structured output.
    pub schema: Option<String>,
    /// Model identifier.
    pub model: Option<String>,
    /// Event this hook fires on.
    pub event_name: String,
}

#[async_trait]
impl Hooks for PromptHook {
    async fn before_tool(&self, _ctx: &ToolCtx<'_>) -> Result<HookDecision> {
        tracing::warn!(
            event = %self.event_name,
            "PromptHook is a v1 stub; returning Allow (real wiring lands with ADR 0023)"
        );
        Ok(HookDecision::Allow)
    }
}

// ---------------------------------------------------------------------------
// AgentHook (v1 stub)
// ---------------------------------------------------------------------------

/// Sub-agent hook handler. v1: stub that warns and returns Allow; the
/// supervisor for sub-agent invocation lands with ADR 0037.
#[derive(Debug, Clone)]
pub struct AgentHook {
    /// Agent name.
    pub agent: String,
    /// Event this hook fires on.
    pub event_name: String,
}

#[async_trait]
impl Hooks for AgentHook {
    async fn before_tool(&self, _ctx: &ToolCtx<'_>) -> Result<HookDecision> {
        tracing::warn!(
            agent = %self.agent,
            event = %self.event_name,
            "AgentHook is a v1 stub; returning Allow (real wiring lands with ADR 0037)"
        );
        Ok(HookDecision::Allow)
    }
}

// ---------------------------------------------------------------------------
// McpHook (v1 stub)
// ---------------------------------------------------------------------------

/// MCP-tool-as-hook handler. v1: stub that warns and returns Allow; the real
/// wiring lands with ADR 0023.
#[derive(Debug, Clone)]
pub struct McpHook {
    /// MCP server name.
    pub server: String,
    /// MCP tool name.
    pub tool: String,
    /// Event this hook fires on.
    pub event_name: String,
}

#[async_trait]
impl Hooks for McpHook {
    async fn before_tool(&self, _ctx: &ToolCtx<'_>) -> Result<HookDecision> {
        tracing::warn!(
            server = %self.server,
            tool = %self.tool,
            event = %self.event_name,
            "McpHook is a v1 stub; returning Allow (real wiring lands with ADR 0023)"
        );
        Ok(HookDecision::Allow)
    }
}

// ---------------------------------------------------------------------------
// Helper: log the file (parameter unused in this build) to ensure imports
// stay sound across optional features.
// ---------------------------------------------------------------------------

#[doc(hidden)]
pub fn __noop_pathbuf(p: PathBuf) -> PathBuf {
    p
}

// ---------------------------------------------------------------------------
// Tests for the decision parser (handlers are integration-tested separately).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_blob_is_allow() {
        assert!(matches!(parse_decision_blob(""), HookDecision::Allow));
        assert!(matches!(parse_decision_blob("   "), HookDecision::Allow));
    }

    #[test]
    fn session_start_context_flat_shape() {
        let blob = r#"{ "additionalContext": "hello from hook" }"#;
        assert_eq!(
            parse_session_start_context(blob),
            Some("hello from hook".to_string())
        );
    }

    #[test]
    fn session_start_context_nested_shape() {
        let blob = r#"{ "hookSpecificOutput": { "hookEventName": "SessionStart", "additionalContext": "nested ctx" } }"#;
        assert_eq!(
            parse_session_start_context(blob),
            Some("nested ctx".to_string())
        );
    }

    #[test]
    fn session_start_context_absent_or_nonjson() {
        assert_eq!(parse_session_start_context(""), None);
        assert_eq!(parse_session_start_context("not json"), None);
        assert_eq!(parse_session_start_context(r#"{ "other": 1 }"#), None);
    }

    #[test]
    fn non_json_blob_is_allow() {
        assert!(matches!(parse_decision_blob("nope"), HookDecision::Allow));
    }

    #[test]
    fn deny_blob_parses() {
        let blob = r#"{
            "hookSpecificOutput": {
                "permissionDecision": "deny",
                "permissionDecisionReason": "no rm allowed"
            }
        }"#;
        match parse_decision_blob(blob) {
            HookDecision::Deny(msg) => assert!(msg.contains("no rm")),
            d => panic!("unexpected: {d:?}"),
        }
    }

    #[test]
    fn updated_input_blob_parses() {
        let blob = r#"{
            "hookSpecificOutput": {
                "updatedInput": { "command": "echo safe" }
            }
        }"#;
        match parse_decision_blob(blob) {
            HookDecision::UpdatedInput(v) => {
                assert_eq!(v["command"], "echo safe");
            }
            d => panic!("unexpected: {d:?}"),
        }
    }

    #[test]
    fn allow_blob_with_no_updated_input() {
        let blob = r#"{ "hookSpecificOutput": { "permissionDecision": "allow" } }"#;
        assert!(matches!(parse_decision_blob(blob), HookDecision::Allow));
    }

    #[test]
    fn truncate_kb_short_string_untouched() {
        assert_eq!(truncate_kb("hi", 8), "hi");
    }

    #[test]
    fn transient_spawn_error_flags_eagain() {
        // EAGAIN from fork (resource temporarily unavailable) decodes to
        // ErrorKind::WouldBlock — the classic loaded-CI fork failure.
        let e = std::io::Error::from(std::io::ErrorKind::WouldBlock);
        assert!(is_transient_spawn_error(&e));
    }

    #[test]
    fn transient_spawn_error_flags_etxtbsy() {
        // ETXTBSY: the just-written hook script is still being closed.
        let e = std::io::Error::from(std::io::ErrorKind::ExecutableFileBusy);
        assert!(is_transient_spawn_error(&e));
    }

    #[test]
    fn transient_spawn_error_rejects_not_found() {
        // A genuinely missing binary must NOT be retried — it will never
        // succeed and retrying just wastes the timeout budget.
        let e = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert!(!is_transient_spawn_error(&e));
    }

    #[test]
    fn transient_spawn_error_rejects_permission_denied() {
        let e = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert!(!is_transient_spawn_error(&e));
    }
}
