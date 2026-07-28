//! Debounced session JSON writer.
//!
//! The on-disk session file used to be rewritten synchronously on every
//! turn end (and on each TUI redraw cycle that took the save path). For
//! long sessions this turned into both a latency tax (~10ms per turn for
//! a moderately sized JSON blob) and an IO amplifier — every interim
//! snapshot hit the disk, not just the meaningful ones.
//!
//! [`DebouncedWriter`] replaces that with a `tokio::sync::mpsc`-driven
//! writer task. Each call to [`DebouncedWriter::request`] enqueues the
//! latest bytes for a target path; the writer collapses bursts inside a
//! 250 ms debounce window into a single [`caliban_common::fs::write_atomic`]
//! call. The timer is reset on every new request, so a steady drumbeat
//! of writes within the window only flushes once it goes quiet.
//!
//! Crash safety:
//! - On a clean drop of the writer (the [`DebouncedWriter`] handle goes
//!   away), the spawned thread drains any pending request synchronously
//!   before exiting. Callers may also invoke [`DebouncedWriter::flush`]
//!   to block until the in-flight buffer is on disk.
//! - On panic / abort, any in-flight debounced write may be lost — same
//!   contract as the pre-change synchronous path (which also offered no
//!   protection against a half-executed process).
//!
//! The writer is hosted on a dedicated OS thread that owns a
//! `current_thread` tokio runtime so this module works regardless of
//! whether the caller is inside an existing runtime (TUI / headless) or
//! not (integration tests, ad-hoc scripts).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;

use crate::backend::SessionBackend;
use crate::session::PersistedSession;
use crate::store::SessionMetadata;

/// Window across which back-to-back writes collapse into one disk write.
pub(crate) const DEBOUNCE_WINDOW: Duration = Duration::from_millis(250);

/// Hard ceiling on how long the *oldest* un-flushed write may sit before it is
/// forced to disk, regardless of a sustained request stream that keeps resetting
/// the debounce window. Without it, the `biased` `select!` in [`writer_loop`]
/// would poll incoming requests first and could starve the timer indefinitely,
/// unboundedly extending the crash-loss window (#414, P10).
pub(crate) const MAX_DELAY: Duration = Duration::from_secs(1);

/// Cap on how long [`DebouncedWriter::drop`] will wait for the writer
/// thread to drain its pending request. Drop must not hang the process,
/// so we bound the wait — if the disk is wedged, we abandon the write
/// and emit a warning.
pub(crate) const DROP_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

/// A single persist request: hand the latest `session` snapshot to the
/// backend.
///
/// The struct is owned by the writer task; the public API only exposes
/// `request` / `flush` / read round-trips.
struct PersistRequest {
    session: PersistedSession,
}

/// Control messages multiplexed onto the same channel.
///
/// Reads (`Load`/`List`/`Delete`) are round-tripped through the worker so the
/// public [`SessionStore`](crate::store::SessionStore) API stays synchronous:
/// the worker owns the async runtime and awaits the backend, while the caller
/// parks on a `std::sync::mpsc` receiver — never calling `block_on` from inside
/// a `#[tokio::main]` context. Each read carries a `std::sync::mpsc::Sender` of
/// its typed result (`String` is the error, matching `flush`'s convention).
enum WriterMsg {
    Persist(PersistRequest),
    /// Block the writer until it finishes any pending flush, then signal
    /// completion via a std mpsc sender. Used to implement `flush()`.
    ///
    /// We deliberately use `std::sync::mpsc` (not `tokio::sync::oneshot`)
    /// here: `flush()` is a synchronous public API called from inside the
    /// caller's tokio runtime context (e.g. `#[tokio::main]` startup),
    /// and `oneshot::Receiver::blocking_recv` panics in that situation.
    /// The std channel has no runtime opinion — it just parks the OS
    /// thread, which is what we want.
    /// The `Ok`/`Err` carries the outcome of the flush's drain so the caller
    /// can observe a failed persist instead of it being only warn-logged (#414).
    Flush(std::sync::mpsc::Sender<Result<(), String>>),
    /// Drain pending writes, then load a session by name through the backend.
    Load(
        String,
        std::sync::mpsc::Sender<Result<Option<PersistedSession>, String>>,
    ),
    /// Drain pending writes, then list session metadata through the backend.
    List(std::sync::mpsc::Sender<Result<Vec<SessionMetadata>, String>>),
    /// Drain pending writes, then delete a session by name through the backend.
    Delete(String, std::sync::mpsc::Sender<Result<(), String>>),
}

