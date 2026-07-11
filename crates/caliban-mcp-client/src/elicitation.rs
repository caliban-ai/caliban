//! Server-initiated user prompting — `ElicitationBridge`.
//!
//! When an MCP server requests user input mid-tool-call via the
//! `elicitation/create` request, caliban routes that request through this
//! bridge: a bounded mpsc queue feeds the TUI, which renders a modal and
//! delivers the user's choice back via a [`tokio::sync::oneshot`]. Non-
//! interactive callers (`--print`, CI) attach a default handler that auto-
//! `Decline`s. A 5-minute hard cap prevents server misbehaviour from
//! blocking a turn indefinitely; on expiry the bridge returns
//! `Decline` and emits a warning.
//!
//! See `docs/superpowers/specs/2026-05-24-mcp-v2-design.md` (Elicitation
//! section) and ADR 0023.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

/// Default hard cap on time-to-respond. Servers that fail to make progress
/// after issuing an elicitation are auto-`Decline`d after this elapses.
pub const DEFAULT_ELICITATION_TIMEOUT: Duration = Duration::from_mins(5);

/// Bound on in-flight elicitation prompts (#432). A bounded channel applies
/// backpressure: a server that issues prompts faster than the TUI drains them
/// awaits at the cap instead of growing an unbounded queue (slow memory growth).
const ELICITATION_QUEUE_CAP: usize = 64;

/// Build the permission rule pattern for elicitation from a given server.
/// `Elicit(<server>)` desugars to the rule grammar's `Elicit:<server>`
/// (the `:` form supported by `caliban-agent-core::permissions`'s
/// `split_pattern` helper).
#[must_use]
pub fn elicit_rule_pattern(server: &str) -> String {
    format!("Elicit:{server}")
}

/// One prompt issued by a server. The TUI renders `message`; if a
/// `schema` is provided, the modal shows it as a hint (full schema-
/// driven form rendering is deferred to v2.1).
#[derive(Debug, Clone, Serialize)]
pub struct ElicitationRequest {
    /// Originating server (matches the `mcp.toml` table key).
    pub server: String,
    /// Free-form prompt presented to the user.
    pub message: String,
    /// Optional JSON-schema describing the expected response shape.
    pub schema: Option<serde_json::Value>,
}

/// The user's choice in response to an `ElicitationRequest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum ElicitationResponse {
    /// User accepted; carries the JSON content the server requested.
    Accept {
        /// JSON payload — should validate against the request's `schema`
        /// when one was given. We do not enforce here; the server may.
        #[serde(default)]
        content: serde_json::Value,
    },
    /// User declined to provide the requested information.
    Decline,
    /// User cancelled the entire tool call.
    Cancel,
}

impl ElicitationResponse {
    /// Shorthand for `Accept { content }`.
    #[must_use]
    pub fn accept(content: serde_json::Value) -> Self {
        Self::Accept { content }
    }
}

/// Errors emitted by the elicitation pathway.
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum ElicitationError {
    /// The bridge consumer (TUI / handler) was dropped before responding.
    #[error("elicitation: bridge consumer dropped before responding")]
    ConsumerGone,
    /// 5-minute hard cap elapsed without a response.
    #[error("elicitation: timed out after {0:?} — auto-declined")]
    Timeout(Duration),
}

/// One inflight request paired with its response channel.
pub(crate) type ElicitationItem = (ElicitationRequest, oneshot::Sender<ElicitationResponse>);

/// Send half of the bridge — installed in `Conn` so server callbacks can
/// route prompts. Cheap to clone.
#[derive(Clone)]
pub struct ElicitationBridge {
    tx: mpsc::Sender<ElicitationItem>,
    timeout: Duration,
}

