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

/// Parse an *explicit* decision from a hook's stdout/response JSON.
///
/// Returns `Some(_)` only when the blob carries an actionable decision — an
/// explicit `permissionDecision` or an `updatedInput`. Returns `None` for
/// empty / non-JSON input, or a JSON object that has no `hookSpecificOutput`
/// (e.g. informational `{"foo":1}`) or an empty one. `None` lets the caller
/// fall back to exit-code semantics rather than silently treating the blob as
/// Allow (#171: a `{…}` + `exit 2` deny was being swallowed).
fn explicit_decision(text: &str) -> Option<HookDecision> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let env = serde_json::from_str::<DecisionEnvelope>(trimmed).ok()?;
    let out = env.hook_specific_output?;
    match out.permission_decision.as_deref() {
        Some("deny") => Some(HookDecision::Deny(
            out.permission_decision_reason
                .unwrap_or_else(|| "denied by hook".into()),
        )),
        Some("ask") => Some(HookDecision::Deny(
            out.permission_decision_reason
                .unwrap_or_else(|| "ask path not yet wired".into()),
        )),
        // An explicit "allow" (or any other explicit value) is authoritative —
        // honor updatedInput if present, else plain Allow.
        Some(_) => Some(
            out.updated_input
                .map_or(HookDecision::Allow, HookDecision::UpdatedInput),
        ),
        // No explicit decision: only `updatedInput` alone is actionable.
        None => out.updated_input.map(HookDecision::UpdatedInput),
    }
}

