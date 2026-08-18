//! MCP-server adapter — `caliban mcp serve` (ADR 0055 / #526).
//!
//! Exposes caliban as an MCP *server* other agents can drive, over the shared
//! [`caliban_drive`] core. v1 is **stdio-only**, with **poll-based unary tools**
//! (MCP tool calls are unary; the drive core is a stream, so a run is driven by
//! `caliban_run` → `caliban_poll(cursor)` → … rather than a live push) and a
//! **versioned envelope** around each `TurnEvent` on the wire so the event type
//! can evolve without breaking clients.
//!
//! Tools:
//! - `caliban_run { prompt, interactive? }` → `{ run_id }`
//! - `caliban_poll { run_id, cursor }` → `{ events: [{v,event}…], next_cursor, status, permission_request? }`
//! - `caliban_status { run_id }` → `{ status }`
//! - `caliban_send_input { run_id, text?, end? }` → resume an awaiting-input run
//! - `caliban_permit { run_id, tool_use_id, allow, reason? }` → answer a permission prompt
//!
//! Auth: stdio is loopback-inherent, so the shared [`crate::serve::auth::AuthGate`]
//! classifies the peer as `Loopback` and admits it; the bearer path bites on the
//! HTTP surface (#531). Permission prompts ride the [`crate::serve::permissions`]
//! bridge: a driven run's `Ask` is surfaced in the `caliban_poll` response and
//! answered by `caliban_permit`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use caliban_agent_core::{Agent, TurnEvent};
use caliban_drive::{DriveInbound, DriveOptions, DriveSession};
use caliban_provider::Message;
use futures::StreamExt as _;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content, ServerCapabilities, ServerInfo};
use rmcp::{ErrorData, ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::serve::auth::{AuthGate, Peer};
use crate::serve::permissions::{DriveAskHandler, DrivePermissionRequest};

/// Wire schema version for the event envelope. Bump when `TurnEvent`'s
/// serialized shape changes in a way clients must notice.
const ENVELOPE_VERSION: u8 = 1;

// ---------------------------------------------------------------------------
// Agent-factory seam — lets the server be tested with a MockProvider agent
// and driven with a real one in production.
// ---------------------------------------------------------------------------

/// What a client asked to run.
pub(crate) struct RunSpec {
    /// The initial user prompt.
    pub(crate) prompt: String,
    /// Whether the run pauses awaiting further input at each turn boundary.
    pub(crate) interactive: bool,
}

/// A freshly-built run: the agent, its initial messages, and the receiver end
/// of its permission-elicitation channel.
pub(crate) struct BuiltRun {
    /// The agent to drive.
    pub(crate) agent: Arc<Agent>,
    /// Initial messages (the user prompt).
    pub(crate) messages: Vec<Message>,
    /// Receiver for permission prompts raised by this run's `AskHandler`.
    pub(crate) perm_rx: mpsc::UnboundedReceiver<DrivePermissionRequest>,
    /// Whether the run is interactive.
    pub(crate) interactive: bool,
}

/// Builds a runnable agent per driven run. The production impl wires the real
/// provider + tools; tests supply a `MockProvider`-backed one.
pub(crate) trait AgentFactory: Send + Sync {
    /// Build a run from `spec`.
    ///
    /// # Errors
    ///
    /// Propagates provider / agent construction failures.
    fn build_run(&self, spec: &RunSpec) -> anyhow::Result<BuiltRun>;
}

// ---------------------------------------------------------------------------
// Run registry
// ---------------------------------------------------------------------------

/// Server-side state for one active run.
struct RunEntry {
    session: DriveSession,
    /// Events accumulated by the per-run drainer task, read by `caliban_poll`.
    events: Arc<Mutex<Vec<TurnEvent>>>,
    /// Permission prompts raised by the run, drained on poll/permit.
    perm_rx: mpsc::UnboundedReceiver<DrivePermissionRequest>,
    /// The currently-surfaced permission prompt awaiting a client decision.
    pending: Option<DrivePermissionRequest>,
    /// Kept alive so the run's parent cancellation scope outlives the run.
    _cancel: CancellationToken,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------------------
// Tool argument structs (input schemas)
// ---------------------------------------------------------------------------

#[derive(Deserialize, JsonSchema)]
struct RunArgs {
    /// The initial user prompt.
    prompt: String,
    /// Pause awaiting further input at each turn boundary (default false).
    #[serde(default)]
    interactive: bool,
}

#[derive(Deserialize, JsonSchema)]
struct PollArgs {
    /// The run to poll.
    run_id: String,
    /// Number of events already consumed; events from here on are returned.
    #[serde(default)]
    cursor: usize,
}

#[derive(Deserialize, JsonSchema)]
struct StatusArgs {
    /// The run to query.
    run_id: String,
}

#[derive(Deserialize, JsonSchema)]
struct SendInputArgs {
    /// The run to feed.
    run_id: String,
    /// A follow-up user message to inject.
    #[serde(default)]
    text: Option<String>,
    /// End the conversation instead of sending a message.
    #[serde(default)]
    end: bool,
}

#[derive(Deserialize, JsonSchema)]
struct PermitArgs {
    /// The run whose pending permission prompt to answer.
    run_id: String,
    /// The `tool_use_id` from the poll response's `permission_request`.
    tool_use_id: String,
    /// Allow (true) or deny (false) the tool call.
    allow: bool,
    /// Optional denial reason.
    #[serde(default)]
    reason: Option<String>,
}

// ---------------------------------------------------------------------------
// The server
// ---------------------------------------------------------------------------

/// The MCP server exposing caliban's drive surface.
pub(crate) struct McpServer {
    factory: Arc<dyn AgentFactory>,
    auth: AuthGate,
    runs: Arc<Mutex<HashMap<String, RunEntry>>>,
    tool_router: ToolRouter<Self>,
}

impl McpServer {
    /// Build a server over `factory`, authorizing connections with `auth`.
    pub(crate) fn new(factory: Arc<dyn AgentFactory>, auth: AuthGate) -> Self {
        Self {
            factory,
            auth,
            runs: Arc::new(Mutex::new(HashMap::new())),
            tool_router: Self::tool_router(),
        }
    }

    fn err(msg: impl Into<String>) -> CallToolResult {
        CallToolResult::error(vec![Content::text(msg.into())])
    }

    fn ok(value: Value) -> Result<CallToolResult, ErrorData> {
        Ok(CallToolResult::success(vec![Content::json(value)?]))
    }
}

#[tool_router]
impl McpServer {
    #[tool(description = "Start an agent run from a prompt; returns a run_id to poll.")]
    async fn caliban_run(
        &self,
        Parameters(args): Parameters<RunArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        // stdio is loopback-inherent; the gate admits local peers (the bearer
        // path bites on the HTTP surface, #531).
        if !self.auth.authorize(Peer::Loopback, None).is_allowed() {
            return Ok(Self::err("unauthorized"));
        }
        let built = match self.factory.build_run(&RunSpec {
            prompt: args.prompt,
            interactive: args.interactive,
        }) {
            Ok(b) => b,
            Err(e) => return Ok(Self::err(format!("failed to build run: {e}"))),
        };

        let cancel = CancellationToken::new();
        let opts = DriveOptions {
            interactive: built.interactive,
            ..DriveOptions::default()
        };
        let session = DriveSession::spawn(built.agent, built.messages, opts, &cancel);
        let run_id = session.id().to_string();

        // Drain the event stream into a buffer the poll tool reads by index.
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut stream = session.subscribe();
        let buf = Arc::clone(&events);
        tokio::spawn(async move {
            while let Some(item) = stream.next().await {
                if let Ok(event) = item {
                    lock(&buf).push(event);
                }
            }
        });

        lock(&self.runs).insert(
            run_id.clone(),
            RunEntry {
                session,
                events,
                perm_rx: built.perm_rx,
                pending: None,
                _cancel: cancel,
            },
        );
        Self::ok(json!({ "run_id": run_id }))
    }

    #[tool(
        description = "Fetch new events for a run from `cursor` onward, plus status and any pending permission prompt."
    )]
    async fn caliban_poll(
        &self,
        Parameters(args): Parameters<PollArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut runs = lock(&self.runs);
        let Some(entry) = runs.get_mut(&args.run_id) else {
            return Ok(Self::err(format!("unknown run_id: {}", args.run_id)));
        };

        let (events_json, next_cursor) = {
            let buf = lock(&entry.events);
            let total = buf.len();
            let start = args.cursor.min(total);
            let events: Vec<Value> = buf[start..]
                .iter()
                .map(|e| json!({ "v": ENVELOPE_VERSION, "event": e }))
                .collect();
            (events, total)
        };

        let status = entry.session.status();
        if entry.pending.is_none() {
            entry.pending = entry.perm_rx.try_recv().ok();
        }
        let permission = entry.pending.as_ref().map(|p| {
            json!({
                "tool_use_id": p.tool_use_id(),
                "tool_name": p.tool_name(),
                "input": p.input(),
            })
        });

        Self::ok(json!({
            "events": events_json,
            "next_cursor": next_cursor,
            "status": status,
            "permission_request": permission,
        }))
    }

    #[tool(description = "Read a run's current lifecycle status.")]
    async fn caliban_status(
        &self,
        Parameters(args): Parameters<StatusArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let runs = lock(&self.runs);
        let Some(entry) = runs.get(&args.run_id) else {
            return Ok(Self::err(format!("unknown run_id: {}", args.run_id)));
        };
        Self::ok(json!({ "status": entry.session.status() }))
    }

    #[tool(description = "Send a follow-up message to an interactive run, or end its input.")]
    async fn caliban_send_input(
        &self,
        Parameters(args): Parameters<SendInputArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let runs = lock(&self.runs);
        let Some(entry) = runs.get(&args.run_id) else {
            return Ok(Self::err(format!("unknown run_id: {}", args.run_id)));
        };
        let inbound = if args.end {
            DriveInbound::EndInput
        } else if let Some(text) = args.text {
            DriveInbound::UserMessage { text }
        } else {
            return Ok(Self::err("send_input requires `text` or `end: true`"));
        };
        match entry.session.send_input(inbound) {
            Ok(()) => Self::ok(json!({ "ok": true })),
            Err(e) => Ok(Self::err(format!("cannot send input: {e}"))),
        }
    }

    #[tool(description = "Answer a run's pending permission prompt (from caliban_poll).")]
    async fn caliban_permit(
        &self,
        Parameters(args): Parameters<PermitArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut runs = lock(&self.runs);
        let Some(entry) = runs.get_mut(&args.run_id) else {
            return Ok(Self::err(format!("unknown run_id: {}", args.run_id)));
        };
        if entry.pending.is_none() {
            entry.pending = entry.perm_rx.try_recv().ok();
        }
        match entry.pending.take() {
            Some(req) if req.tool_use_id() == args.tool_use_id => {
                let outcome = if args.allow {
                    req.allow()
                } else {
                    req.deny(args.reason.unwrap_or_else(|| "denied by client".into()))
                };
                match outcome {
                    Ok(()) => Self::ok(json!({ "ok": true })),
                    Err(_) => Ok(Self::err("run is no longer waiting on that prompt")),
                }
            }
            Some(other) => {
                let expected = other.tool_use_id().to_string();
                entry.pending = Some(other);
                Ok(Self::err(format!(
                    "no pending permission with tool_use_id {}; current prompt is {expected}",
                    args.tool_use_id
                )))
            }
            None => Ok(Self::err("no pending permission request for this run")),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Drive a caliban agent over MCP. Start a run with caliban_run, then loop \
             caliban_poll with an advancing cursor to read TurnEvent batches (each wrapped \
             as {v, event}) until status is done/failed. Use caliban_send_input to feed an \
             interactive run and caliban_permit to answer permission prompts surfaced by poll.",
        )
    }
}