/// Handle to the debounced writer. Cheap to clone (`Arc` internally).
///
/// The writer task is started in [`DebouncedWriter::new`] and shut down
/// on `Drop` of the last clone — at that point any pending debounced
/// request is drained before the worker thread joins.
#[derive(Clone)]
pub(crate) struct DebouncedWriter {
    inner: Arc<WriterInner>,
}

/// Most recent write failure, shared between the worker thread (which records
/// it) and the handle (which exposes it). `Some((name, message))` means the
/// last write to session `name` failed and no later write to it has succeeded;
/// `None` means the last observed write succeeded. Lets a failed deferred
/// persist be observed even when it flushed via the timer, not an explicit
/// `flush` (#414).
type LastError = Arc<Mutex<Option<(String, String)>>>;

struct WriterInner {
    tx: mpsc::UnboundedSender<WriterMsg>,
    last_error: LastError,
    // Worker thread join handle. Mutex<Option> so `Drop` can `take` it
    // even though `Drop` only has `&mut self` on the Arc's inner via
    // get_mut (impossible when other clones exist — but only the *last*
    // arc drop triggers `Drop for WriterInner`, so this is always
    // exclusive).
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl DebouncedWriter {
    /// Spawn the writer task on a dedicated OS thread, driving `backend`.
    pub(crate) fn new(backend: Arc<dyn SessionBackend>) -> Self {
        Self::with_window_and_max_delay(backend, DEBOUNCE_WINDOW, MAX_DELAY)
    }

    /// Like [`DebouncedWriter::new`] but lets tests dial the debounce window
    /// (max-delay bound scaled to the default ceiling).
    #[cfg(test)]
    pub(crate) fn with_window(backend: Arc<dyn SessionBackend>, window: Duration) -> Self {
        Self::with_window_and_max_delay(backend, window, MAX_DELAY)
    }

    /// Like [`DebouncedWriter::new`] but lets tests dial both the debounce
    /// window and the max-delay ceiling.
    pub(crate) fn with_window_and_max_delay(
        backend: Arc<dyn SessionBackend>,
        window: Duration,
        max_delay: Duration,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel::<WriterMsg>();
        let last_error: LastError = Arc::new(Mutex::new(None));
        let last_error_worker = Arc::clone(&last_error);
        // `backend` is not needed on this side of the spawn, so move it
        // straight into the worker rather than cloning the `Arc`.
        let thread = std::thread::Builder::new()
            .name("caliban-session-writer".into())
            .spawn(move || {
                run_writer_thread(rx, window, max_delay, &last_error_worker, backend);
            })
            .expect("spawn session writer thread");
        Self {
            inner: Arc::new(WriterInner {
                tx,
                last_error,
                thread: Mutex::new(Some(thread)),
            }),
        }
    }

    /// Enqueue a persist request. Returns immediately — the actual backend
    /// save happens after the debounce window elapses, or sooner via
    /// [`DebouncedWriter::flush`] / shutdown. Back-to-back requests for the
    /// same session name coalesce, latest wins.
    pub(crate) fn request(&self, session: PersistedSession) {
        // Send failure means the worker thread has gone away (only
        // possible during shutdown). Drop the request rather than panic.
        let _ = self
            .inner
            .tx
            .send(WriterMsg::Persist(PersistRequest { session }));
    }

    /// Block until any pending request has been flushed to disk, returning the
    /// drain outcome so a failed persist is observable (#414).
    ///
    /// Safe to call from inside or outside a tokio runtime — it blocks
    /// the calling thread on a `std::sync::mpsc` receiver, which has no
    /// runtime opinion. If the writer thread has already exited (e.g.
    /// during shutdown), returns `Ok(())` (nothing left to flush).
    pub(crate) fn flush(&self) -> Result<(), String> {
        let (done_tx, done_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        if self.inner.tx.send(WriterMsg::Flush(done_tx)).is_err() {
            // Worker is gone; nothing to flush.
            return Ok(());
        }
        // `recv` returns Err when the sender is dropped without sending —
        // that happens on worker shutdown. Treat it as a successful flush:
        // there was nothing left to flush.
        done_rx.recv().unwrap_or(Ok(()))
    }

    /// The most recent deferred-write failure, if the last write to that path
    /// has not since succeeded. A health signal so a failure that flushed via
    /// the debounce timer (not an explicit [`flush`](Self::flush)) is still
    /// observable (#414).
    pub(crate) fn last_error(&self) -> Option<String> {
        self.inner
            .last_error
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|(_, msg)| msg.clone()))
    }

