//! ACP adapter — `caliban acp serve` (ADR 0055 / #530).
//!
//! Exposes caliban over the **Agent Client Protocol** (ACP) so editors and
//! editor-shaped tools — the `OpenCode` / Grok Build / Zed drive-in path — can
//! drive it interactively, turn by turn, over the shared
//! [`crate::serve::registry::DriveRegistry`]. Unlike the poll-based MCP and HTTP
//! surfaces, ACP is **push/streaming**: a `session/prompt` call blocks for the
//! duration of one agent turn while the adapter streams `session/update`
//! notifications for the events it produces, then returns a `stopReason`.
//!
//! The wire is newline-delimited **JSON-RPC 2.0** over stdio (one JSON object
//! per line, no LSP-style `Content-Length` framing). We hand-roll it on tokio +
//! `serde_json` rather than take a framework dependency, matching the HTTP
//! surface's owned serve loop (ADR 0055 consequences) and keeping the adapter a
//! thin shim that holds no agent logic.
//!
//! Methods handled (client → agent):
//! - `initialize` `{ protocolVersion, clientCapabilities }` → agent capabilities
//! - `authenticate` → accepted (the shared gate governs auth; see below)
//! - `session/new` `{ cwd, mcpServers }` → `{ sessionId }`
//! - `session/prompt` `{ sessionId, prompt: [ContentBlock] }` → streams
//!   `session/update`, returns `{ stopReason }`
//! - `session/cancel` (notification) `{ sessionId }`
//!
//! Agent → client:
//! - `session/update` (notification): [`TurnEvent`] mapped to ACP session updates
//!   (`agent_message_chunk`, `agent_thought_chunk`, `tool_call`,
//!   `tool_call_update`).
//! - `session/request_permission` (request): a driven run's `Ask` surfaced over
//!   the [`crate::serve::permissions`] bridge into the editor's own permission UI;
//!   the client's selected option is routed back as the decision.
//!
//! Auth: v1 is stdio-only, which is loopback-inherent, so the shared
//! [`crate::serve::auth::AuthGate`] classifies the peer as `Loopback` and admits
//! it (the bearer path bites on the HTTP surface, #531); the gate is wired here
//! so a future network transport enforces it uniformly.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use caliban_agent_core::TurnEvent;
use caliban_drive::{DriveInbound, DriveStatus};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

use crate::serve::auth::{AuthGate, Peer};
use crate::serve::permissions::PermissionDecision;
use crate::serve::registry::{AgentFactory, DriveRegistry, PermitOutcome, RunSpec, SendInputError};

/// ACP protocol version this adapter speaks.
const PROTOCOL_VERSION: i64 = 1;

/// How often the prompt pump polls the registry for new events / status.
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// How long a `session/prompt` waits for the client to answer a surfaced
/// `session/request_permission` before treating it as a denial. Matches the
/// permission bridge's own run-side ceiling so neither side wins a race by much.
#[allow(
    clippy::duration_suboptimal_units,
    reason = "Duration::from_mins is unstable; from_secs(600) keeps the intent legible enough"
)]
const PERMISSION_TIMEOUT: Duration = Duration::from_secs(600);

// JSON-RPC 2.0 standard error codes (a subset — the ones this adapter emits).
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// In-flight agent→client requests, keyed by the id we assigned. Each entry's
/// sender resolves the awaiting [`Conn::request_client`] with the client's
/// response (`Ok`) or error object (`Err`).
type PendingRequests = Mutex<HashMap<i64, oneshot::Sender<Result<Value, Value>>>>;

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// One inbound JSON-RPC frame. A single struct covers all three shapes we read:
/// a client **request** (`method` + `id`), a client **notification** (`method`,
/// no `id`), and a **response** to one of our agent→client requests (`id` +
/// `result`/`error`, no `method`).
#[derive(Deserialize)]
struct Incoming {
    #[serde(default)]
    id: Option<Value>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Value,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<Value>,
}

// ---------------------------------------------------------------------------
// Per-connection state
// ---------------------------------------------------------------------------

/// Server-side state for one ACP session.
struct SessionState {
    /// The backing registry run, once the first `session/prompt` has spawned it.
    run_id: Option<String>,
    /// Event cursor into the run's buffer, carried across prompt turns.
    cursor: usize,
    /// Cooperative cancel flag flipped by `session/cancel`, observed by the pump.
    cancel: Arc<AtomicBool>,
}