// ---------------------------------------------------------------------------
// Production entry point
// ---------------------------------------------------------------------------

/// The production [`AgentFactory`]: builds a real agent (real provider + the
/// full builtin tool registry) with a fresh [`DriveAskHandler`] per run.
struct ProdAgentFactory {
    args: crate::args::Args,
    settings: caliban_settings::Settings,
    provider: Arc<dyn caliban_provider::Provider + Send + Sync>,
    model: String,
    max_tokens: u32,
    workspace_root: std::path::PathBuf,
}

impl AgentFactory for ProdAgentFactory {
    fn build_run(&self, spec: &RunSpec) -> anyhow::Result<BuiltRun> {
        use caliban_agent_core::{NoopHooks, PermissionsHook, default_rules, new_shared_plan_mode};
        use caliban_tools_builtin::WorkspaceRoot;

        let workspace = WorkspaceRoot::new(self.workspace_root.clone());
        let todos = caliban_agent_core::new_shared_todos();
        let plan_mode = new_shared_plan_mode();
        let mem_cfg = caliban_memory::MemoryConfig::from_env(&self.workspace_root);
        let topic_backend: Arc<dyn caliban_memory::TopicBackend> = Arc::new(
            caliban_memory::FsTopicBackend::new(mem_cfg.auto_memory_dir.clone()),
        );
        let registry = crate::startup::build_registry(
            &self.args,
            workspace,
            todos,
            plan_mode,
            &[],
            &self.settings,
            &topic_backend,
        );

        let (ask, perm_rx) = DriveAskHandler::pair();
        let permissions = PermissionsHook::new(default_rules(), Arc::new(ask), Arc::new(NoopHooks));

        let agent = Agent::builder()
            .provider(Arc::clone(&self.provider))
            .tools(registry)
            .model(&self.model)
            .max_tokens(self.max_tokens)
            .hooks(Arc::new(permissions))
            .build()?;

        Ok(BuiltRun {
            agent: Arc::new(agent),
            messages: vec![Message::user_text(spec.prompt.clone())],
            perm_rx,
            interactive: spec.interactive,
        })
    }
}