    /// Load a session by name, round-tripped through the worker so the read
    /// sees the latest pending write (the worker drains before reading) while
    /// this call stays synchronous. Returns `Ok(None)` if the worker is gone.
    pub(crate) fn load(&self, name: &str) -> Result<Option<PersistedSession>, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        if self
            .inner
            .tx
            .send(WriterMsg::Load(name.to_string(), tx))
            .is_err()
        {
            return Ok(None);
        }
        rx.recv().unwrap_or(Ok(None))
    }

    /// List session metadata, round-tripped through the worker (drains pending
    /// first). Returns an empty list if the worker is gone.
    pub(crate) fn list(&self) -> Result<Vec<SessionMetadata>, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        if self.inner.tx.send(WriterMsg::List(tx)).is_err() {
            return Ok(Vec::new());
        }
        rx.recv().unwrap_or(Ok(Vec::new()))
    }

    /// Delete a session by name, round-tripped through the worker (drains
    /// pending first so an in-flight write cannot resurrect the file). A no-op
    /// if the worker is gone.
    pub(crate) fn delete(&self, name: &str) -> Result<(), String> {
        let (tx, rx) = std::sync::mpsc::channel();
        if self
            .inner
            .tx
            .send(WriterMsg::Delete(name.to_string(), tx))
            .is_err()
        {
            return Ok(());
        }
        rx.recv().unwrap_or(Ok(()))
    }
}

impl Drop for WriterInner {
    fn drop(&mut self) {
        // Dropping `tx` here is what wakes the worker out of its `recv`
        // loop after any pending flush completes.
        //
        // We can't move `tx` out, but `mpsc::UnboundedSender` doesn't
        // expose a `close()`. Instead: take the thread handle (the
        // sender drops naturally when `self` goes out of scope right
        // after this `drop` body returns). To avoid a deadlock in tests
        // that hold and instantly drop the writer, we *first* signal the
        // worker by simply allowing the sender to be dropped at the end
        // of this block — but the join must observe `tx` already gone.
        //
        // Workaround: replace `self.tx` with a fresh, never-used pair so
        // the live `tx` is dropped now.
        let (junk_tx, _junk_rx) = mpsc::unbounded_channel::<WriterMsg>();
        let live_tx = std::mem::replace(&mut self.tx, junk_tx);
        drop(live_tx);

        // Now join the worker thread, but with a small ceiling so we
        // don't wedge process shutdown on a stuck disk.
        let Some(handle) = self.thread.lock().ok().and_then(|mut g| g.take()) else {
            return;
        };
        // `std::thread::JoinHandle::join` has no timeout in std. Park
        // ourselves on a oneshot driven by a helper thread so we can cap
        // the wait. Allocate it inline; if the join completes first we
        // never wait on the oneshot.
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let joined = Arc::new(Mutex::new(false));
        let joined_for_thread = Arc::clone(&joined);
        let spawn_result = std::thread::Builder::new()
            .name("caliban-session-writer-joiner".into())
            .spawn(move || {
                let _ = handle.join();
                *joined_for_thread.lock().expect("joiner mutex poisoned") = true;
                let _ = done_tx.send(());
            });
        if spawn_result.is_ok() {
            // Wait up to DROP_DRAIN_TIMEOUT for the worker to finish.
            let _ = done_rx.recv_timeout(DROP_DRAIN_TIMEOUT);
            if !*joined.lock().expect("joiner mutex poisoned") {
                let timeout_ms = u64::try_from(DROP_DRAIN_TIMEOUT.as_millis()).unwrap_or(u64::MAX);
                tracing::warn!(
                    target: caliban_common::tracing_targets::TARGET_SESSIONS,
                    timeout_ms,
                    "session writer drain timed out; pending write may be lost",
                );
            }
        }
        // If spawning the joiner failed, fall through: the runtime is
        // already in distress; abandoning the join is the safest move.
    }
}