impl std::fmt::Debug for ElicitationBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ElicitationBridge")
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl ElicitationBridge {
    /// Build a new bridge. Returns the send half and the receiver the TUI
    /// should drain.
    #[must_use]
    pub fn new() -> (Self, ElicitationReceiver) {
        let (tx, rx) = mpsc::channel(ELICITATION_QUEUE_CAP);
        (
            Self {
                tx,
                timeout: DEFAULT_ELICITATION_TIMEOUT,
            },
            ElicitationReceiver { rx },
        )
    }

    /// Build a bridge with a custom hard-cap timeout (tests use this to
    /// drive the timeout path quickly).
    #[must_use]
    pub fn with_timeout(timeout: Duration) -> (Self, ElicitationReceiver) {
        let (tx, rx) = mpsc::channel(ELICITATION_QUEUE_CAP);
        (Self { tx, timeout }, ElicitationReceiver { rx })
    }

    /// Build a no-op bridge that auto-`Decline`s every request immediately
    /// — used by non-interactive callers (`--print`, CI, headless).
    #[must_use]
    pub fn auto_decline() -> Self {
        let (tx, mut rx) = mpsc::channel::<ElicitationItem>(ELICITATION_QUEUE_CAP);
        // Drain in a detached task; reply Decline to every inflight prompt.
        tokio::spawn(async move {
            while let Some((req, sender)) = rx.recv().await {
                tracing::warn!(
                    target: caliban_common::tracing_targets::TARGET_MCP_ELICITATION,
                    server = %req.server,
                    "auto-declining server elicitation (non-interactive caller)",
                );
                let _ = sender.send(ElicitationResponse::Decline);
            }
        });
        Self {
            tx,
            timeout: DEFAULT_ELICITATION_TIMEOUT,
        }
    }

    /// Send a prompt and wait for the response, bounded by `self.timeout`.
    /// Returns `Decline` (+ logs a warning) on timeout — never `Err(Timeout)`
    /// in the public API; the variant exists only for diagnostics callers
    /// who use [`Self::request_strict`].
    ///
    /// # Errors
    /// [`ElicitationError::ConsumerGone`] when no TUI / handler is attached.
    pub async fn request(
        &self,
        req: ElicitationRequest,
    ) -> Result<ElicitationResponse, ElicitationError> {
        match self.request_strict(req).await {
            Ok(r) => Ok(r),
            Err(ElicitationError::Timeout(d)) => {
                tracing::warn!(
                    target: caliban_common::tracing_targets::TARGET_MCP_ELICITATION,
                    timeout = ?d,
                    "elicitation timed out — auto-declined",
                );
                Ok(ElicitationResponse::Decline)
            }
            Err(e) => Err(e),
        }
    }

    /// Like [`Self::request`] but surfaces `Timeout` as a distinct error.
    ///
    /// # Errors
    /// - [`ElicitationError::ConsumerGone`] if the receive half was dropped.
    /// - [`ElicitationError::Timeout`] if the timeout elapses.
    pub async fn request_strict(
        &self,
        req: ElicitationRequest,
    ) -> Result<ElicitationResponse, ElicitationError> {
        let (tx, rx) = oneshot::channel();
        if self.tx.send((req, tx)).await.is_err() {
            return Err(ElicitationError::ConsumerGone);
        }
        match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(_)) => Err(ElicitationError::ConsumerGone),
            Err(_) => Err(ElicitationError::Timeout(self.timeout)),
        }
    }

    /// Configured timeout (informational; used by `/mcp` diagnostics).
    #[must_use]
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

/// Receive half held by the TUI. The TUI loop polls `recv()` and for each
/// `(request, sender)` pair, displays a modal and sends the user's choice
/// back on the oneshot.
#[derive(Debug)]
pub struct ElicitationReceiver {
    rx: mpsc::Receiver<ElicitationItem>,
}

impl ElicitationReceiver {
    /// Await the next request. Returns `None` when all senders have been
    /// dropped (no more bridges remain).
    pub async fn recv(&mut self) -> Option<ElicitationItem> {
        self.rx.recv().await
    }