/// Serve caliban as an MCP server over stdio (`caliban mcp serve`).
///
/// # Errors
///
/// Propagates provider/settings construction and transport errors.
pub(crate) async fn run_serve(args: &crate::args::Args) -> anyhow::Result<i32> {
    use anyhow::Context as _;

    let settings = crate::startup::load_layered_settings(args, &std::env::current_dir()?)
        .map_err(|e| anyhow::anyhow!("failed to load settings: {e}"))?
        .settings;
    let helper_pool = Arc::new(caliban_settings::ApiKeyHelperPool::from_raw(
        settings.api_key_helper.as_ref(),
    ));
    let provider = crate::startup::build_provider(args, &helper_pool)?;

    let factory = Arc::new(build_prod_factory(args, settings, provider)?);
    let server = McpServer::new(factory, AuthGate::from_env());
    let running = server
        .serve(rmcp::transport::io::stdio())
        .await
        .context("failed to start MCP server over stdio")?;
    running
        .waiting()
        .await
        .context("MCP server terminated with error")?;
    Ok(0)
}

/// Assemble the production [`ProdAgentFactory`] from parsed args, a loaded
/// settings snapshot, and an already-built provider.
///
/// Factored out of [`run_serve`] so the non-I/O assembly (workspace resolution,
/// model defaulting, struct wiring) is unit-testable with a `MockProvider`,
/// leaving only the genuine entrypoint I/O (settings load, provider build,
/// stdio serve loop) in `run_serve`.
///
/// # Errors
///
/// Fails if the current working directory cannot be resolved.
fn build_prod_factory(
    args: &crate::args::Args,
    settings: caliban_settings::Settings,
    provider: Arc<dyn caliban_provider::Provider + Send + Sync>,
) -> anyhow::Result<ProdAgentFactory> {
    use anyhow::Context as _;
    use caliban_tools_builtin::WorkspaceRoot;

    let workspace = WorkspaceRoot::current_dir().context("could not get current directory")?;
    let model = args
        .model
        .clone()
        .unwrap_or_else(|| crate::default_model_for(crate::resolved_provider(args)).to_string());
    Ok(ProdAgentFactory {
        args: args.clone(),
        settings,
        provider,
        model,
        max_tokens: args.max_tokens,
        workspace_root: workspace.root().to_path_buf(),
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use caliban_agent_core::{Agent, ToolCtx, ToolRegistry};
    use caliban_provider::{
        MockProvider, Provider, StopReason, StreamEvent, StreamingContentType, StreamingDelta,
        Usage,
    };
    use rmcp::handler::server::wrapper::Parameters;
    use serde_json::{Value, json};

    use super::{
        AgentFactory, AuthGate, BuiltRun, DriveAskHandler, McpServer, PermitArgs, PollArgs,
        RunArgs, RunSpec, SendInputArgs, StatusArgs,
    };

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

    /// A factory that scripts `turns` mock turns and, for the permit test,
    /// hands the created `DriveAskHandler` back to the test via `captured`.
    struct MockAgentFactory {
        turns: usize,
        captured: Arc<Mutex<Option<DriveAskHandler>>>,
    }

    impl MockAgentFactory {
        fn new(turns: usize) -> Self {
            Self {
                turns,
                captured: Arc::new(Mutex::new(None)),
            }
        }
    }

    impl AgentFactory for MockAgentFactory {
        fn build_run(&self, spec: &RunSpec) -> anyhow::Result<BuiltRun> {
            let mp = Arc::new(MockProvider::new());
            for _ in 0..self.turns {
                mp.enqueue_stream(text_turn("hello"));
            }
            let agent = Agent::builder()
                .provider(mp as Arc<dyn Provider + Send + Sync>)
                .tools(ToolRegistry::default())
                .model("mock-model")
                .max_tokens(64)
                .build()
                .expect("agent builds");
            let (handler, perm_rx) = DriveAskHandler::pair();
            *self.captured.lock().unwrap() = Some(handler);
            Ok(BuiltRun {
                agent: Arc::new(agent),
                messages: vec![caliban_provider::Message::user_text(spec.prompt.clone())],
                perm_rx,
                interactive: spec.interactive,
            })
        }
    }

    fn server(turns: usize) -> McpServer {
        McpServer::new(Arc::new(MockAgentFactory::new(turns)), AuthGate::new(None))
    }

    /// Parse the JSON body out of a successful tool result.
    fn body(result: &rmcp::model::CallToolResult) -> Value {
        assert_ne!(result.is_error, Some(true), "tool returned an error result");
        // The server always emits exactly one JSON content block.
        let raw = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .expect("text content")
            .text
            .clone();
        serde_json::from_str(&raw).expect("valid JSON body")
    }

    async fn poll_until_done(srv: &McpServer, run_id: &str) -> Vec<Value> {
        let mut cursor = 0usize;
        let mut all = Vec::new();
        let mut terminal_seen = false;
        for _ in 0..500 {
            let r = srv
                .caliban_poll(Parameters(PollArgs {
                    run_id: run_id.to_string(),
                    cursor,
                }))
                .await
                .unwrap();
            let b = body(&r);
            let batch = b["events"].as_array().unwrap();
            let got = batch.len();
            for ev in batch {
                all.push(ev.clone());
            }
            cursor = usize::try_from(b["next_cursor"].as_u64().unwrap()).unwrap();
            let terminal = b["status"]["state"] == "done" || b["status"]["state"] == "failed";
            // The event drainer is a separate task, so it can lag the terminal
            // status; keep polling until a terminal poll yields no new events.
            if terminal && terminal_seen && got == 0 {
                break;
            }
            terminal_seen |= terminal;
            tokio::time::sleep(Duration::from_millis(3)).await;
        }
        all
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_then_poll_streams_enveloped_events_to_done() {
        let srv = server(1);
        let r = srv
            .caliban_run(Parameters(RunArgs {
                prompt: "hi".into(),
                interactive: false,
            }))
            .await
            .unwrap();
        let run_id = body(&r)["run_id"].as_str().unwrap().to_string();

        let events = poll_until_done(&srv, &run_id).await;
        assert!(!events.is_empty());
        // Every event is a v1 envelope wrapping a TurnEvent.
        for ev in &events {
            assert_eq!(ev["v"], 1);
            assert!(ev["event"]["type"].is_string());
        }
        let types: Vec<&str> = events
            .iter()
            .map(|e| e["event"]["type"].as_str().unwrap())
            .collect();
        assert!(types.contains(&"TurnStart"), "{types:?}");
        assert_eq!(types.last(), Some(&"RunEnd"), "{types:?}");

        let status = srv
            .caliban_status(Parameters(super::StatusArgs {
                run_id: run_id.clone(),
            }))
            .await
            .unwrap();
        assert_eq!(body(&status)["status"]["state"], "done");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn interactive_run_awaits_and_send_input_resumes_and_ends() {
        let srv = server(2);
        let r = srv
            .caliban_run(Parameters(RunArgs {
                prompt: "hi".into(),
                interactive: true,
            }))
            .await
            .unwrap();
        let run_id = body(&r)["run_id"].as_str().unwrap().to_string();

        // Poll until it parks awaiting input.
        let mut awaited = false;
        for _ in 0..500 {
            let s = srv
                .caliban_status(Parameters(super::StatusArgs {
                    run_id: run_id.clone(),
                }))
                .await
                .unwrap();
            if body(&s)["status"]["state"] == "awaiting_input" {
                awaited = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(3)).await;
        }
        assert!(awaited, "run never awaited input");

        // Feed it, then end.
        srv.caliban_send_input(Parameters(SendInputArgs {
            run_id: run_id.clone(),
            text: Some("more".into()),
            end: false,
        }))
        .await
        .unwrap();
        // Wait for it to park again then end.
        for _ in 0..500 {
            let s = srv
                .caliban_status(Parameters(super::StatusArgs {
                    run_id: run_id.clone(),
                }))
                .await
                .unwrap();
            if body(&s)["status"]["state"] == "awaiting_input" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(3)).await;
        }
        srv.caliban_send_input(Parameters(SendInputArgs {
            run_id: run_id.clone(),
            text: None,
            end: true,
        }))
        .await
        .unwrap();

        let done = poll_until_done(&srv, &run_id).await;
        let _ = done;
        let s = srv
            .caliban_status(Parameters(super::StatusArgs { run_id }))
            .await
            .unwrap();
        assert_eq!(body(&s)["status"]["state"], "done");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn permit_answers_a_surfaced_permission_prompt() {
        let factory = Arc::new(MockAgentFactory::new(1));
        let captured = Arc::clone(&factory.captured);
        let srv = McpServer::new(factory, AuthGate::new(None));

        let r = srv
            .caliban_run(Parameters(RunArgs {
                prompt: "hi".into(),
                interactive: false,
            }))
            .await
            .unwrap();
        let run_id = body(&r)["run_id"].as_str().unwrap().to_string();

        // Simulate the run raising an Ask by driving the captured handler's
        // AskHandler::prompt (it sends a DrivePermissionRequest into the run's
        // perm_rx and awaits the decision).
        let handler = captured.lock().unwrap().take().expect("handler captured");
        let decided = tokio::spawn(async move {
            use caliban_agent_core::AskHandler as _;
            let input = json!({ "command": "ls" });
            let ctx = ToolCtx {
                session_id: "s",
                turn_index: 0,
                tool_use_id: "tu_perm",
                tool_name: "Bash",
                input: &input,
                is_read_only: false,
            };
            handler.prompt(&ctx).await
        });

        // Poll until the permission prompt surfaces.
        let mut surfaced = false;
        for _ in 0..500 {
            let p = srv
                .caliban_poll(Parameters(PollArgs {
                    run_id: run_id.clone(),
                    cursor: 0,
                }))
                .await
                .unwrap();
            if body(&p)["permission_request"]["tool_use_id"] == "tu_perm" {
                surfaced = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(3)).await;
        }
        assert!(surfaced, "permission prompt never surfaced in poll");

        // Answer it.
        let permit = srv
            .caliban_permit(Parameters(super::PermitArgs {
                run_id: run_id.clone(),
                tool_use_id: "tu_perm".into(),
                allow: true,
                reason: None,
            }))
            .await
            .unwrap();
        assert_eq!(body(&permit)["ok"], true);

        let decision = decided.await.unwrap();
        assert!(
            matches!(decision, caliban_agent_core::HookDecision::Allow),
            "{decision:?}"
        );
    }

    #[test]
    fn prod_factory_assembles_and_builds_a_real_agent() {
        // Covers the production path end to end without a network: build the
        // factory from parsed args + default settings + an injected
        // `MockProvider` (build_prod_factory), then build a run from it (real
        // builtin tool registry + permissions hook + agent build).
        use clap::Parser as _;
        let args = crate::args::Args::parse_from(["caliban"]);
        let provider = Arc::new(MockProvider::new()) as Arc<dyn Provider + Send + Sync>;
        let factory =
            super::build_prod_factory(&args, caliban_settings::Settings::default(), provider)
                .expect("factory assembles");
        let built = factory
            .build_run(&RunSpec {
                prompt: "hello".into(),
                interactive: true,
            })
            .expect("build_run succeeds");
        assert_eq!(built.messages.len(), 1);
        assert!(built.interactive);
        drop(built);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unknown_run_id_is_rejected_by_every_tool() {
        let srv = server(0);
        let poll = srv
            .caliban_poll(Parameters(PollArgs {
                run_id: "nope".into(),
                cursor: 0,
            }))
            .await
            .unwrap();
        assert_eq!(poll.is_error, Some(true));
        let status = srv
            .caliban_status(Parameters(StatusArgs {
                run_id: "nope".into(),
            }))
            .await
            .unwrap();
        assert_eq!(status.is_error, Some(true));
        let send = srv
            .caliban_send_input(Parameters(SendInputArgs {
                run_id: "nope".into(),
                text: Some("x".into()),
                end: false,
            }))
            .await
            .unwrap();
        assert_eq!(send.is_error, Some(true));
        let permit = srv
            .caliban_permit(Parameters(PermitArgs {
                run_id: "nope".into(),
                tool_use_id: "x".into(),
                allow: true,
                reason: None,
            }))
            .await
            .unwrap();
        assert_eq!(permit.is_error, Some(true));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn send_input_requires_text_or_end() {
        let srv = server(1);
        let r = srv
            .caliban_run(Parameters(RunArgs {
                prompt: "hi".into(),
                interactive: true,
            }))
            .await
            .unwrap();
        let run_id = body(&r)["run_id"].as_str().unwrap().to_string();
        let e = srv
            .caliban_send_input(Parameters(SendInputArgs {
                run_id,
                text: None,
                end: false,
            }))
            .await
            .unwrap();
        assert_eq!(e.is_error, Some(true));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn permit_without_a_pending_prompt_errors() {
        let srv = server(1);
        let r = srv
            .caliban_run(Parameters(RunArgs {
                prompt: "hi".into(),
                interactive: false,
            }))
            .await
            .unwrap();
        let run_id = body(&r)["run_id"].as_str().unwrap().to_string();
        let e = srv
            .caliban_permit(Parameters(PermitArgs {
                run_id,
                tool_use_id: "x".into(),
                allow: true,
                reason: None,
            }))
            .await
            .unwrap();
        assert_eq!(e.is_error, Some(true));
    }

    // -----------------------------------------------------------------------
    // End-to-end (#529): drive the server through a real rmcp client over an
    // in-process bidirectional transport (a `tokio::io::duplex` pair standing
    // in for stdio) — no TUI, no hand-called handlers. Exercises the actual MCP
    // initialize handshake, tool listing, and `call_tool` dispatch/serialization.
    // -----------------------------------------------------------------------

    /// Call a tool through the rmcp client and return the parsed JSON body.
    macro_rules! call_tool {
        ($client:expr, $name:expr, $args:expr) => {{
            let mut req = rmcp::model::CallToolRequestParams::new($name);
            req.arguments = $args.as_object().cloned();
            let res = $client.call_tool(req).await.expect($name);
            body(&res)
        }};
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn e2e_client_drives_run_stream_status_input_over_mcp() {
        use rmcp::ServiceExt as _;

        // In-process bidirectional transport standing in for stdio.
        let (server_end, client_end) = tokio::io::duplex(64 * 1024);

        // Serve the mock-backed server on one end.
        let server = McpServer::new(Arc::new(MockAgentFactory::new(2)), AuthGate::new(None));
        let server_task = tokio::spawn(async move {
            if let Ok(running) = server.serve(server_end).await {
                let _ = running.waiting().await;
            }
        });

        // Connect a bare rmcp client on the other end (performs the handshake).
        let client = ().serve(client_end).await.expect("client initializes");

        // The server advertises its tools.
        let tools = client.list_all_tools().await.expect("list_all_tools");
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        for expected in [
            "caliban_run",
            "caliban_poll",
            "caliban_status",
            "caliban_send_input",
            "caliban_permit",
        ] {
            assert!(names.contains(&expected), "missing {expected}: {names:?}");
        }

        // run — interactive so we can exercise send_input.
        let run = call_tool!(
            client,
            "caliban_run",
            json!({ "prompt": "hi", "interactive": true })
        );
        let run_id = run["run_id"].as_str().unwrap().to_string();

        // Poll (stream) + status until the run parks awaiting input.
        let mut cursor = 0u64;
        let mut types: Vec<String> = Vec::new();
        let mut awaited = false;
        for _ in 0..500 {
            let b = call_tool!(
                client,
                "caliban_poll",
                json!({ "run_id": run_id, "cursor": cursor })
            );
            for ev in b["events"].as_array().unwrap() {
                assert_eq!(ev["v"], 1);
                types.push(ev["event"]["type"].as_str().unwrap().to_string());
            }
            cursor = b["next_cursor"].as_u64().unwrap();
            if b["status"]["state"] == "awaiting_input" {
                awaited = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(3)).await;
        }
        assert!(awaited, "run never awaited input; saw {types:?}");
        assert!(types.iter().any(|t| t == "TurnStart"), "{types:?}");

        // input — end the conversation.
        let sent = call_tool!(
            client,
            "caliban_send_input",
            json!({ "run_id": run_id, "end": true })
        );
        assert_eq!(sent["ok"], true);

        // Poll to completion.
        let mut terminal_seen = false;
        for _ in 0..500 {
            let b = call_tool!(
                client,
                "caliban_poll",
                json!({ "run_id": run_id, "cursor": cursor })
            );
            let batch = b["events"].as_array().unwrap();
            for ev in batch {
                types.push(ev["event"]["type"].as_str().unwrap().to_string());
            }
            cursor = b["next_cursor"].as_u64().unwrap();
            let terminal = b["status"]["state"] == "done";
            if terminal && terminal_seen && batch.is_empty() {
                break;
            }
            terminal_seen |= terminal;
            tokio::time::sleep(Duration::from_millis(3)).await;
        }
        assert_eq!(
            types.last().map(String::as_str),
            Some("RunEnd"),
            "{types:?}"
        );

        // status reads done.
        let s = call_tool!(client, "caliban_status", json!({ "run_id": run_id }));
        assert_eq!(s["status"]["state"], "done");

        let _ = client.cancel().await;
        server_task.abort();
    }
}