/// A live ACP connection: the shared drive state plus the JSON-RPC plumbing
/// (outbound writer channel, agent→client request correlation, sessions). Cloned
/// into each spawned message handler; every field is cheaply shareable.
#[derive(Clone)]
struct Conn {
    registry: DriveRegistry,
    factory: Arc<dyn AgentFactory>,
    auth: Arc<AuthGate>,
    /// Serialized outbound frames; a single writer task drains this to the wire.
    out: mpsc::UnboundedSender<String>,
    sessions: Arc<Mutex<HashMap<String, SessionState>>>,
    /// In-flight agent→client requests awaiting the client's response.
    pending: Arc<PendingRequests>,
    next_req_id: Arc<AtomicI64>,
    next_session: Arc<AtomicI64>,
}

impl Conn {
    /// Enqueue a raw JSON value as one newline-delimited outbound frame.
    fn send(&self, frame: &Value) {
        // A send error only means the writer task is gone (connection closing);
        // nothing actionable, so drop the frame.
        let _ = self.out.send(frame.to_string());
    }

    /// Reply to a client request with a success result.
    fn send_result(&self, id: &Value, result: &Value) {
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }

    /// Reply to a client request with an error.
    fn send_err(&self, id: &Value, code: i64, message: impl Into<String>) {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message.into() },
        }));
    }

    /// Emit an agent→client notification (no id, no response expected).
    fn notify(&self, method: &str, params: &Value) {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// Emit a `session/update` notification carrying one ACP session update.
    fn session_update(&self, session_id: &str, update: &Value) {
        self.notify(
            "session/update",
            &json!({ "sessionId": session_id, "update": update }),
        );
    }

    /// Send an agent→client request and await the client's response, while also
    /// observing `cancel` so a `session/cancel` mid-request doesn't block for the
    /// full timeout. Returns `Ok(result)` on a success response, or `Err(reason)`
    /// on an error response, a dropped channel, a timeout, or cancellation. In
    /// every `Err` path the in-flight entry is removed before returning.
    async fn request_client(
        &self,
        method: &str,
        params: &Value,
        cancel: &AtomicBool,
    ) -> Result<Value, String> {
        let id = self.next_req_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        lock(&self.pending).insert(id, tx);
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));

        let deadline = tokio::time::sleep(PERMISSION_TIMEOUT);
        tokio::pin!(rx, deadline);
        let outcome = loop {
            tokio::select! {
                res = &mut rx => break match res {
                    Ok(Ok(result)) => Ok(result),
                    Ok(Err(err)) => Err(format!("client returned an error: {err}")),
                    Err(_dropped) => Err("client channel closed".to_string()),
                },
                () = &mut deadline => break Err("client did not respond in time".to_string()),
                () = tokio::time::sleep(POLL_INTERVAL) => {
                    if cancel.load(Ordering::Relaxed) {
                        break Err("cancelled".to_string());
                    }
                }
            }
        };
        if outcome.is_err() {
            lock(&self.pending).remove(&id);
        }
        outcome
    }

    // -----------------------------------------------------------------------
    // Inbound dispatch
    // -----------------------------------------------------------------------

    /// Route one parsed inbound frame. Client requests/notifications dispatch by
    /// method; a frame with no method is a response to one of our agent→client
    /// requests and is routed to the awaiting [`Self::request_client`] caller.
    async fn handle(self, msg: Incoming) {
        let Some(method) = msg.method.clone() else {
            self.route_response(msg);
            return;
        };

        match method.as_str() {
            "initialize" => self.on_initialize(msg),
            "authenticate" => {
                if let Some(id) = msg.id {
                    self.send_result(&id, &Value::Null);
                }
            }
            "session/new" => self.on_session_new(msg),
            "session/prompt" => self.on_session_prompt(msg).await,
            "session/cancel" => self.on_session_cancel(&msg),
            _ => {
                // Only requests (with an id) get an error reply; unknown
                // notifications are ignored per JSON-RPC.
                if let Some(id) = msg.id {
                    self.send_err(&id, METHOD_NOT_FOUND, format!("unknown method: {method}"));
                }
            }
        }
    }

    /// Deliver a client response to the matching in-flight agent→client request.
    fn route_response(&self, msg: Incoming) {
        let Some(id) = msg.id.as_ref().and_then(Value::as_i64) else {
            return;
        };
        if let Some(tx) = lock(&self.pending).remove(&id) {
            let outcome = match msg.error {
                Some(err) => Err(err),
                None => Ok(msg.result.unwrap_or(Value::Null)),
            };
            let _ = tx.send(outcome);
        }
    }

    fn on_initialize(&self, msg: Incoming) {
        let Some(id) = msg.id else { return };
        // stdio is loopback-inherent; the gate admits local peers (the bearer
        // path bites on a future network transport / the HTTP surface, #531).
        if !self.auth.authorize(Peer::Loopback, None).is_allowed() {
            self.send_err(&id, INVALID_REQUEST, "unauthorized");
            return;
        }
        self.send_result(
            &id,
            &json!({
                "protocolVersion": PROTOCOL_VERSION,
                "agentCapabilities": {
                    "loadSession": false,
                    "promptCapabilities": {
                        "image": false,
                        "audio": false,
                        "embeddedContext": false,
                    },
                },
                "authMethods": [],
            }),
        );
    }

    fn on_session_new(&self, msg: Incoming) {
        let Some(id) = msg.id else { return };
        let n = self.next_session.fetch_add(1, Ordering::Relaxed);
        let session_id = format!("sess-{n}");
        lock(&self.sessions).insert(
            session_id.clone(),
            SessionState {
                run_id: None,
                cursor: 0,
                cancel: Arc::new(AtomicBool::new(false)),
            },
        );
        self.send_result(&id, &json!({ "sessionId": session_id }));
    }

    fn on_session_cancel(&self, msg: &Incoming) {
        if let Some(sid) = msg.params.get("sessionId").and_then(Value::as_str)
            && let Some(state) = lock(&self.sessions).get(sid)
        {
            state.cancel.store(true, Ordering::Relaxed);
        }
    }

    async fn on_session_prompt(&self, msg: Incoming) {
        let Some(id) = msg.id else { return };

        let Some(sid) = msg
            .params
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            self.send_err(&id, INVALID_PARAMS, "session/prompt requires sessionId");
            return;
        };
        let prompt = prompt_text(&msg.params);
        if prompt.trim().is_empty() {
            self.send_err(
                &id,
                INVALID_PARAMS,
                "session/prompt requires non-empty text",
            );
            return;
        }

        // Snapshot / initialise the session's run + cursor. `cancel` is reset for
        // this turn.
        let (mut run_id, mut cursor, cancel) = {
            let mut sessions = lock(&self.sessions);
            let Some(state) = sessions.get_mut(&sid) else {
                self.send_err(&id, INVALID_PARAMS, format!("unknown sessionId: {sid}"));
                return;
            };
            state.cancel.store(false, Ordering::Relaxed);
            (
                state.run_id.clone(),
                state.cursor,
                Arc::clone(&state.cancel),
            )
        };

        // First prompt on the session spawns the run; a follow-up resumes it.
        if run_id.is_none() {
            let built = match self.factory.build_run(&RunSpec {
                prompt,
                interactive: true,
            }) {
                Ok(b) => b,
                Err(e) => {
                    self.send_err(&id, INTERNAL_ERROR, format!("failed to build run: {e}"));
                    return;
                }
            };
            let new_id = self.registry.spawn(built);
            cursor = 0;
            if let Some(s) = lock(&self.sessions).get_mut(&sid) {
                s.run_id = Some(new_id.clone());
                s.cursor = 0;
            }
            run_id = Some(new_id);
        } else if let Some(rid) = &run_id {
            match self
                .registry
                .send_input(rid, DriveInbound::UserMessage { text: prompt })
            {
                Ok(()) => {}
                Err(SendInputError::UnknownRun) => {
                    self.send_err(&id, INVALID_PARAMS, format!("unknown sessionId: {sid}"));
                    return;
                }
                Err(SendInputError::Ended(e)) => {
                    self.send_err(&id, INVALID_REQUEST, format!("session has ended: {e}"));
                    return;
                }
            }
        }
        let run_id = run_id.expect("run_id set above");

        // Pump: drive the run for one turn, streaming updates, until it parks at
        // a turn boundary (awaiting_input), finishes, fails, or is cancelled.
        let stop_reason = self.pump_turn(&sid, &run_id, &mut cursor, &cancel).await;

        // Persist the advanced cursor for the next prompt turn.
        if let Some(state) = lock(&self.sessions).get_mut(&sid) {
            state.cursor = cursor;
        }

        match stop_reason {
            Ok(reason) => self.send_result(&id, &json!({ "stopReason": reason })),
            Err(message) => self.send_err(&id, INTERNAL_ERROR, message),
        }
    }

    /// Drive one turn of `run_id`, streaming `session/update`s and surfacing any
    /// permission prompt, until a terminal / boundary state. `cursor` is advanced
    /// in place so the caller can persist it. Returns the ACP `stopReason` string,
    /// or an error message if the run failed.
    async fn pump_turn(
        &self,
        session_id: &str,
        run_id: &str,
        cursor: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<&'static str, String> {
        // The lifecycle status is watch-driven and can flip to a
        // boundary/terminal state before the event drainer has pushed the last
        // events of the turn. So don't exit the moment we see such a status —
        // require SETTLE consecutive empty polls first, to flush stragglers.
        const SETTLE: u8 = 3;
        let mut settle = 0u8;

        loop {
            if cancel.load(Ordering::Relaxed) {
                return Ok("cancelled");
            }

            let Some(view) = self.registry.poll(run_id, *cursor) else {
                return Err(format!("unknown run for session {session_id}"));
            };

            for event in &view.events {
                if let Some(update) = turn_event_to_update(event) {
                    self.session_update(session_id, &update);
                }
            }
            let drained = view.events.is_empty();
            *cursor = view.next_cursor;

            if let Some(pending) = view.pending {
                self.surface_permission(session_id, run_id, &pending, cancel)
                    .await;
                // Answering unblocks the tool, which produces its ToolCallEnd and
                // more events; keep pumping from the top without settling.
                settle = 0;
                continue;
            }

            match view.status {
                DriveStatus::AwaitingInput | DriveStatus::Done => {
                    if drained {
                        settle += 1;
                        if settle >= SETTLE {
                            return Ok("end_turn");
                        }
                    } else {
                        settle = 0;
                    }
                }
                DriveStatus::Failed { error } => {
                    if drained {
                        settle += 1;
                        if settle >= SETTLE {
                            return Err(error);
                        }
                    } else {
                        settle = 0;
                    }
                }
                DriveStatus::Starting | DriveStatus::Running => settle = 0,
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Surface a pending permission prompt as an ACP `session/request_permission`
    /// and route the client's selected option back into the run.
    async fn surface_permission(
        &self,
        session_id: &str,
        run_id: &str,
        pending: &crate::serve::registry::PendingPermission,
        cancel: &AtomicBool,
    ) {
        let params = json!({
            "sessionId": session_id,
            "toolCall": {
                "toolCallId": pending.tool_use_id,
                "title": pending.tool_name,
                "rawInput": pending.input,
            },
            "options": [
                { "optionId": "allow", "name": "Allow", "kind": "allow_once" },
                { "optionId": "reject", "name": "Reject", "kind": "reject_once" },
            ],
        });

        let decision = match self
            .request_client("session/request_permission", &params, cancel)
            .await
        {
            Ok(result) => permission_decision(&result),
            // A failed / cancelled / absent client response is a denial — the run
            // must never fail open. On cancel the pump exits at its next check.
            Err(_) => PermissionDecision::Deny("permission request not answered".into()),
        };

        // Route the decision into the run. A non-`Answered` outcome (`NoPending`,
        // `Mismatch`, `RunGone`) means the run moved on before we could deliver
        // it — e.g. its own ask timeout fired; nothing more to do here.
        let _ = matches!(
            self.registry.permit(run_id, &pending.tool_use_id, decision),
            PermitOutcome::Answered
        );
    }
}

// ---------------------------------------------------------------------------
// Pure mapping helpers
// ---------------------------------------------------------------------------

/// Concatenate the text of a `session/prompt`'s content blocks. Non-text blocks
/// (images/audio/embedded context — which we don't advertise support for) are
/// ignored.
fn prompt_text(params: &Value) -> String {
    let Some(blocks) = params.get("prompt").and_then(Value::as_array) else {
        return String::new();
    };
    let mut out = String::new();
    for block in blocks {
        if block.get("type").and_then(Value::as_str) == Some("text")
            && let Some(text) = block.get("text").and_then(Value::as_str)
        {
            out.push_str(text);
        }
    }
    out
}

/// Interpret a `session/request_permission` response into a decision. The
/// expected success shape is `{ outcome: { outcome: "selected", optionId } }`;
/// anything else (including `"cancelled"`) is a denial.
fn permission_decision(result: &Value) -> PermissionDecision {
    let outcome = result.get("outcome");
    let kind = outcome
        .and_then(|o| o.get("outcome"))
        .and_then(Value::as_str);
    let selected = outcome
        .and_then(|o| o.get("optionId"))
        .and_then(Value::as_str);
    match (kind, selected) {
        (Some("selected"), Some("allow")) => PermissionDecision::Allow,
        (Some("selected"), Some("reject")) => PermissionDecision::Deny("rejected by client".into()),
        _ => PermissionDecision::Deny("permission not granted".into()),
    }
}

/// Map a [`TurnEvent`] to an ACP session update, or `None` for events with no
/// editor-facing representation (turn/run bookkeeping, streaming input deltas).
fn turn_event_to_update(event: &TurnEvent) -> Option<Value> {
    match event {
        TurnEvent::AssistantTextDelta { text, .. } => Some(json!({
            "sessionUpdate": "agent_message_chunk",
            "content": { "type": "text", "text": text },
        })),
        TurnEvent::AssistantThinkingDelta { text, .. } => Some(json!({
            "sessionUpdate": "agent_thought_chunk",
            "content": { "type": "text", "text": text },
        })),
        TurnEvent::ToolCallStart {
            tool_use_id, name, ..
        } => Some(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": tool_use_id,
            "title": name,
            "kind": "other",
            "status": "pending",
        })),
        TurnEvent::ToolCallEnd {
            tool_use_id,
            is_error,
            result_text,
            ..
        } => Some(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": tool_use_id,
            "status": if *is_error { "failed" } else { "completed" },
            "content": [{ "type": "content", "content": { "type": "text", "text": result_text } }],
        })),
        TurnEvent::TurnStart { .. }
        | TurnEvent::ToolCallInputDelta { .. }
        | TurnEvent::TurnEnd { .. }
        | TurnEvent::RunEnd { .. } => None,
    }
}

