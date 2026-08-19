//! Protocol-agnostic run registry shared by the drive surfaces (ADR 0055).
//!
//! [`DriveRegistry`] owns the server-side state and lifecycle of active runs —
//! the [`DriveSession`], a buffered event log for cursor polling, and the
//! per-run permission channel — plus the operations every surface needs
//! (`spawn` / `poll` / `status` / `send_input` / `permit`). The MCP-server,
//! HTTP-serve, and ACP adapters are thin protocol layers over it: they
//! translate their wire format to/from these calls. The registry deals only in
//! `caliban_drive` types (`TurnEvent`, `DriveStatus`); wire concerns such as the
//! `{v, event}` envelope live in the adapters.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use caliban_agent_core::{Agent, TurnEvent};
use caliban_drive::{DriveInbound, DriveOptions, DriveSession, DriveStatus};
use caliban_provider::Message;
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;

use crate::serve::permissions::{DriveAskHandler, DrivePermissionRequest, PermissionDecision};

// ---------------------------------------------------------------------------
// Agent-factory seam — lets a surface be tested with a MockProvider agent and
// driven with a real one in production.
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
    pub(crate) perm_rx: tokio::sync::mpsc::UnboundedReceiver<DrivePermissionRequest>,
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
// Views + outcomes returned to adapters
// ---------------------------------------------------------------------------

/// A permission prompt surfaced to the client, awaiting a decision.
pub(crate) struct PendingPermission {
    /// The `tool_use_id` of the gated tool call.
    pub(crate) tool_use_id: String,
    /// The tool being gated.
    pub(crate) tool_name: String,
    /// The tool's input JSON.
    pub(crate) input: serde_json::Value,
}

/// The result of a [`DriveRegistry::poll`].
pub(crate) struct PollView {
    /// New events since the requested cursor.
    pub(crate) events: Vec<TurnEvent>,
    /// Cursor to pass to the next poll.
    pub(crate) next_cursor: usize,
    /// Current lifecycle status.
    pub(crate) status: DriveStatus,
    /// A pending permission prompt, if the run is blocked on one.
    pub(crate) pending: Option<PendingPermission>,
}

/// Failure modes for [`DriveRegistry::send_input`].
#[derive(Debug)]
pub(crate) enum SendInputError {
    /// No run with that id.
    UnknownRun,
    /// The run's input channel is closed (it has ended).
    Ended(String),
}

/// The result of [`DriveRegistry::permit`].
#[derive(Debug)]
pub(crate) enum PermitOutcome {
    /// No run with that id.
    UnknownRun,
    /// The run has no pending permission prompt.
    NoPending,
    /// A prompt is pending, but for a different `tool_use_id`.
    Mismatch {
        /// The `tool_use_id` actually awaiting a decision.
        expected: String,
    },
    /// The decision was delivered.
    Answered,
    /// The run moved on before the decision could be delivered.
    RunGone,
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Server-side state for one active run.
struct RunEntry {
    session: DriveSession,
    /// Events accumulated by the per-run drainer task, read by `poll`.
    events: Arc<Mutex<Vec<TurnEvent>>>,
    /// Permission prompts raised by the run, drained on poll/permit.
    perm_rx: tokio::sync::mpsc::UnboundedReceiver<DrivePermissionRequest>,
    /// The currently-surfaced permission prompt awaiting a client decision.
    pending: Option<DrivePermissionRequest>,
    /// Kept alive so the run's parent cancellation scope outlives the run.
    _cancel: CancellationToken,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The shared, protocol-agnostic registry of active runs.
#[derive(Clone, Default)]
pub(crate) struct DriveRegistry {
    runs: Arc<Mutex<HashMap<String, RunEntry>>>,
}

impl DriveRegistry {
    /// An empty registry.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Spawn a built run: start its `DriveSession`, begin draining its events,
    /// register it, and return the run id.
    pub(crate) fn spawn(&self, built: BuiltRun) -> String {
        let cancel = CancellationToken::new();
        let opts = DriveOptions {
            interactive: built.interactive,
            ..DriveOptions::default()
        };
        let session = DriveSession::spawn(built.agent, built.messages, opts, &cancel);
        let run_id = session.id().to_string();

        // Drain the event stream into a buffer that `poll` reads by index.
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
        run_id
    }

    /// Fetch new events from `cursor` onward, plus status and any pending
    /// permission prompt. Returns `None` if the run is unknown.
    pub(crate) fn poll(&self, run_id: &str, cursor: usize) -> Option<PollView> {
        let mut runs = lock(&self.runs);
        let entry = runs.get_mut(run_id)?;

        let (events, next_cursor) = {
            let buf = lock(&entry.events);
            let total = buf.len();
            let start = cursor.min(total);
            (buf[start..].to_vec(), total)
        };

        let status = entry.session.status();
        if entry.pending.is_none() {
            entry.pending = entry.perm_rx.try_recv().ok();
        }
        let pending = entry.pending.as_ref().map(|p| PendingPermission {
            tool_use_id: p.tool_use_id().to_string(),
            tool_name: p.tool_name().to_string(),
            input: p.input().clone(),
        });

        Some(PollView {
            events,
            next_cursor,
            status,
            pending,
        })
    }