/// Body of the worker thread: own a current-thread tokio runtime and
/// drive the debounce state machine on it.
fn run_writer_thread(
    rx: mpsc::UnboundedReceiver<WriterMsg>,
    window: Duration,
    max_delay: Duration,
    last_error: &LastError,
    backend: Arc<dyn SessionBackend>,
) {
    // `current_thread` flavor is sufficient — this thread runs nothing
    // but the debouncer.
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!(
                target: caliban_common::tracing_targets::TARGET_SESSIONS,
                error = %e,
                "failed to build session writer runtime; writes will be lost",
            );
            return;
        }
    };
    rt.block_on(writer_loop(rx, window, max_delay, last_error, backend));
}

/// The debounce state machine.
///
/// Holds pending session snapshots keyed by session name — so back-to-back
/// writes targeting the *same* session collapse to one backend save
/// (the common case), while writes targeting *different* sessions
/// each get their own save (no silent data loss across sessions).
///
/// The debounce timer is shared across all names and is reset on every
/// new request, matching the spec's "true debounce" semantic ("waits
/// 250 ms; timer reset on each new request").
///
/// A second, non-resetting bound caps the total wait: `oldest_dirty` records
/// when `pending` first became non-empty after a drain, and the effective flush
/// deadline is `min(debounce_deadline, oldest_dirty + max_delay)`. Because a
/// saturating request stream could keep the `biased` `select!` from ever
/// reaching the timer branch, the max-delay bound is *also* checked inline after
/// each request — guaranteeing a flush at least every `max_delay` regardless of
/// incoming traffic (#414, P10).
///
/// Reads (`Load`/`List`/`Delete`) drain pending first so they observe the
/// latest state, then await the backend directly on this thread's runtime.
async fn writer_loop(
    mut rx: mpsc::UnboundedReceiver<WriterMsg>,
    window: Duration,
    max_delay: Duration,
    last_error: &LastError,
    backend: Arc<dyn SessionBackend>,
) {
    let mut pending: HashMap<String, PersistedSession> = HashMap::new();
    let mut deadline = tokio::time::Instant::now();
    let mut oldest_dirty: Option<tokio::time::Instant> = None;

    loop {
        if pending.is_empty() {
            oldest_dirty = None;
            // No work — block on the channel.
            match rx.recv().await {
                Some(WriterMsg::Persist(req)) => {
                    let now = tokio::time::Instant::now();
                    pending.insert(req.session.name.clone(), req.session);
                    deadline = now + window;
                    oldest_dirty = Some(now);
                }
                Some(WriterMsg::Flush(done)) => {
                    // Nothing to flush; signal success immediately.
                    let _ = done.send(Ok(()));
                }
                Some(WriterMsg::Load(name, done)) => {
                    let _ = drain_pending(&mut pending, last_error, &backend).await;
                    oldest_dirty = None;
                    let r = backend.load(&name).await.map_err(|e| e.to_string());
                    let _ = done.send(r);
                }
                Some(WriterMsg::List(done)) => {
                    let _ = drain_pending(&mut pending, last_error, &backend).await;
                    oldest_dirty = None;
                    let r = backend.list().await.map_err(|e| e.to_string());
                    let _ = done.send(r);
                }
                Some(WriterMsg::Delete(name, done)) => {
                    let _ = drain_pending(&mut pending, last_error, &backend).await;
                    oldest_dirty = None;
                    let r = backend.delete(&name).await.map_err(|e| e.to_string());
                    let _ = done.send(r);
                }
                None => {
                    // Channel closed — no work left, exit cleanly.
                    return;
                }
            }
        } else {
            // Hard ceiling: never wait past `oldest_dirty + max_delay`.
            let hard = oldest_dirty.map_or(deadline, |od| od + max_delay);
            let effective = deadline.min(hard);
            tokio::select! {
                biased;

                msg = rx.recv() => match msg {
                    Some(WriterMsg::Persist(req)) => {
                        // Same name -> overwrite buffered snapshot (latest
                        // wins). Different name -> coexists in the map.
                        // Reset the debounce timer but NOT oldest_dirty.
                        let now = tokio::time::Instant::now();
                        pending.insert(req.session.name.clone(), req.session);
                        deadline = now + window;
                        // A sustained stream can starve the timer branch under
                        // `biased`; enforce the max-delay bound inline.
                        if oldest_dirty.is_some_and(|od| now >= od + max_delay) {
                            let _ = drain_pending(&mut pending, last_error, &backend).await;
                            oldest_dirty = None;
                        }
                    }
                    Some(WriterMsg::Flush(done)) => {
                        let r = drain_pending(&mut pending, last_error, &backend).await;
                        oldest_dirty = None;
                        let _ = done.send(r);
                    }
                    Some(WriterMsg::Load(name, done)) => {
                        let _ = drain_pending(&mut pending, last_error, &backend).await;
                        oldest_dirty = None;
                        let r = backend.load(&name).await.map_err(|e| e.to_string());
                        let _ = done.send(r);
                    }
                    Some(WriterMsg::List(done)) => {
                        let _ = drain_pending(&mut pending, last_error, &backend).await;
                        oldest_dirty = None;
                        let r = backend.list().await.map_err(|e| e.to_string());
                        let _ = done.send(r);
                    }
                    Some(WriterMsg::Delete(name, done)) => {
                        let _ = drain_pending(&mut pending, last_error, &backend).await;
                        oldest_dirty = None;
                        let r = backend.delete(&name).await.map_err(|e| e.to_string());
                        let _ = done.send(r);
                    }
                    None => {
                        // Channel closed during pending — final drain
                        // before exit.
                        let _ = drain_pending(&mut pending, last_error, &backend).await;
                        return;
                    }
                },
                () = tokio::time::sleep_until(effective) => {
                    let _ = drain_pending(&mut pending, last_error, &backend).await;
                    oldest_dirty = None;
                }
            }
        }
    }
}