// ---------------------------------------------------------------------------
// Serve loop
// ---------------------------------------------------------------------------

/// Serve one ACP connection over a JSON-RPC read/write pair until EOF. Reads
/// newline-delimited frames, dispatching each on its own task so a long
/// `session/prompt` never blocks reading a concurrent `session/cancel` or the
/// reply to a `session/request_permission`. A dedicated writer task serializes
/// all outbound frames.
async fn serve_conn<R, W>(
    factory: Arc<dyn AgentFactory>,
    auth: AuthGate,
    reader: R,
    writer: W,
) -> anyhow::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
    let writer_task = tokio::spawn(async move {
        let mut writer = writer;
        while let Some(line) = out_rx.recv().await {
            if writer.write_all(line.as_bytes()).await.is_err()
                || writer.write_all(b"\n").await.is_err()
                || writer.flush().await.is_err()
            {
                break;
            }
        }
    });

    let conn = Conn {
        registry: DriveRegistry::new(),
        factory,
        auth: Arc::new(auth),
        out: out_tx,
        sessions: Arc::new(Mutex::new(HashMap::new())),
        pending: Arc::new(Mutex::new(HashMap::new())),
        next_req_id: Arc::new(AtomicI64::new(1)),
        next_session: Arc::new(AtomicI64::new(1)),
    };

    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Incoming>(&line) {
            Ok(msg) => {
                let conn = conn.clone();
                tokio::spawn(conn.handle(msg));
            }
            Err(e) => {
                // Malformed frame: reply with a parse error only if we can't even
                // tell it apart from a notification (no id recoverable), send a
                // null-id parse error per JSON-RPC.
                conn.send(&json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": { "code": PARSE_ERROR, "message": format!("parse error: {e}") },
                }));
            }
        }
    }

    // Reader hit EOF: drop the outbound sender so the writer task finishes.
    drop(conn);
    let _ = writer_task.await;
    Ok(())
}

