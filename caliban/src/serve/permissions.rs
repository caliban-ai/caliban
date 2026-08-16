//! Permission-elicitation bridge for the drive surfaces (ADR 0055 / ADR 0045).
//!
//! A driven run still hits tool calls whose matched rule is `Ask` (Permissions
//! v2). With no TUI present, the prompt must be surfaced to the driving client
//! over the surface's own channel and its decision routed back into the run —
//! otherwise the run would deadlock, or (worse) fail open.
//!
//! This is the transport-agnostic half of that bridge, mirroring the TUI's
//! [`crate::tui::ask::TuiAskHandler`]: a [`DriveAskHandler`] (an
//! [`AskHandler`](caliban_agent_core::AskHandler)) emits a
//! [`DrivePermissionRequest`] over an mpsc channel and awaits the client's
//! decision on a oneshot. Each adapter drains the channel, renders the request
//! in its own protocol (MCP elicitation, ACP permission request, HTTP
//! awaiting-input), and answers it. The Permissions v2 engine still makes and
//! enforces the actual decision — this bridge only carries the prompt and the
//! answer.

use std::time::Duration;

use async_trait::async_trait;
use caliban_agent_core::{AskHandler, HookDecision, ToolCtx};
use tokio::sync::{mpsc, oneshot};

/// Hard upper bound on how long a driven run waits for the client to answer a
/// permission prompt before falling back to a denial. Matches the TUI's Ask
/// timeout (the longest Bash tool deadline).
#[allow(
    clippy::duration_suboptimal_units,
    reason = "Duration::from_mins is unstable; from_secs(600) keeps the intent legible enough"
)]
const ASK_TIMEOUT: Duration = Duration::from_secs(600);

/// The client's decision on a [`DrivePermissionRequest`].
#[derive(Debug, Clone)]
pub(crate) enum PermissionDecision {
    /// Allow this tool invocation.
    Allow,
    /// Deny this tool invocation, with a reason surfaced to the run.
    Deny(String),
}

/// A pending permission prompt from a driven run, awaiting the client's answer.
///
/// The adapter obtains these from the receiver returned by
/// [`DriveAskHandler::pair`], renders the request to the client, and calls
/// [`DrivePermissionRequest::answer`] (or the [`allow`](Self::allow) /
/// [`deny`](Self::deny) shorthands) with the outcome.
#[derive(Debug)]
pub(crate) struct DrivePermissionRequest {
    tool_use_id: String,
    tool_name: String,
    input: serde_json::Value,
    respond: oneshot::Sender<PermissionDecision>,
}

impl DrivePermissionRequest {
    /// The model-assigned `tool_use_id` for the invocation being gated.
    pub(crate) fn tool_use_id(&self) -> &str {
        &self.tool_use_id
    }

    /// The name of the tool the run is trying to invoke.
    pub(crate) fn tool_name(&self) -> &str {
        &self.tool_name
    }

    /// The tool's input JSON.
    pub(crate) fn input(&self) -> &serde_json::Value {
        &self.input
    }

    /// Answer the request, resuming the run.
    ///
    /// # Errors
    ///
    /// Returns the `decision` back if the run has already moved on (its receiver
    /// was dropped, e.g. because it was cancelled), so the caller can observe
    /// that its answer had no effect.
    pub(crate) fn answer(self, decision: PermissionDecision) -> Result<(), PermissionDecision> {
        self.respond.send(decision)
    }

    /// Shorthand for `answer(PermissionDecision::Allow)`.
    pub(crate) fn allow(self) -> Result<(), PermissionDecision> {
        self.answer(PermissionDecision::Allow)
    }

    /// Shorthand for `answer(PermissionDecision::Deny(reason))`.
    pub(crate) fn deny(self, reason: impl Into<String>) -> Result<(), PermissionDecision> {
        self.answer(PermissionDecision::Deny(reason.into()))
    }
}

/// An [`AskHandler`] that bridges `Ask` rules to a driving client over an
/// unbounded mpsc channel. Clone it wherever an `AskHandler` is needed; the
/// receiver is held by the adapter's event loop.
#[derive(Debug, Clone)]
pub(crate) struct DriveAskHandler {
    tx: mpsc::UnboundedSender<DrivePermissionRequest>,
    timeout: Duration,
}

impl DriveAskHandler {
    /// Build the handler + receiver pair with the default [`ASK_TIMEOUT`].
    pub(crate) fn pair() -> (Self, mpsc::UnboundedReceiver<DrivePermissionRequest>) {
        Self::pair_with_timeout(ASK_TIMEOUT)
    }

    /// Build the handler + receiver pair with an explicit answer timeout.
    pub(crate) fn pair_with_timeout(
        timeout: Duration,
    ) -> (Self, mpsc::UnboundedReceiver<DrivePermissionRequest>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx, timeout }, rx)
    }
}