/// Drain all pending writes through the backend, returning the first failure
/// (if any) so a `Flush` can report it to its caller.
async fn drain_pending(
    pending: &mut HashMap<String, PersistedSession>,
    last_error: &LastError,
    backend: &Arc<dyn SessionBackend>,
) -> Result<(), String> {
    let mut first_err: Option<String> = None;
    let drained: Vec<PersistedSession> = pending.drain().map(|(_, v)| v).collect();
    for session in drained {
        if let Err(msg) = do_write(&session, last_error, backend).await {
            first_err.get_or_insert(msg);
        }
    }
    first_err.map_or(Ok(()), Err)
}

/// Save one buffered snapshot via the backend, updating the shared `last_error`
/// health slot: set it on failure, clear it when this session's write succeeds.
/// Returns the formatted error on failure.
async fn do_write(
    session: &PersistedSession,
    last_error: &LastError,
    backend: &Arc<dyn SessionBackend>,
) -> Result<(), String> {
    match backend.save(session).await {
        Ok(()) => {
            if let Ok(mut slot) = last_error.lock()
                && slot.as_ref().is_some_and(|(n, _)| n == &session.name)
            {
                *slot = None;
            }
            Ok(())
        }
        Err(e) => {
            let msg = e.to_string();
            tracing::warn!(
                target: caliban_common::tracing_targets::TARGET_SESSIONS,
                error = %msg,
                session = %session.name,
                "debounced session write failed",
            );
            if let Ok(mut slot) = last_error.lock() {
                *slot = Some((session.name.clone(), msg.clone()));
            }
            Err(msg)
        }
    }
}