/// Parse a decision blob with no exit-code fallback (used by HTTP hooks, which
/// have no exit code). Empty / unparseable / no-decision input yields `Allow`.
fn parse_decision_blob(text: &str) -> HookDecision {
    explicit_decision(text).unwrap_or(HookDecision::Allow)
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
// Config → runtime bridge
// ---------------------------------------------------------------------------

/// Events that the executable `Command`/`Http` hook handlers actually fire on.
/// Anything else is a config gap that would silently never run (#185 H4).
fn event_supported(event_name: &str) -> bool {
    matches!(event_name, "PreToolUse" | "PostToolUse" | "SessionStart")
}

/// Build executing hook trait objects from a parsed [`HooksConfig`], for
/// composition into the agent `Hooks` chain. Returns an empty vec when hooks are
/// globally disabled.
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
    http_client: &reqwest::Client,
) -> Vec<std::sync::Arc<dyn crate::hooks::Hooks + Send + Sync>> {
    use crate::hooks_config::HookHandlerType;

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

    let mut out: Vec<std::sync::Arc<dyn crate::hooks::Hooks + Send + Sync>> = Vec::new();
    for (event_name, handlers) in &cfg.events {
        for h in handlers {
            // Command/Http hooks only fire on the events they implement
            // (PreToolUse / PostToolUse / SessionStart). A handler bound to any
            // other event (UserPromptSubmit, PreCompact, …) would build, join
            // the chain, and never run — warn and skip instead (#185 H4).
            if matches!(h.kind, HookHandlerType::Command | HookHandlerType::Http)
                && !event_supported(event_name)
            {
                tracing::warn!(
                    event = %event_name,
                    kind = ?h.kind,
                    "hook is bound to an event this handler kind does not fire on \
                     (only PreToolUse/PostToolUse/SessionStart are supported); skipping"
                );
                continue;
            }
            match h.kind {
                HookHandlerType::Command => {
                    let Some(command) = h.command.clone() else {
                        tracing::warn!(event = %event_name, "command hook missing `command`; skipping");
                        continue;
                    };
                    out.push(std::sync::Arc::new(ShellCommandHook {
                        command,
                        args: h.args.clone(),
                        timeout: h.timeout,
                        env: h.env.clone(),
                        matcher: h.matcher.clone(),
                        if_pattern: h.if_pattern.clone(),
                        asynchronous: h.asynchronous,
                        event_name: event_name.clone(),
                    }));
                }
                HookHandlerType::Http => {
                    let Some(url) = h.url.clone() else {
                        tracing::warn!(event = %event_name, "http hook missing `url`; skipping");
                        continue;
                    };
                    out.push(std::sync::Arc::new(HttpHook {
                        url,
                        headers: h.headers.clone(),
                        timeout: h.timeout,
                        allowed_url_globs: cfg.allowed_http_hook_urls.clone(),
                        event_name: event_name.clone(),
                        matcher: h.matcher.clone(),
                        if_pattern: h.if_pattern.clone(),
                        asynchronous: h.asynchronous,
                        allowed_env_vars: cfg.http_hook_allowed_env_vars.clone(),
                        allow_local_targets: cfg.allow_local_http_hook_targets,
                        client: http_client.clone(),
                    }));
                }
                HookHandlerType::Mcp | HookHandlerType::Prompt | HookHandlerType::Agent => {
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
    /// Optional full-pattern (`Tool:arg-glob`) firing guard from `if = "…"`.
    /// `None` means "no extra guard"; `Some(p)` fires only when `p` matches the
    /// tool + first-arg (#171).
    pub if_pattern: Option<String>,
    /// When true, this handler is fire-and-forget: its decision is ignored and
    /// it never blocks the tool (ADR-0024). See #171.
    pub asynchronous: bool,
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

    /// Whether this hook should fire for `ctx`: the tool-name `matcher` must
    /// match AND, when an `if` pattern is configured, the full `Tool:arg-glob`
    /// pattern must match too (#171).
    fn fires_for(&self, ctx: &ToolCtx<'_>) -> bool {
        crate::permissions::matches_glob(&self.matcher, ctx.tool_name)
            && match &self.if_pattern {
                None => true,
                Some(p) => crate::permissions_matcher::matches(p, ctx),
            }
    }

    /// Spawn the child, send the envelope, return the decision.
    async fn dispatch(&self, envelope: serde_json::Value) -> HookDecision {
        let Some(out) = self.run_capture(envelope).await else {
            return HookDecision::Allow;
        };

        // An *explicit* JSON decision (permissionDecision / updatedInput) wins.
        // Informational JSON without a decision must NOT mask the exit code —
        // otherwise a `{…}` + `exit 2` deny is swallowed (#171).
        if let Some(decision) = explicit_decision(&out.stdout) {
            return decision;
        }

        // No explicit decision — fall back to exit-code semantics.
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
        // Back off to the nearest char boundary at or below `max`; slicing at a
        // byte that splits a multibyte char panics (#185 H7).
        let mut end = max;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}\n[truncated to {kib} KiB]", &s[..end])
    }
}

#[async_trait]
impl Hooks for ShellCommandHook {
    async fn before_tool(&self, ctx: &ToolCtx<'_>) -> Result<HookDecision> {
        if self.event_name != "PreToolUse" || !self.fires_for(ctx) {
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
        // async = true: fire-and-forget, decision ignored, never blocks (#171).
        if self.asynchronous {
            let hook = self.clone();
            tokio::spawn(async move {
                let _ = hook.dispatch(envelope).await;
            });
            return Ok(HookDecision::Allow);
        }
        Ok(self.dispatch(envelope).await)
    }

    async fn after_tool(
        &self,
        ctx: &ToolCtx<'_>,
        _result: &std::result::Result<Vec<caliban_provider::ContentBlock>, crate::tool::ToolError>,
    ) -> Result<()> {
        if self.event_name != "PostToolUse" || !self.fires_for(ctx) {
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
        // PostToolUse is observer-only; async just detaches the await.
        if self.asynchronous {
            let hook = self.clone();
            tokio::spawn(async move {
                let _ = hook.dispatch(envelope).await;
            });
            return Ok(());
        }
        let _ = self.dispatch(envelope).await;
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
    /// Optional full-pattern (`Tool:arg-glob`) firing guard from `if = "…"` (#171).
    pub if_pattern: Option<String>,
    /// When true, fire-and-forget: decision ignored, never blocks the tool (#171).
    pub asynchronous: bool,
    /// Env vars allowed for `${VAR}` expansion in the URL / headers, from
    /// `http_hook_allowed_env_vars` (#185 H5).
    pub allowed_env_vars: Vec<String>,
    /// Opt-in (`allow_local_http_hook_targets`) permitting loopback/private
    /// targets. Link-local / cloud-metadata is blocked regardless (#217).
    pub allow_local_targets: bool,
    /// Shared `reqwest::Client` (lets callers reuse a connection pool).
    pub client: reqwest::Client,
}

/// Expand `${VAR}` references in `s`, substituting only env vars present in
/// `allowed` (the `http_hook_allowed_env_vars` allowlist). A non-allowlisted or
/// unset var expands to empty with a warning; a `${` with no closing `}` is
/// left literal. ADR-0024 documents this allowlist-gated expansion (#185 H5).
fn expand_env_vars(s: &str, allowed: &[String]) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            // Unterminated `${` — emit verbatim and stop scanning.
            out.push_str("${");
            rest = after;
            continue;
        };
        let var = &after[..end];
        if allowed.iter().any(|a| a == var) {
            match std::env::var(var) {
                Ok(val) => out.push_str(&val),
                Err(_) => {
                    tracing::warn!(var = %var, "http hook: ${{VAR}} is unset; expanding to empty");
                }
            }
        } else {
            tracing::warn!(
                var = %var,
                "http hook: ${{VAR}} not in http_hook_allowed_env_vars; expanding to empty"
            );
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Match one allow-list `pattern` against a parsed target URL by **component**
/// (scheme / host / path), not a substring glob over the whole URL string.
///
/// The old whole-URL glob let a leading-`*` pattern like `*.example.com/*`
/// match `http://attacker.com/?x=.example.com/evil` because `*` crossed `/`,
/// `:` and `@` (#217 part 2). Matching the host as its own component means a
/// glob can never reach across into the path or query.
///
/// Pattern grammar: `[scheme://]host[:port][/path-glob]`. A missing scheme
/// matches any scheme; a missing path matches any path. Globs (`*`,`?`) are
/// honored within each component.
fn url_glob_matches(pattern: &str, target: &reqwest::Url) -> bool {
    use crate::permissions::matches_glob;

    // Optional scheme.
    let (scheme_pat, rest) = match pattern.split_once("://") {
        Some((s, r)) => (Some(s), r),
        None => (None, pattern),
    };
    if let Some(sp) = scheme_pat
        && !matches_glob(sp, target.scheme())
    {
        return false;
    }

    // rest = authority[/path]
    let (authority, path_pat) = match rest.split_once('/') {
        Some((a, p)) => (a, Some(format!("/{p}"))),
        None => (rest, None),
    };

    // authority = host[:port]; strip an optional `:port` (host-only globs are
    // the common case — domains never contain ':'). IPv6 literal patterns are
    // not supported here.
    let host_pat = authority.rsplit_once(':').map_or(authority, |(h, _)| h);
    let Some(target_host) = target.host_str() else {
        return false;
    };
    if !matches_glob(host_pat, target_host) {
        return false;
    }

    // Path: a missing or `/`-only pattern matches any path.
    match path_pat {
        None => true,
        Some(p) if p == "/" => true,
        Some(p) => matches_glob(&p, target.path()),
    }
}

/// SSRF guard: reject a target whose scheme isn't http/https, or whose host is
/// an internal address (#217 part 3).
///
/// Link-local / cloud-metadata (`169.254.0.0/16`, `fe80::/10`) is **always**
/// rejected. Loopback / private / CGNAT / unspecified targets are rejected
/// unless `allow_local` is set (the `allow_local_http_hook_targets` opt-in for
/// users who genuinely run a hook server on localhost). Returns `Some(reason)`
/// when the target must be blocked.
fn ssrf_blocked_reason(target: &reqwest::Url, allow_local: bool) -> Option<&'static str> {
    if !matches!(target.scheme(), "http" | "https") {
        return Some("scheme is not http/https");
    }
    let host = target.host_str()?;
    // `host_str` brackets IPv6 literals (`[::1]`); strip them before parsing.
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let lower = bare.to_ascii_lowercase();
    if let Ok(ip) = bare.parse::<std::net::IpAddr>() {
        if ip_is_link_local(ip) {
            return Some("link-local / cloud-metadata address");
        }
        if !allow_local && ip_is_internal(ip) {
            return Some("loopback / private address");
        }
    } else if !allow_local && (lower == "localhost" || lower.ends_with(".localhost")) {
        return Some("loopback host (localhost)");
    }
    None
}

/// `true` for IPv4 `169.254.0.0/16` (incl. the `169.254.169.254` cloud-metadata
/// address) and IPv6 `fe80::/10` link-local.
fn ip_is_link_local(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => v4.is_link_local(),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return v4.is_link_local();
            }
            (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

/// `true` if `ip` is a non-public address an HTTP hook should not reach by
/// default: loopback, private, link-local, CGNAT, unspecified, broadcast,
/// documentation, or multicast.
fn ip_is_internal(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    if ip_is_link_local(ip) {
        return true;
    }
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || o[0] == 0
                // CGNAT 100.64.0.0/10
                || (o[0] == 100 && (o[1] & 0xc0) == 64)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return ip_is_internal(IpAddr::V4(v4));
            }
            let s0 = v6.segments()[0];
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // ULA fc00::/7
                || (s0 & 0xfe00) == 0xfc00
        }
    }
}

impl HttpHook {
    fn is_url_allowed(&self, target: &reqwest::Url) -> bool {
        self.allowed_url_globs
            .iter()
            .any(|g| url_glob_matches(g, target))
    }

    /// POST the envelope, returning the response body on a 2xx. `None` when the
    /// URL is disallowed / the request fails / non-2xx / body read fails.
    async fn fetch_body(&self, envelope: serde_json::Value) -> Option<String> {
        // Expand `${VAR}` (allowlist-gated) before the allow-check so the URL
        // actually contacted is the one validated against the globs (#185 H5).
        let url = expand_env_vars(&self.url, &self.allowed_env_vars);
        let parsed = match reqwest::Url::parse(&url) {
            Ok(u) => u,
            Err(e) => {
                tracing::warn!(url = %url, error = %e, "http hook: unparseable URL; skipping (Allow)");
                return None;
            }
        };
        if !self.is_url_allowed(&parsed) {
            tracing::warn!(
                url = %url,
                "http hook: URL not in allowed_http_hook_urls; skipping (Allow)"
            );
            return None;
        }
        // SSRF guard (#217): even an allow-listed URL must not target a
        // loopback/link-local/private address or a non-http(s) scheme.
        if let Some(reason) = ssrf_blocked_reason(&parsed, self.allow_local_targets) {
            tracing::warn!(
                url = %url,
                reason,
                "http hook: SSRF-blocked target; skipping (Allow)"
            );
            return None;
        }
        let mut req = self.client.post(parsed).json(&envelope);
        for (k, v) in &self.headers {
            req = req.header(k, expand_env_vars(v, &self.allowed_env_vars));
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

    /// Whether this hook should fire for `ctx`: tool-name `matcher` plus the
    /// optional `if` full-pattern guard (#171).
    fn fires_for(&self, ctx: &ToolCtx<'_>) -> bool {
        crate::permissions::matches_glob(&self.matcher, ctx.tool_name)
            && match &self.if_pattern {
                None => true,
                Some(p) => crate::permissions_matcher::matches(p, ctx),
            }
    }
}

#[async_trait]
impl Hooks for HttpHook {
    async fn before_tool(&self, ctx: &ToolCtx<'_>) -> Result<HookDecision> {
        if self.event_name != "PreToolUse" || !self.fires_for(ctx) {
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
        // async = true: fire-and-forget, decision ignored, never blocks (#171).
        if self.asynchronous {
            let hook = self.clone();
            tokio::spawn(async move {
                let _ = hook.dispatch(envelope).await;
            });
            return Ok(HookDecision::Allow);
        }
        Ok(self.dispatch(envelope).await)
    }

    async fn after_tool(
        &self,
        ctx: &ToolCtx<'_>,
        _result: &std::result::Result<Vec<caliban_provider::ContentBlock>, crate::tool::ToolError>,
    ) -> Result<()> {
        // PostToolUse is observer-only — a `[[hooks.PostToolUse]]` http handler
        // previously built but never fired (#185 H4).
        if self.event_name != "PostToolUse" || !self.fires_for(ctx) {
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
        if self.asynchronous {
            let hook = self.clone();
            tokio::spawn(async move {
                let _ = hook.dispatch(envelope).await;
            });
            return Ok(());
        }
        let _ = self.dispatch(envelope).await;
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

    fn test_client() -> reqwest::Client {
        reqwest::Client::builder().build().unwrap()
    }

    fn u(s: &str) -> reqwest::Url {
        reqwest::Url::parse(s).unwrap()
    }

    #[test]
    fn url_glob_matches_host_component_not_substring() {
        // #217 part 2: a host glob matches the URL's host component, never a
        // substring of the whole URL.
        let allow = "*.example.com/*";
        assert!(
            url_glob_matches(allow, &u("https://api.example.com/hook")),
            "legit subdomain must match"
        );
        assert!(
            !url_glob_matches(allow, &u("http://attacker.com/?x=.example.com/evil")),
            "attacker.com host must not match *.example.com (#217)"
        );
        assert!(
            !url_glob_matches(allow, &u("http://example.com.attacker.net/x")),
            "suffix-style spoof must not match"
        );
    }

    #[test]
    fn url_glob_matches_scheme_and_path_components() {
        let allow = "https://hooks.example.com/cb";
        assert!(url_glob_matches(allow, &u("https://hooks.example.com/cb")));
        assert!(
            !url_glob_matches(allow, &u("http://hooks.example.com/cb")),
            "scheme mismatch must fail"
        );
        assert!(
            !url_glob_matches(allow, &u("https://evil.com/cb")),
            "host mismatch must fail"
        );
        // Path glob.
        assert!(url_glob_matches(
            "https://hooks.example.com/*",
            &u("https://hooks.example.com/anything/here")
        ));
    }

    #[test]
    fn ssrf_blocks_loopback_link_local_and_bad_scheme() {
        // #217 part 3.
        for blocked in [
            "http://127.0.0.1/x",
            "http://169.254.169.254/latest/meta-data/",
            "http://localhost/x",
            "http://sub.localhost/x",
            "http://[::1]/x",
            "http://10.0.0.5/x",
            "http://192.168.1.1/x",
            "http://172.16.5.4/x",
            "http://100.64.1.2/x",
            "http://0.0.0.0/x",
            "file:///etc/passwd",
            "gopher://127.0.0.1/x",
        ] {
            assert!(
                ssrf_blocked_reason(&u(blocked), false).is_some(),
                "{blocked} must be SSRF-blocked"
            );
        }
        // A normal public https host passes the synchronous guard.
        assert!(ssrf_blocked_reason(&u("https://api.example.com/x"), false).is_none());
        assert!(ssrf_blocked_reason(&u("http://93.184.216.34/x"), false).is_none());

        // With the opt-in, loopback/private are permitted, but link-local /
        // cloud-metadata and a bad scheme stay blocked.
        assert!(ssrf_blocked_reason(&u("http://127.0.0.1/x"), true).is_none());
        assert!(ssrf_blocked_reason(&u("http://localhost/x"), true).is_none());
        assert!(ssrf_blocked_reason(&u("http://10.0.0.5/x"), true).is_none());
        assert!(
            ssrf_blocked_reason(&u("http://169.254.169.254/latest/"), true).is_some(),
            "link-local / metadata must stay blocked even with the local opt-in"
        );
        assert!(ssrf_blocked_reason(&u("file:///etc/passwd"), true).is_some());
    }

    #[test]
    fn bridge_builds_command_and_skips_stub_kinds() {
        let toml = r#"
[[hooks.PreToolUse]]
matcher = "Bash"
[[hooks.PreToolUse.handlers]]
type = "command"
command = "/bin/true"
[[hooks.SessionStart]]
[[hooks.SessionStart.handlers]]
type = "mcp"
mcp = "srv"
tool = "t"
"#;
        let cfg =
            crate::hooks_config::HooksConfig::from_str(toml, std::path::Path::new("test")).unwrap();
        let hooks = build_config_hooks(&cfg, &test_client());
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
        let cfg =
            crate::hooks_config::HooksConfig::from_str(toml, std::path::Path::new("test")).unwrap();
        assert!(build_config_hooks(&cfg, &test_client()).is_empty());
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
        let cfg =
            crate::hooks_config::HooksConfig::from_str(toml, std::path::Path::new("test")).unwrap();
        assert!(build_config_hooks(&cfg, &test_client()).is_empty());
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
    fn truncate_kb_handles_multibyte_at_boundary() {
        // #185 H7: a 4-byte char straddling the 8 KiB cut must not panic.
        let mut s = "a".repeat(8 * 1024 - 1);
        s.push('😀'); // bytes 8191..8195; byte 8192 splits the char
        let out = truncate_kb(&s, 8);
        assert!(out.contains("[truncated to 8 KiB]"));
        // The emoji was dropped at the boundary; only ASCII 'a's survive.
        assert!(!out.contains('😀'));
    }

    #[test]
    fn event_supported_only_for_implemented_events() {
        // #185 H4.
        assert!(event_supported("PreToolUse"));
        assert!(event_supported("PostToolUse"));
        assert!(event_supported("SessionStart"));
        assert!(!event_supported("UserPromptSubmit"));
        assert!(!event_supported("PreCompact"));
        assert!(!event_supported("Stop"));
    }

    #[test]
    fn expand_env_vars_only_substitutes_allowlisted() {
        // #185 H5: only allow-listed vars expand; others (and unset) → empty.
        // A non-allowlisted var is never looked up — the security gate.
        assert_eq!(expand_env_vars("x=${HOME}", &[]), "x=");
        // Allowlisted but unset → empty.
        assert_eq!(
            expand_env_vars(
                "x=${DEFINITELY_UNSET_VAR_H5}",
                &["DEFINITELY_UNSET_VAR_H5".to_string()]
            ),
            "x="
        );
        // Unterminated `${` is left literal.
        assert_eq!(expand_env_vars("a${b", &["b".to_string()]), "a${b");
        // A real, allowlisted env var expands to its value (use whatever the
        // test process already has — avoids the forbidden `set_var`).
        if let Some((name, value)) = std::env::vars().next() {
            let tmpl = format!("v=${{{name}}}");
            assert_eq!(expand_env_vars(&tmpl, &[name]), format!("v={value}"));
        }
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