#[async_trait]
impl AskHandler for DriveAskHandler {
    async fn prompt(&self, ctx: &ToolCtx<'_>) -> HookDecision {
        let (respond_tx, respond_rx) = oneshot::channel();
        let req = DrivePermissionRequest {
            tool_use_id: ctx.tool_use_id.to_string(),
            tool_name: ctx.tool_name.to_string(),
            input: ctx.input.clone(),
            respond: respond_tx,
        };
        if self.tx.send(req).is_err() {
            // No client is draining the channel: this is the non-interactive
            // fallback (like a missing TTY), so use AskDenied — a mode such as
            // acceptEdits/dontAsk may still flip it back to Allow.
            return HookDecision::AskDenied(format!(
                "permission denied for tool '{}': no drive client attached",
                ctx.tool_name
            ));
        }
        match tokio::time::timeout(self.timeout, respond_rx).await {
            Ok(Ok(PermissionDecision::Allow)) => HookDecision::Allow,
            // An explicit client decision is a hard Deny (not flippable).
            Ok(Ok(PermissionDecision::Deny(reason))) => HookDecision::Deny(reason),
            Ok(Err(_dropped)) => HookDecision::Deny(format!(
                "permission denied for tool '{}': drive client dropped the request",
                ctx.tool_name
            )),
            Err(_elapsed) => HookDecision::Deny(format!(
                "permission denied for tool '{}': drive client did not respond within {}s",
                ctx.tool_name,
                self.timeout.as_secs()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use caliban_agent_core::{AskHandler, HookDecision, ToolCtx};
    use serde_json::json;

    use super::DriveAskHandler;

    fn ctx<'a>(input: &'a serde_json::Value, tool: &'a str) -> ToolCtx<'a> {
        ToolCtx {
            session_id: "sess",
            turn_index: 0,
            tool_use_id: "tu_1",
            tool_name: tool,
            input,
            is_read_only: false,
        }
    }

    #[tokio::test]
    async fn allow_decision_routes_to_allow() {
        let (handler, mut rx) = DriveAskHandler::pair();
        let input = json!({ "command": "ls" });
        let responder = async {
            let req = rx.recv().await.expect("request surfaced");
            assert_eq!(req.tool_name(), "Bash");
            assert_eq!(req.tool_use_id(), "tu_1");
            assert_eq!(req.input(), &json!({ "command": "ls" }));
            req.allow().expect("run still listening");
        };
        let tcx = ctx(&input, "Bash");
        let (decision, ()) = tokio::join!(handler.prompt(&tcx), responder);
        assert!(matches!(decision, HookDecision::Allow), "{decision:?}");
    }

    #[tokio::test]
    async fn deny_decision_routes_to_deny_with_reason() {
        let (handler, mut rx) = DriveAskHandler::pair();
        let input = json!({});
        let responder = async {
            let req = rx.recv().await.expect("request surfaced");
            req.deny("client said no").expect("run still listening");
        };
        let tcx = ctx(&input, "Write");
        let (decision, ()) = tokio::join!(handler.prompt(&tcx), responder);
        match decision {
            HookDecision::Deny(reason) => assert_eq!(reason, "client said no"),
            other => panic!("expected Deny, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn no_client_attached_falls_back_to_ask_denied() {
        let (handler, rx) = DriveAskHandler::pair();
        drop(rx); // no adapter draining the channel
        let input = json!({});
        let decision = handler.prompt(&ctx(&input, "Bash")).await;
        assert!(
            matches!(decision, HookDecision::AskDenied(_)),
            "{decision:?}"
        );
    }

    #[tokio::test]
    async fn dropped_request_without_answer_denies() {
        let (handler, mut rx) = DriveAskHandler::pair();
        let input = json!({});
        let responder = async {
            let _req = rx.recv().await.expect("request surfaced");
            // Drop it without answering — client took the prompt then vanished.
        };
        let tcx = ctx(&input, "Bash");
        let (decision, ()) = tokio::join!(handler.prompt(&tcx), responder);
        assert!(matches!(decision, HookDecision::Deny(_)), "{decision:?}");
    }

    #[tokio::test]
    async fn timeout_denies() {
        // Keep the receiver alive so the send succeeds, but never answer.
        let (handler, _rx) = DriveAskHandler::pair_with_timeout(Duration::from_millis(10));
        let input = json!({});
        let decision = handler.prompt(&ctx(&input, "Bash")).await;
        match decision {
            HookDecision::Deny(reason) => assert!(reason.contains("did not respond"), "{reason}"),
            other => panic!("expected timeout Deny, got {other:?}"),
        }
    }
}
