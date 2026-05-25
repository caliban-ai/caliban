//! `TuiAskHandler` — bridges `caliban-agent-core`'s `AskHandler` trait into a
//! ratatui modal driven by the TUI event loop.
//!
//! ## Design
//!
//! The agent loop runs in the same tokio runtime as the TUI's event loop;
//! `AskHandler::prompt` is `async` and may park indefinitely. We bridge via
//! an mpsc → oneshot pair:
//!
//! 1. Agent: matched-Ask rule triggers `AskHandler::prompt(...)`.
//! 2. `TuiAskHandler`: builds an `AskRequest { ..., respond: oneshot::Sender }`
//!    and sends it down the mpsc.
//! 3. TUI event loop: drains the mpsc, opens the modal with the request.
//! 4. User: picks an option in the modal; the resolver consumes the
//!    `respond` sender and resolves the oneshot.
//! 5. `TuiAskHandler`: awaits the oneshot and returns a `HookDecision`.
//!
//! Safety: a 10-minute hard timeout on the oneshot resolves to `Deny`. Drop
//! of the sender without a response also resolves to `Deny`.

use std::time::Duration;

use async_trait::async_trait;

/// Hard upper bound on how long we wait for the user to dismiss an Ask
/// modal — matches the longest tool deadline (Bash). After this, the
/// pending request resolves to `Deny`.
#[allow(
    clippy::duration_suboptimal_units,
    reason = "Duration::from_mins is unstable; from_secs(600) keeps the intent legible enough"
)]
const ASK_TIMEOUT: Duration = Duration::from_secs(600);
use caliban_agent_core::{AskHandler, HookDecision, ToolCtx};
use tokio::sync::{mpsc, oneshot};

/// One pending permission prompt waiting on user input.
#[derive(Debug)]
pub(crate) struct AskRequest {
    /// Tool the model is trying to invoke.
    pub(crate) tool_name: String,
    /// Pretty summary of the tool input for display in the modal.
    pub(crate) input_summary: String,
    /// Oneshot to resolve when the user picks an answer.
    pub(crate) respond: oneshot::Sender<AskResponse>,
}

/// User's choice in the Ask modal.
#[derive(Debug, Clone, Copy)]
pub(crate) enum AskResponse {
    /// Allow this invocation only.
    AllowOnce,
    /// Deny this invocation.
    Deny,
}

/// `AskHandler` impl that bridges Ask rules to a ratatui modal via an
/// unbounded mpsc channel. The TUI event loop drains the channel and pumps
/// requests into the modal state.
#[derive(Debug)]
pub(crate) struct TuiAskHandler {
    /// Sender owned by the handler; cloned wherever an `AskHandler` is
    /// needed. The receiver is held by the TUI event loop.
    tx: mpsc::UnboundedSender<AskRequest>,
}

impl TuiAskHandler {
    /// Build the handler + receiver pair. The receiver should be plumbed into
    /// the TUI event loop's `select!` so requests are surfaced as soon as
    /// the agent triggers an Ask.
    pub(crate) fn pair() -> (Self, mpsc::UnboundedReceiver<AskRequest>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }
}

/// Format a tool input value for compact display in the modal header.
fn input_summary(input: &serde_json::Value) -> String {
    use serde_json::Value;
    match input {
        Value::Object(map) => {
            // Prefer the "command" key for Bash; else "path"; else first key.
            for k in ["command", "path", "url", "pattern"] {
                if let Some(v) = map.get(k).and_then(Value::as_str) {
                    let s: String = v.chars().take(160).collect();
                    return format!("{k}={s}");
                }
            }
            let mut parts: Vec<String> = Vec::new();
            for (k, v) in map {
                let s = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                let trimmed: String = s.chars().take(40).collect();
                parts.push(format!("{k}={trimmed}"));
            }
            parts.join(", ")
        }
        Value::String(s) => s.chars().take(160).collect(),
        other => other.to_string(),
    }
}

#[async_trait]
impl AskHandler for TuiAskHandler {
    async fn prompt(&self, ctx: &ToolCtx<'_>) -> HookDecision {
        let (respond_tx, respond_rx) = oneshot::channel();
        let req = AskRequest {
            tool_name: ctx.tool_name.to_string(),
            input_summary: input_summary(ctx.input),
            respond: respond_tx,
        };
        if self.tx.send(req).is_err() {
            // TUI gone — fall back to Deny, matching CLI behavior.
            return HookDecision::Deny(format!(
                "permission denied for tool '{}': Ask modal unavailable",
                ctx.tool_name
            ));
        }
        // 10-minute hard timeout (matches the longest Bash deadline).
        let result = tokio::time::timeout(ASK_TIMEOUT, respond_rx).await;
        match result {
            Ok(Ok(AskResponse::AllowOnce)) => HookDecision::Allow,
            Ok(Ok(AskResponse::Deny) | Err(_)) => {
                HookDecision::Deny(format!("permission denied for tool '{}'", ctx.tool_name))
            }
            Err(_elapsed) => HookDecision::Deny(format!(
                "permission denied for tool '{}': ask modal timed out",
                ctx.tool_name
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caliban_agent_core::AskHandler;

    fn ctx<'a>(name: &'a str, input: &'a serde_json::Value) -> ToolCtx<'a> {
        ToolCtx {
            turn_index: 0,
            tool_use_id: "t1",
            tool_name: name,
            input,
        }
    }

    #[tokio::test]
    async fn allow_once_resolves_to_allow() {
        let (handler, mut rx) = TuiAskHandler::pair();
        let input = serde_json::json!({"command": "ls"});
        // Spawn the modal responder: take the request, AllowOnce.
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.respond.send(AskResponse::AllowOnce);
            }
        });
        let dec = handler.prompt(&ctx("Bash", &input)).await;
        assert!(matches!(dec, HookDecision::Allow));
    }

    #[tokio::test]
    async fn deny_resolves_to_deny() {
        let (handler, mut rx) = TuiAskHandler::pair();
        let input = serde_json::json!({"command": "rm -rf"});
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.respond.send(AskResponse::Deny);
            }
        });
        let dec = handler.prompt(&ctx("Bash", &input)).await;
        assert!(matches!(dec, HookDecision::Deny(_)));
    }

    #[tokio::test]
    async fn dropped_sender_resolves_to_deny() {
        let (handler, mut rx) = TuiAskHandler::pair();
        let input = serde_json::json!({"command": "x"});
        tokio::spawn(async move {
            // Drop without responding — simulates Esc.
            if let Some(req) = rx.recv().await {
                drop(req.respond);
            }
        });
        let dec = handler.prompt(&ctx("Bash", &input)).await;
        assert!(matches!(dec, HookDecision::Deny(_)));
    }

    #[test]
    fn summary_prefers_command_for_bash() {
        let v = serde_json::json!({"command": "git status", "cwd": "/tmp"});
        assert!(input_summary(&v).starts_with("command=git status"));
    }

    #[test]
    fn summary_handles_long_strings() {
        let long = "a".repeat(500);
        let v = serde_json::json!({"command": long});
        assert!(input_summary(&v).len() < 250);
    }
}