    /// Non-blocking variant — returns the next request if one is ready,
    /// otherwise `None`. Used by the TUI event loop to interleave with
    /// other futures without holding the receiver borrow across awaits.
    pub fn try_recv(&mut self) -> Option<ElicitationItem> {
        self.rx.try_recv().ok()
    }
}

/// Shared, cheaply-cloneable wrapper around an [`ElicitationBridge`]. The
/// MCP `Conn` and registered tools hold one of these; cloning is just an
/// `Arc::clone`.
pub type SharedElicitationBridge = Arc<ElicitationBridge>;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    #[tokio::test]
    async fn accept_round_trip() {
        let (bridge, mut rx) = ElicitationBridge::new();
        // Spawn a "TUI" task that accepts with a payload.
        tokio::spawn(async move {
            let (req, sender) = rx.recv().await.expect("recv request");
            assert_eq!(req.server, "linear");
            assert_eq!(req.message, "Pick a workspace");
            sender
                .send(ElicitationResponse::accept(json!({"workspace": "demo"})))
                .expect("send response");
        });

        let resp = bridge
            .request(ElicitationRequest {
                server: "linear".to_string(),
                message: "Pick a workspace".to_string(),
                schema: None,
            })
            .await
            .expect("request");
        match resp {
            ElicitationResponse::Accept { content } => {
                assert_eq!(content["workspace"], "demo");
            }
            other => panic!("expected Accept, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn decline_round_trip() {
        let (bridge, mut rx) = ElicitationBridge::new();
        tokio::spawn(async move {
            let (_req, sender) = rx.recv().await.expect("recv");
            sender.send(ElicitationResponse::Decline).expect("send");
        });
        let resp = bridge
            .request(ElicitationRequest {
                server: "s".to_string(),
                message: "?".to_string(),
                schema: None,
            })
            .await
            .expect("request");
        assert_eq!(resp, ElicitationResponse::Decline);
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_yields_decline_and_warns() {
        // 50ms cap to make the test fast under start_paused.
        let (bridge, _rx_keep_alive) = ElicitationBridge::with_timeout(Duration::from_millis(50));
        // Don't reply — the receiver is held but never reads.
        let fut = bridge.request(ElicitationRequest {
            server: "stuck".to_string(),
            message: "...".to_string(),
            schema: None,
        });
        // Advance virtual time past the timeout.
        let resp = tokio::time::timeout(Duration::from_secs(10), fut)
            .await
            .expect("must complete via internal timeout")
            .expect("request");
        assert_eq!(resp, ElicitationResponse::Decline);
    }

    #[tokio::test]
    async fn auto_decline_handles_requests() {
        let bridge = ElicitationBridge::auto_decline();
        let resp = bridge
            .request(ElicitationRequest {
                server: "s".to_string(),
                message: "ask".to_string(),
                schema: None,
            })
            .await
            .expect("request");
        assert_eq!(resp, ElicitationResponse::Decline);
    }

    #[tokio::test]
    async fn consumer_dropped_surfaces_error() {
        let (bridge, rx) = ElicitationBridge::new();
        drop(rx);
        let err = bridge
            .request_strict(ElicitationRequest {
                server: "s".to_string(),
                message: "x".to_string(),
                schema: None,
            })
            .await
            .unwrap_err();
        assert!(
            matches!(err, ElicitationError::ConsumerGone),
            "got: {err:?}"
        );
    }

    #[test]
    fn elicit_rule_pattern_format() {
        assert_eq!(elicit_rule_pattern("linear"), "Elicit:linear");
        assert_eq!(elicit_rule_pattern("*"), "Elicit:*");
    }

    #[tokio::test(start_paused = true)]
    async fn request_strict_surfaces_timeout() {
        let (bridge, _rx) = ElicitationBridge::with_timeout(Duration::from_millis(20));
        let err = bridge
            .request_strict(ElicitationRequest {
                server: "s".to_string(),
                message: "x".to_string(),
                schema: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ElicitationError::Timeout(_)), "got: {err:?}");
    }
}