/// Serve caliban as an ACP agent over stdio (`caliban acp serve`).
///
/// # Errors
///
/// Propagates settings/provider construction and transport errors.
pub(crate) async fn run_serve(args: &crate::args::Args) -> anyhow::Result<i32> {
    let settings = crate::startup::load_layered_settings(args, &std::env::current_dir()?)
        .map_err(|e| anyhow::anyhow!("failed to load settings: {e}"))?
        .settings;
    let helper_pool = Arc::new(caliban_settings::ApiKeyHelperPool::from_raw(
        settings.api_key_helper.as_ref(),
    ));
    let provider = crate::startup::build_provider(args, &helper_pool)?;
    let factory = Arc::new(crate::serve::registry::build_prod_factory(
        args, settings, provider,
    )?);

    serve_conn(
        factory,
        AuthGate::from_env(),
        tokio::io::stdin(),
        tokio::io::stdout(),
    )
    .await?;
    Ok(0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use caliban_agent_core::{
        Agent, ContentBlock, NoopHooks, PermissionsHook, TextBlock, Tool, ToolContext, ToolError,
        ToolRegistry, TurnEvent, default_rules,
    };
    use caliban_provider::{
        MockProvider, Provider, StopReason, StreamEvent, StreamingContentType, StreamingDelta,
        Usage,
    };
    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};

    use super::{
        AuthGate, INVALID_PARAMS, METHOD_NOT_FOUND, permission_decision, prompt_text, serve_conn,
        turn_event_to_update,
    };
    use crate::serve::permissions::{DriveAskHandler, PermissionDecision};
    use crate::serve::registry::{AgentFactory, BuiltRun, RunSpec};

    // -- pure-helper unit tests --------------------------------------------

    #[test]
    fn prompt_text_concatenates_text_blocks_only() {
        let params = json!({
            "prompt": [
                { "type": "text", "text": "hello " },
                { "type": "image", "data": "…" },
                { "type": "text", "text": "world" },
            ]
        });
        assert_eq!(prompt_text(&params), "hello world");
        assert_eq!(prompt_text(&json!({})), "");
        assert_eq!(prompt_text(&json!({ "prompt": [] })), "");
    }

    #[test]
    fn permission_decision_maps_selected_options() {
        assert!(matches!(
            permission_decision(
                &json!({ "outcome": { "outcome": "selected", "optionId": "allow" } })
            ),
            PermissionDecision::Allow
        ));
        assert!(matches!(
            permission_decision(
                &json!({ "outcome": { "outcome": "selected", "optionId": "reject" } })
            ),
            PermissionDecision::Deny(_)
        ));
        // Cancelled / unknown / malformed → deny (never fail open).
        assert!(matches!(
            permission_decision(&json!({ "outcome": { "outcome": "cancelled" } })),
            PermissionDecision::Deny(_)
        ));
        assert!(matches!(
            permission_decision(&json!({})),
            PermissionDecision::Deny(_)
        ));
    }

    #[test]
    fn turn_event_mapping_covers_and_skips() {
        let text = TurnEvent::AssistantTextDelta {
            turn_index: 0,
            content_block_index: 0,
            text: "hi".into(),
        };
        let u = turn_event_to_update(&text).unwrap();
        assert_eq!(u["sessionUpdate"], "agent_message_chunk");
        assert_eq!(u["content"]["text"], "hi");

        let tool = TurnEvent::ToolCallStart {
            turn_index: 0,
            tool_use_id: "tu1".into(),
            name: "gate".into(),
        };
        let u = turn_event_to_update(&tool).unwrap();
        assert_eq!(u["sessionUpdate"], "tool_call");
        assert_eq!(u["toolCallId"], "tu1");
        assert_eq!(u["title"], "gate");

        let end = TurnEvent::ToolCallEnd {
            turn_index: 0,
            tool_use_id: "tu1".into(),
            is_error: true,
            content: vec![],
            result_text: "boom".into(),
            truncated: false,
        };
        let u = turn_event_to_update(&end).unwrap();
        assert_eq!(u["sessionUpdate"], "tool_call_update");
        assert_eq!(u["status"], "failed");

        // Bookkeeping events produce no editor-facing update.
        assert!(
            turn_event_to_update(&TurnEvent::TurnStart {
                turn_index: 0,
                message_id: "m".into(),
                model: "mock".into(),
            })
            .is_none()
        );
    }

    // -- test scaffolding ---------------------------------------------------

    fn text_turn(text: &str) -> Vec<caliban_provider::error::Result<StreamEvent>> {
        vec![
            Ok(StreamEvent::MessageStart {
                id: "m".into(),
                model: "mock-model".into(),
            }),
            Ok(StreamEvent::ContentBlockStart {
                index: 0,
                content_type: StreamingContentType::Text,
            }),
            Ok(StreamEvent::Delta {
                index: 0,
                delta: StreamingDelta::Text(text.to_string()),
            }),
            Ok(StreamEvent::ContentBlockStop { index: 0 }),
            Ok(StreamEvent::MessageDelta {
                stop_reason: Some(StopReason::EndTurn),
                usage_delta: Some(Usage::default()),
            }),
            Ok(StreamEvent::MessageStop),
        ]
    }

    fn tool_use_turn(
        tool_use_id: &str,
        name: &str,
    ) -> Vec<caliban_provider::error::Result<StreamEvent>> {
        vec![
            Ok(StreamEvent::MessageStart {
                id: "m".into(),
                model: "mock-model".into(),
            }),
            Ok(StreamEvent::ContentBlockStart {
                index: 0,
                content_type: StreamingContentType::ToolUse {
                    id: tool_use_id.into(),
                    name: name.into(),
                },
            }),
            Ok(StreamEvent::Delta {
                index: 0,
                delta: StreamingDelta::ToolUseInputJson("{}".into()),
            }),
            Ok(StreamEvent::ContentBlockStop { index: 0 }),
            Ok(StreamEvent::MessageDelta {
                stop_reason: Some(StopReason::ToolUse),
                usage_delta: Some(Usage::default()),
            }),
            Ok(StreamEvent::MessageStop),
        ]
    }

    /// A trivial gated tool: `default_rules()`' catch-all makes every tool `Ask`,
    /// so invoking it forces the run to block on the permission bridge.
    struct GateTool {
        schema: Value,
    }
    #[async_trait]
    impl Tool for GateTool {
        fn name(&self) -> &'static str {
            "gate"
        }
        fn description(&self) -> &'static str {
            "a gated no-op tool"
        }
        fn input_schema(&self) -> &Value {
            &self.schema
        }
        async fn invoke(
            &self,
            _input: Value,
            _cx: ToolContext,
        ) -> Result<Vec<ContentBlock>, ToolError> {
            Ok(vec![ContentBlock::Text(TextBlock {
                text: "ok".into(),
                cache_control: None,
            })])
        }
    }

    /// Streams plain text turns (no tools) — for the pure run/stream/input path.
    struct TextFactory {
        turns: usize,
    }
    impl AgentFactory for TextFactory {
        fn build_run(&self, spec: &RunSpec) -> anyhow::Result<BuiltRun> {
            let mp = Arc::new(MockProvider::new());
            for _ in 0..self.turns {
                mp.enqueue_stream(text_turn("hi"));
            }
            let agent = Agent::builder()
                .provider(mp as Arc<dyn Provider + Send + Sync>)
                .tools(ToolRegistry::default())
                .model("mock-model")
                .max_tokens(64)
                .build()
                .expect("agent builds");
            let (_ask, perm_rx) = DriveAskHandler::pair();
            Ok(BuiltRun {
                agent: Arc::new(agent),
                messages: vec![caliban_provider::Message::user_text(spec.prompt.clone())],
                perm_rx,
                interactive: spec.interactive,
            })
        }
    }

    /// Emits a tool-use turn (which trips the `Ask` gate via a real
    /// `PermissionsHook` + `DriveAskHandler`) then a text turn — so the run
    /// genuinely blocks on the permission bridge until answered.
    struct GatedFactory;
    impl AgentFactory for GatedFactory {
        fn build_run(&self, spec: &RunSpec) -> anyhow::Result<BuiltRun> {
            let mp = Arc::new(MockProvider::new());
            mp.enqueue_stream(tool_use_turn("tu_gate", "gate"));
            mp.enqueue_stream(text_turn("done"));

            let mut reg = ToolRegistry::new();
            reg.register(Arc::new(GateTool {
                schema: json!({ "type": "object", "properties": {} }),
            }));

            let (ask, perm_rx) = DriveAskHandler::pair();
            let permissions =
                PermissionsHook::new(default_rules(), Arc::new(ask), Arc::new(NoopHooks));

            let agent = Agent::builder()
                .provider(mp as Arc<dyn Provider + Send + Sync>)
                .tools(reg)
                .model("mock-model")
                .max_tokens(64)
                .hooks(Arc::new(permissions))
                .build()
                .expect("agent builds");
            Ok(BuiltRun {
                agent: Arc::new(agent),
                messages: vec![caliban_provider::Message::user_text(spec.prompt.clone())],
                perm_rx,
                interactive: spec.interactive,
            })
        }
    }

    type ClientLines = Lines<BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>>;

    async fn write_frame<W: AsyncWriteExt + Unpin>(w: &mut W, v: Value) {
        let mut s = v.to_string();
        s.push('\n');
        w.write_all(s.as_bytes()).await.unwrap();
        w.flush().await.unwrap();
    }

    async fn read_frame(lines: &mut ClientLines) -> Value {
        let line = lines
            .next_line()
            .await
            .unwrap()
            .expect("a frame before EOF");
        serde_json::from_str(&line).expect("valid JSON frame")
    }

    /// Serve `factory` over an in-process duplex; return the client's writer and
    /// line-reader halves.
    fn spawn_server(
        factory: Arc<dyn AgentFactory>,
    ) -> (tokio::io::WriteHalf<tokio::io::DuplexStream>, ClientLines) {
        let (server_end, client_end) = tokio::io::duplex(64 * 1024);
        let (sr, sw) = tokio::io::split(server_end);
        tokio::spawn(async move {
            let _ = serve_conn(factory, AuthGate::new(None), sr, sw).await;
        });
        let (cr, cw) = tokio::io::split(client_end);
        (cw, BufReader::new(cr).lines())
    }

    async fn new_session(
        cw: &mut tokio::io::WriteHalf<tokio::io::DuplexStream>,
        lines: &mut ClientLines,
        id: i64,
    ) -> String {
        write_frame(
            cw,
            json!({ "jsonrpc": "2.0", "id": id, "method": "session/new",
                    "params": { "cwd": "/tmp", "mcpServers": [] } }),
        )
        .await;
        read_frame(lines).await["result"]["sessionId"]
            .as_str()
            .unwrap()
            .to_string()
    }

    // -- protocol tests -----------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn initialize_new_prompt_streams_updates_and_returns_stop_reason() {
        let (mut cw, mut lines) = spawn_server(Arc::new(TextFactory { turns: 3 }));

        // initialize
        write_frame(
            &mut cw,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
                    "params": { "protocolVersion": 1, "clientCapabilities": {} } }),
        )
        .await;
        let init = read_frame(&mut lines).await;
        assert_eq!(init["id"], 1);
        assert_eq!(init["result"]["protocolVersion"], 1);
        assert!(init["result"]["agentCapabilities"].is_object());

        let session_id = new_session(&mut cw, &mut lines, 2).await;

        // session/prompt — streams session/update notifications, then a result.
        write_frame(
            &mut cw,
            json!({ "jsonrpc": "2.0", "id": 3, "method": "session/prompt",
                    "params": { "sessionId": session_id,
                                "prompt": [{ "type": "text", "text": "hello" }] } }),
        )
        .await;

        let mut saw_message_chunk = false;
        let stop_reason;
        loop {
            let f = read_frame(&mut lines).await;
            if f.get("method").and_then(Value::as_str) == Some("session/update") {
                assert_eq!(f["params"]["sessionId"], Value::String(session_id.clone()));
                if f["params"]["update"]["sessionUpdate"] == "agent_message_chunk" {
                    saw_message_chunk = true;
                }
            } else if f["id"] == 3 {
                stop_reason = f["result"]["stopReason"].as_str().unwrap().to_string();
                break;
            }
        }
        assert!(saw_message_chunk, "never streamed an agent_message_chunk");
        assert_eq!(stop_reason, "end_turn");

        // A follow-up prompt resumes the same run (send_input path).
        write_frame(
            &mut cw,
            json!({ "jsonrpc": "2.0", "id": 4, "method": "session/prompt",
                    "params": { "sessionId": session_id,
                                "prompt": [{ "type": "text", "text": "again" }] } }),
        )
        .await;
        loop {
            let f = read_frame(&mut lines).await;
            if f["id"] == 4 {
                assert_eq!(f["result"]["stopReason"], "end_turn");
                break;
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unknown_method_and_bad_params_error() {
        let (mut cw, mut lines) = spawn_server(Arc::new(TextFactory { turns: 1 }));

        write_frame(
            &mut cw,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "does/not/exist", "params": {} }),
        )
        .await;
        let r = read_frame(&mut lines).await;
        assert_eq!(r["id"], 1);
        assert_eq!(r["error"]["code"], METHOD_NOT_FOUND);

        write_frame(
            &mut cw,
            json!({ "jsonrpc": "2.0", "id": 2, "method": "session/prompt",
                    "params": { "sessionId": "ghost",
                                "prompt": [{ "type": "text", "text": "hi" }] } }),
        )
        .await;
        let r = read_frame(&mut lines).await;
        assert_eq!(r["id"], 2);
        assert_eq!(r["error"]["code"], INVALID_PARAMS);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn permission_request_surfaces_and_client_decision_routes_back() {
        // A real gated tool blocks the run on the permission bridge; the adapter
        // must surface a `session/request_permission` and route the client's
        // selected option back so the turn completes.
        let (mut cw, mut lines) = spawn_server(Arc::new(GatedFactory));
        let session_id = new_session(&mut cw, &mut lines, 1).await;

        write_frame(
            &mut cw,
            json!({ "jsonrpc": "2.0", "id": 2, "method": "session/prompt",
                    "params": { "sessionId": session_id,
                                "prompt": [{ "type": "text", "text": "go" }] } }),
        )
        .await;

        let mut saw_permission = false;
        loop {
            let f = read_frame(&mut lines).await;
            if f.get("method").and_then(Value::as_str) == Some("session/request_permission") {
                saw_permission = true;
                assert_eq!(f["params"]["toolCall"]["toolCallId"], "tu_gate");
                assert_eq!(f["params"]["toolCall"]["title"], "gate");
                assert!(f["params"]["options"].is_array());
                write_frame(
                    &mut cw,
                    json!({ "jsonrpc": "2.0", "id": f["id"],
                            "result": { "outcome": { "outcome": "selected", "optionId": "allow" } } }),
                )
                .await;
            } else if f["id"] == 2 {
                assert!(f.get("result").is_some(), "prompt errored: {f}");
                assert_eq!(f["result"]["stopReason"], "end_turn");
                break;
            }
        }
        assert!(saw_permission, "never surfaced a permission request");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_ends_a_blocked_turn_with_cancelled_stop_reason() {
        // The run blocks on a permission the client never answers; session/cancel
        // must end the prompt turn with stopReason "cancelled".
        let (mut cw, mut lines) = spawn_server(Arc::new(GatedFactory));
        let session_id = new_session(&mut cw, &mut lines, 1).await;

        write_frame(
            &mut cw,
            json!({ "jsonrpc": "2.0", "id": 2, "method": "session/prompt",
                    "params": { "sessionId": session_id,
                                "prompt": [{ "type": "text", "text": "go" }] } }),
        )
        .await;

        // Wait for the permission request (turn is now blocked), then cancel.
        loop {
            let f = read_frame(&mut lines).await;
            if f.get("method").and_then(Value::as_str) == Some("session/request_permission") {
                break;
            }
        }
        write_frame(
            &mut cw,
            json!({ "jsonrpc": "2.0", "method": "session/cancel",
                    "params": { "sessionId": session_id } }),
        )
        .await;

        loop {
            let f = read_frame(&mut lines).await;
            if f["id"] == 2 {
                assert_eq!(f["result"]["stopReason"], "cancelled");
                break;
            }
        }
    }
}
