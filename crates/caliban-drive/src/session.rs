//! [`DriveSession`] — the transport-agnostic drive core.
//!
//! A `DriveSession` wraps a single agent run and exposes the four operations
//! every driveable surface needs, with no protocol or transport assumptions:
//!
//! - **run**  — [`DriveSession::spawn`] starts the run on a background task.
//! - **stream** — [`DriveSession::subscribe`] returns a replay-then-live
//!   `TurnEventStream`; late subscribers still see the whole run.
//! - **status** — [`DriveSession::status`] returns the current [`DriveStatus`].
//! - **input** — [`DriveSession::send_input`] delivers a [`DriveInbound`] into
//!   an interactive run.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use caliban_agent_core::{Agent, InputProvider, RunSettings, TurnEvent, TurnEventStream};
use caliban_provider::Message;
use futures::StreamExt as _;
use tokio::sync::{broadcast, mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::error::DriveError;
use crate::inbound::DriveInbound;
use crate::status::DriveStatus;

/// Default cap on the number of buffered events retained for replay.
const DEFAULT_HISTORY_CAP: usize = 1024;

static RUN_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Generate a process-unique run identifier of the form `run-<n>`.
#[must_use]
pub fn new_run_id() -> String {
    format!("run-{}", RUN_COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// Configuration for a [`DriveSession`].
#[derive(Clone, Debug)]
pub struct DriveOptions {
    /// Stable identifier for this run, surfaced by [`DriveSession::id`].
    pub run_id: String,
    /// Opaque session identifier threaded into [`RunSettings::session_id`].
    pub session_id: String,
    /// Workspace root threaded into [`RunSettings::workspace_root`].
    pub workspace_root: PathBuf,
    /// Monotonic prompt index threaded into [`RunSettings::prompt_index`].
    pub prompt_index: u32,
    /// When `true`, an input source is wired so the run pauses at each
    /// end-of-turn boundary awaiting a [`DriveInbound`] instead of ending.
    pub interactive: bool,
    /// Maximum number of events retained for replay to late subscribers.
    pub history_cap: usize,
}

impl Default for DriveOptions {
    fn default() -> Self {
        Self {
            run_id: new_run_id(),
            session_id: String::new(),
            workspace_root: PathBuf::from("."),
            prompt_index: 0,
            interactive: false,
            history_cap: DEFAULT_HISTORY_CAP,
        }
    }
}

/// A broadcast hub that fans one run's events out to any number of subscribers,
/// retaining a bounded history so a subscriber that attaches mid-run (or after
/// the run ends) still replays every buffered event exactly once.
///
/// Publishing and subscribing are serialized on the same lock so the history
/// snapshot and the live receiver hand-off can never race: a subscriber sees
/// each event either in its replay snapshot or from the live channel, never
/// both and never neither.
struct Hub {
    history: Mutex<VecDeque<TurnEvent>>,
    tx: broadcast::Sender<TurnEvent>,
    cap: usize,
}

impl Hub {
    fn new(cap: usize) -> Self {
        let (tx, _rx) = broadcast::channel(cap.max(1));
        Self {
            history: Mutex::new(VecDeque::new()),
            tx,
            cap,
        }
    }

    fn publish(&self, event: TurnEvent) {
        let mut history = self
            .history
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if history.len() == self.cap {
            history.pop_front();
        }
        history.push_back(event.clone());
        // Send while still holding the lock so ordering vs. `subscribe` is
        // total. A send error only means there are no live receivers, which is
        // fine — the event is already in `history` for future subscribers.
        let _ = self.tx.send(event);
    }

    fn subscribe(&self) -> (Vec<TurnEvent>, broadcast::Receiver<TurnEvent>) {
        let history = self
            .history
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let rx = self.tx.subscribe();
        (history.iter().cloned().collect(), rx)
    }
}

/// Shared state behind a [`DriveSession`] handle.
struct Inner {
    run_id: String,
    hub: Arc<Hub>,
    status_rx: watch::Receiver<DriveStatus>,
    // Held so `watch::Sender::send` from the driver task and the input provider
    // never fails for want of a receiver, and to keep the channel open.
    _status_tx: watch::Sender<DriveStatus>,
    input_tx: Option<mpsc::UnboundedSender<DriveInbound>>,
    cancel: CancellationToken,
}

/// A single driven agent run.
///
/// Cloneable handle semantics are intentionally *not* provided: one
/// `DriveSession` owns one run. Share it behind an `Arc` if multiple owners
/// need to observe the same run.
pub struct DriveSession {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for DriveSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DriveSession")
            .field("run_id", &self.inner.run_id)
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

impl DriveSession {
    /// Start a run and return a handle to drive it.
    ///
    /// The run executes on a spawned background task, so this returns
    /// immediately — subscribe to observe events, poll [`DriveSession::status`],
    /// and (for interactive sessions) [`DriveSession::send_input`] to feed it.
    #[must_use]
    pub fn spawn(
        agent: Arc<Agent>,
        messages: Vec<Message>,
        opts: DriveOptions,
        cancel: &CancellationToken,
    ) -> Self {
        let hub = Arc::new(Hub::new(opts.history_cap));
        let (status_tx, status_rx) = watch::channel(DriveStatus::Starting);

        // Run on a *child* of the caller's token. Parent cancellation (e.g.
        // Ctrl-C) still propagates down to the run, but cancelling this run —
        // including on `DriveSession` drop — never cancels the caller's token,
        // which a driver such as the headless multi-frame loop reuses across
        // several sequential runs.
        let run_cancel = cancel.child_token();

        // Wire an input source only for interactive sessions; a non-interactive
        // run keeps the exact run-to-completion behavior of a plain
        // `stream_until_done` (no input boundary).
        let (input_tx, input_source): (
            Option<mpsc::UnboundedSender<DriveInbound>>,
            Option<Arc<dyn InputProvider>>,
        ) = if opts.interactive {
            let (tx, rx) = mpsc::unbounded_channel::<DriveInbound>();
            let provider = Arc::new(ChannelInputProvider {
                rx: tokio::sync::Mutex::new(rx),
                status: status_tx.clone(),
            });
            (Some(tx), Some(provider as Arc<dyn InputProvider>))
        } else {
            (None, None)
        };

        let settings = RunSettings {
            session_id: opts.session_id,
            workspace_root: opts.workspace_root,
            prompt_index: opts.prompt_index,
            input_source,
        };

        let driver_hub = Arc::clone(&hub);
        let driver_status = status_tx.clone();
        let driver_cancel = run_cancel.clone();
        tokio::spawn(async move {
            drive_run(
                agent,
                messages,
                settings,
                driver_cancel,
                driver_hub,
                driver_status,
            )
            .await;
        });

        Self {
            inner: Arc::new(Inner {
                run_id: opts.run_id,
                hub,
                status_rx,
                _status_tx: status_tx,
                input_tx,
                cancel: run_cancel,
            }),
        }
    }

    /// The stable identifier for this run.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.inner.run_id
    }

    /// The current lifecycle status.
    #[must_use]
    pub fn status(&self) -> DriveStatus {
        self.inner.status_rx.borrow().clone()
    }

    /// Subscribe to this run's events.
    ///
    /// The returned stream first replays every buffered event (so a subscriber
    /// that attaches mid-run, or after the run has ended, still sees the run
    /// from the start), then yields live events until the run reaches a terminal
    /// state. Run failures are reported via [`DriveSession::status`], not as an
    /// error item on this stream.
    #[must_use]
    pub fn subscribe(&self) -> TurnEventStream {
        let (snapshot, mut rx) = self.inner.hub.subscribe();
        let mut status_rx = self.inner.status_rx.clone();
        // `borrow_and_update` marks the current version seen, so the first
        // `changed()` in the loop below awaits the *next* transition rather
        // than returning immediately in a busy loop.
        let terminal_at_subscribe = status_rx.borrow_and_update().is_terminal();

        let stream = async_stream::stream! {
            for event in snapshot {
                yield Ok(event);
            }

            // The run already ended before we subscribed: everything is in the
            // replay snapshot plus whatever the live channel already buffered.
            if terminal_at_subscribe {
                loop {
                    match rx.try_recv() {
                        Ok(event) => yield Ok(event),
                        // Lagged: buffered events were dropped; keep draining.
                        Err(broadcast::error::TryRecvError::Lagged(_)) => {}
                        Err(_) => break,
                    }
                }
                return;
            }

            loop {
                tokio::select! {
                    received = rx.recv() => match received {
                        Ok(event) => yield Ok(event),
                        Err(broadcast::error::RecvError::Closed) => break,
                        // Lagged: a slow subscriber dropped events; resync.
                        Err(broadcast::error::RecvError::Lagged(_)) => {}
                    },
                    changed = status_rx.changed() => {
                        if changed.is_err() || status_rx.borrow_and_update().is_terminal() {
                            // The driver publishes all events before flipping to
                            // a terminal status, so anything still queued in the
                            // channel is drainable now. Drain it, then end.
                            loop {
                                match rx.try_recv() {
                                    Ok(event) => yield Ok(event),
                                    // Lagged: buffered events were dropped; keep draining.
                        Err(broadcast::error::TryRecvError::Lagged(_)) => {}
                                    Err(_) => break,
                                }
                            }
                            break;
                        }
                    }
                }
            }
        };
        Box::pin(stream)
    }

    /// Deliver an inbound message to an interactive run.
    ///
    /// # Errors
    ///
    /// Returns [`DriveError::NotInteractive`] if the session was created without
    /// `interactive`, or [`DriveError::Ended`] if the run has already ended and
    /// its input channel is closed.
    pub fn send_input(&self, inbound: DriveInbound) -> Result<(), DriveError> {
        match &self.inner.input_tx {
            Some(tx) => tx.send(inbound).map_err(|_| DriveError::Ended),
            None => Err(DriveError::NotInteractive),
        }
    }

    /// Request cancellation of the run.
    pub fn cancel(&self) {
        self.inner.cancel.cancel();
    }

    /// Await the run reaching a terminal state, returning that final status.
    #[must_use = "the final status describes how the run ended"]
    pub async fn wait_done(&self) -> DriveStatus {
        let mut rx = self.inner.status_rx.clone();
        loop {
            // `borrow_and_update` marks the current version seen so the
            // following `changed()` awaits the next transition instead of
            // returning immediately.
            if rx.borrow_and_update().is_terminal() {
                return rx.borrow().clone();
            }
            if rx.changed().await.is_err() {
                return rx.borrow().clone();
            }
        }
    }
}

impl Drop for DriveSession {
    /// Cancel the run when the owning handle is dropped.
    ///
    /// This mirrors the semantics of dropping a raw `TurnEventStream` (which
    /// stops the run): a consumer that stops observing a `DriveSession` no
    /// longer wants the run to continue. Because the run executes on a child
    /// token (see [`DriveSession::spawn`]), this never disturbs the caller's
    /// own cancellation token.
    fn drop(&mut self) {
        self.inner.cancel.cancel();
    }
}

/// Drive the underlying agent stream, fanning events into `hub` and updating
/// `status` at the run's transitions.
async fn drive_run(
    agent: Arc<Agent>,
    messages: Vec<Message>,
    settings: RunSettings,
    cancel: CancellationToken,
    hub: Arc<Hub>,
    status: watch::Sender<DriveStatus>,
) {
    let _ = status.send(DriveStatus::Running);
    let mut stream = agent.stream_until_done_with_settings(messages, cancel, settings);

    let mut failure: Option<String> = None;
    while let Some(item) = stream.next().await {
        match item {
            Ok(event) => hub.publish(event),
            Err(err) => {
                failure = Some(err.to_string());
                break;
            }
        }
    }

    let terminal = match failure {
        Some(error) => DriveStatus::Failed { error },
        None => DriveStatus::Done,
    };
    let _ = status.send(terminal);
}

/// An [`InputProvider`] backed by an mpsc channel, translating [`DriveInbound`]
/// frames into the messages the agent loop resumes with, and reflecting the
/// awaiting/running transition into the shared status.
struct ChannelInputProvider {
    rx: tokio::sync::Mutex<mpsc::UnboundedReceiver<DriveInbound>>,
    status: watch::Sender<DriveStatus>,
}

#[async_trait]
impl InputProvider for ChannelInputProvider {
    async fn next_input(&self, cancel: &CancellationToken) -> Option<Vec<Message>> {
        // Reaching this await point *is* the run going idle awaiting input.
        let _ = self.status.send(DriveStatus::AwaitingInput);
        let mut rx = self.rx.lock().await;
        let inbound = tokio::select! {
            () = cancel.cancelled() => return None,
            received = rx.recv() => received,
        };
        match inbound {
            Some(DriveInbound::UserMessage { text }) => {
                let _ = self.status.send(DriveStatus::Running);
                Some(vec![Message::user_text(text)])
            }
            Some(DriveInbound::EndInput) | None => None,
        }
    }
}