    /// The run's current status, or `None` if unknown.
    pub(crate) fn status(&self, run_id: &str) -> Option<DriveStatus> {
        Some(lock(&self.runs).get(run_id)?.session.status())
    }

    /// Deliver an inbound message to an interactive run.
    ///
    /// # Errors
    ///
    /// [`SendInputError::UnknownRun`] if there is no such run;
    /// [`SendInputError::Ended`] if the run's input channel is closed.
    pub(crate) fn send_input(
        &self,
        run_id: &str,
        inbound: DriveInbound,
    ) -> Result<(), SendInputError> {
        let runs = lock(&self.runs);
        let entry = runs.get(run_id).ok_or(SendInputError::UnknownRun)?;
        entry
            .session
            .send_input(inbound)
            .map_err(|e| SendInputError::Ended(e.to_string()))
    }

    /// Answer a run's pending permission prompt.
    pub(crate) fn permit(
        &self,
        run_id: &str,
        tool_use_id: &str,
        decision: PermissionDecision,
    ) -> PermitOutcome {
        let mut runs = lock(&self.runs);
        let Some(entry) = runs.get_mut(run_id) else {
            return PermitOutcome::UnknownRun;
        };
        if entry.pending.is_none() {
            entry.pending = entry.perm_rx.try_recv().ok();
        }
        match entry.pending.take() {
            Some(req) if req.tool_use_id() == tool_use_id => match req.answer(decision) {
                Ok(()) => PermitOutcome::Answered,
                Err(_) => PermitOutcome::RunGone,
            },
            Some(other) => {
                let expected = other.tool_use_id().to_string();
                entry.pending = Some(other);
                PermitOutcome::Mismatch { expected }
            }
            None => PermitOutcome::NoPending,
        }
    }
}

// ---------------------------------------------------------------------------
// Production agent factory — shared by every serve entrypoint.
// ---------------------------------------------------------------------------

/// The production [`AgentFactory`]: builds a real agent (real provider + the
/// full builtin tool registry) with a fresh [`DriveAskHandler`] per run.
pub(crate) struct ProdAgentFactory {
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

/// Assemble the production [`ProdAgentFactory`] from parsed args, a loaded
/// settings snapshot, and an already-built provider.
///
/// Factored out of the serve entrypoints so the non-I/O assembly (workspace
/// resolution, model defaulting, struct wiring) is unit-testable with a
/// `MockProvider`, leaving only genuine entrypoint I/O (settings load, provider
/// build, bind/serve loop) in the entrypoints.
///
/// # Errors
///
/// Fails if the current working directory cannot be resolved.
pub(crate) fn build_prod_factory(
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use caliban_agent_core::{Agent, ToolRegistry};
    use caliban_provider::{
        MockProvider, Provider, StopReason, StreamEvent, StreamingContentType, StreamingDelta,
        Usage,
    };

    use super::{AgentFactory, BuiltRun, DriveRegistry, PermitOutcome, RunSpec, SendInputError};
    use crate::serve::permissions::{DriveAskHandler, PermissionDecision};

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

    struct MockFactory {
        turns: usize,
    }

    impl AgentFactory for MockFactory {
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_poll_status_reaches_done() {
        let reg = DriveRegistry::new();
        let built = MockFactory { turns: 1 }
            .build_run(&RunSpec {
                prompt: "hi".into(),
                interactive: false,
            })
            .unwrap();
        let run_id = reg.spawn(built);

        let mut cursor = 0;
        let mut saw_run_end = false;
        for _ in 0..500 {
            let view = reg.poll(&run_id, cursor).expect("known run");
            for e in &view.events {
                if matches!(e, caliban_agent_core::TurnEvent::RunEnd { .. }) {
                    saw_run_end = true;
                }
            }
            cursor = view.next_cursor;
            if view.status.is_terminal() && view.events.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(3)).await;
        }
        assert!(saw_run_end);
        assert_eq!(reg.status(&run_id), Some(caliban_drive::DriveStatus::Done));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unknown_run_is_reported() {
        let reg = DriveRegistry::new();
        assert!(reg.poll("nope", 0).is_none());
        assert!(reg.status("nope").is_none());
        assert!(matches!(
            reg.send_input("nope", caliban_drive::DriveInbound::EndInput),
            Err(SendInputError::UnknownRun)
        ));
        assert!(matches!(
            reg.permit("nope", "x", PermissionDecision::Allow),
            PermitOutcome::UnknownRun
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn permit_without_pending_is_reported() {
        let reg = DriveRegistry::new();
        let built = MockFactory { turns: 1 }
            .build_run(&RunSpec {
                prompt: "hi".into(),
                interactive: false,
            })
            .unwrap();
        let run_id = reg.spawn(built);
        assert!(matches!(
            reg.permit(&run_id, "x", PermissionDecision::Allow),
            PermitOutcome::NoPending
        ));
    }

    #[test]
    fn prod_factory_assembles_and_builds_a_real_agent() {
        // Covers build_prod_factory + ProdAgentFactory::build_run without a
        // network: parsed args + default settings + an injected MockProvider.
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
}