impl std::fmt::Debug for DebouncedWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DebouncedWriter").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests live alongside the writer; integration tests at
    //! `tests/debounced.rs` exercise it end-to-end via `SessionStore`.

    use super::*;
    use crate::backend::SessionBackend;
    use crate::session::PersistedSession;
    use crate::store::SessionMetadata;
    use std::collections::HashMap as StdHashMap;
    use std::sync::Arc;

    /// A short test window so tests don't dawdle.
    const TEST_WINDOW: Duration = Duration::from_millis(40);

    #[derive(Default)]
    struct MemBackend {
        map: Mutex<StdHashMap<String, PersistedSession>>,
        fail: Mutex<bool>,
    }
    #[async_trait::async_trait]
    impl SessionBackend for MemBackend {
        async fn save(&self, session: &PersistedSession) -> Result<(), crate::error::Error> {
            if *self.fail.lock().unwrap() {
                return Err(crate::error::Error::Persist("boom".into()));
            }
            self.map
                .lock()
                .unwrap()
                .insert(session.name.clone(), session.clone());
            Ok(())
        }
        async fn load(&self, name: &str) -> Result<Option<PersistedSession>, crate::error::Error> {
            Ok(self.map.lock().unwrap().get(name).cloned())
        }
        async fn list(&self) -> Result<Vec<SessionMetadata>, crate::error::Error> {
            Ok(self
                .map
                .lock()
                .unwrap()
                .values()
                .map(SessionMetadata::from_session)
                .collect())
        }
        async fn delete(&self, name: &str) -> Result<(), crate::error::Error> {
            self.map.lock().unwrap().remove(name);
            Ok(())
        }
    }

    fn sess(name: &str) -> PersistedSession {
        PersistedSession::new(name, "anthropic", "m")
    }

    #[test]
    fn single_write_lands_after_flush() {
        let be = Arc::new(MemBackend::default());
        let w =
            DebouncedWriter::with_window(Arc::clone(&be) as Arc<dyn SessionBackend>, TEST_WINDOW);
        w.request(sess("a"));
        w.flush().unwrap();
        assert!(be.map.lock().unwrap().contains_key("a"));
    }

    #[test]
    fn writes_within_window_collapse_to_latest() {
        let be = Arc::new(MemBackend::default());
        let w = DebouncedWriter::with_window(
            Arc::clone(&be) as Arc<dyn SessionBackend>,
            Duration::from_millis(150),
        );
        let mut s1 = sess("a");
        s1.model = "v1".into();
        let mut s3 = sess("a");
        s3.model = "v3".into();
        w.request(s1);
        w.request(sess("a"));
        w.request(s3);
        assert!(be.map.lock().unwrap().is_empty());
        w.flush().unwrap();
        assert_eq!(be.map.lock().unwrap().get("a").unwrap().model, "v3");
    }

    #[test]
    fn drop_drains_pending() {
        let be = Arc::new(MemBackend::default());
        {
            let w = DebouncedWriter::with_window(
                Arc::clone(&be) as Arc<dyn SessionBackend>,
                Duration::from_mins(1),
            );
            w.request(sess("a"));
        }
        assert!(be.map.lock().unwrap().contains_key("a"));
    }

    #[test]
    fn flush_surfaces_backend_failure() {
        let be = Arc::new(MemBackend::default());
        *be.fail.lock().unwrap() = true;
        let w =
            DebouncedWriter::with_window(Arc::clone(&be) as Arc<dyn SessionBackend>, TEST_WINDOW);
        w.request(sess("a"));
        assert!(w.flush().is_err());
        assert!(w.last_error().is_some());
    }

    #[test]
    fn flush_from_inside_tokio_runtime_does_not_panic() {
        let be = Arc::new(MemBackend::default());
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let w = DebouncedWriter::with_window(
                Arc::clone(&be) as Arc<dyn SessionBackend>,
                Duration::from_mins(1),
            );
            w.request(sess("a"));
            w.flush().unwrap();
        });
        assert!(be.map.lock().unwrap().contains_key("a"));
    }

    #[test]
    fn read_roundtrip_through_worker() {
        let be = Arc::new(MemBackend::default());
        let w =
            DebouncedWriter::with_window(Arc::clone(&be) as Arc<dyn SessionBackend>, TEST_WINDOW);
        w.request(sess("a"));
        // load() flushes pending first, then reads through the backend.
        let got = w.load("a").unwrap();
        assert!(got.is_some());
        assert_eq!(w.list().unwrap().len(), 1);
        w.delete("a").unwrap();
        assert!(w.load("a").unwrap().is_none());
    }
}
