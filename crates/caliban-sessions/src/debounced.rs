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
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;

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

/// A single persist request: write `bytes` atomically to `path`.
///
/// The struct is owned by the writer task; the public API only exposes
/// `request` / `flush` / `flush_with_timeout`.
struct PersistRequest {
    path: PathBuf,
    bytes: Vec<u8>,
}

/// Control messages multiplexed onto the same channel.
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
/// it) and the handle (which exposes it). `Some((path, message))` means the
/// last write to `path` failed and no later write to it has succeeded; `None`
/// means the last observed write succeeded. Lets a failed deferred persist be
/// observed even when it flushed via the timer, not an explicit `flush` (#414).
type LastError = Arc<Mutex<Option<(PathBuf, String)>>>;

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
    /// Spawn the writer task on a dedicated OS thread.
    pub(crate) fn new() -> Self {
        Self::with_window_and_max_delay(DEBOUNCE_WINDOW, MAX_DELAY)
    }

    /// Like [`DebouncedWriter::new`] but lets tests dial the debounce window
    /// (max-delay bound scaled to the default ceiling).
    #[cfg(test)]
    pub(crate) fn with_window(window: Duration) -> Self {
        Self::with_window_and_max_delay(window, MAX_DELAY)
    }

    /// Like [`DebouncedWriter::new`] but lets tests dial both the debounce
    /// window and the max-delay ceiling.
    pub(crate) fn with_window_and_max_delay(window: Duration, max_delay: Duration) -> Self {
        let (tx, rx) = mpsc::unbounded_channel::<WriterMsg>();
        let last_error: LastError = Arc::new(Mutex::new(None));
        let last_error_worker = Arc::clone(&last_error);
        let thread = std::thread::Builder::new()
            .name("caliban-session-writer".into())
            .spawn(move || run_writer_thread(rx, window, max_delay, &last_error_worker))
            .expect("spawn session writer thread");
        Self {
            inner: Arc::new(WriterInner {
                tx,
                last_error,
                thread: Mutex::new(Some(thread)),
            }),
        }
    }

    /// Enqueue a persist request. Returns immediately — the actual disk
    /// write happens after the debounce window elapses, or sooner via
    /// [`DebouncedWriter::flush`] / shutdown.
    pub(crate) fn request(&self, path: PathBuf, bytes: Vec<u8>) {
        // Send failure means the worker thread has gone away (only
        // possible during shutdown). Drop the request rather than panic.
        let _ = self
            .inner
            .tx
            .send(WriterMsg::Persist(PersistRequest { path, bytes }));
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
    rt.block_on(writer_loop(rx, window, max_delay, last_error));
}

/// The debounce state machine.
///
/// Holds pending bytes keyed by destination path — so back-to-back
/// writes targeting the *same* session collapse to one disk write
/// (the common case), while writes targeting *different* sessions
/// each get their own write (no silent data loss across sessions).
///
/// The debounce timer is shared across all paths and is reset on every
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
async fn writer_loop(
    mut rx: mpsc::UnboundedReceiver<WriterMsg>,
    window: Duration,
    max_delay: Duration,
    last_error: &LastError,
) {
    let mut pending: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    let mut deadline = tokio::time::Instant::now();
    let mut oldest_dirty: Option<tokio::time::Instant> = None;

    loop {
        if pending.is_empty() {
            oldest_dirty = None;
            // No work — block on the channel.
            match rx.recv().await {
                Some(WriterMsg::Persist(req)) => {
                    let now = tokio::time::Instant::now();
                    pending.insert(req.path, req.bytes);
                    deadline = now + window;
                    oldest_dirty = Some(now);
                }
                Some(WriterMsg::Flush(done)) => {
                    // Nothing to flush; signal success immediately.
                    let _ = done.send(Ok(()));
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
                        // Same path -> overwrite buffered bytes (latest
                        // wins). Different path -> coexists in the map.
                        // Reset the debounce timer but NOT oldest_dirty.
                        let now = tokio::time::Instant::now();
                        pending.insert(req.path, req.bytes);
                        deadline = now + window;
                        // A sustained stream can starve the timer branch under
                        // `biased`; enforce the max-delay bound inline.
                        if oldest_dirty.is_some_and(|od| now >= od + max_delay) {
                            let _ = drain_pending(&mut pending, last_error);
                            oldest_dirty = None;
                        }
                    }
                    Some(WriterMsg::Flush(done)) => {
                        let r = drain_pending(&mut pending, last_error);
                        oldest_dirty = None;
                        let _ = done.send(r);
                    }
                    None => {
                        // Channel closed during pending — final drain
                        // before exit.
                        let _ = drain_pending(&mut pending, last_error);
                        return;
                    }
                },
                () = tokio::time::sleep_until(effective) => {
                    let _ = drain_pending(&mut pending, last_error);
                    oldest_dirty = None;
                }
            }
        }
    }
}

/// Drain all pending writes, returning the first failure (if any) so a `Flush`
/// can report it to its caller.
fn drain_pending(
    pending: &mut HashMap<PathBuf, Vec<u8>>,
    last_error: &LastError,
) -> Result<(), String> {
    let mut first_err: Option<String> = None;
    for (path, bytes) in pending.drain() {
        if let Err(msg) = do_write(&path, &bytes, last_error) {
            first_err.get_or_insert(msg);
        }
    }
    first_err.map_or(Ok(()), Err)
}

/// Write one buffered snapshot, updating the shared `last_error` health slot:
/// set it on failure, clear it when this path's write succeeds. Returns the
/// formatted error on failure.
fn do_write(path: &std::path::Path, bytes: &[u8], last_error: &LastError) -> Result<(), String> {
    match caliban_common::fs::write_atomic(path, bytes) {
        Ok(()) => {
            if let Ok(mut slot) = last_error.lock()
                && slot.as_ref().is_some_and(|(p, _)| p == path)
            {
                *slot = None;
            }
            Ok(())
        }
        Err(e) => {
            let msg = e.to_string();
            tracing::warn!(
                target: caliban_common::tracing_targets::TARGET_SESSIONS,
                error = %e,
                path = %path.display(),
                "debounced session write failed",
            );
            if let Ok(mut slot) = last_error.lock() {
                *slot = Some((path.to_path_buf(), msg.clone()));
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
    use tempfile::TempDir;

    /// A short test window so tests don't dawdle.
    const TEST_WINDOW: Duration = Duration::from_millis(40);

    fn count_files(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir).map_or(0, |it| it.filter_map(Result::ok).count())
    }

    #[test]
    fn single_write_lands_after_window() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("a.json");
        let w = DebouncedWriter::with_window(TEST_WINDOW);
        w.request(p.clone(), b"hello".to_vec());
        // Flush ensures the request is on disk regardless of timer race.
        w.flush().unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"hello");
    }

    #[test]
    fn multiple_writes_within_window_collapse_to_latest() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("a.json");
        // Wider window so we can stack three writes inside it.
        let w = DebouncedWriter::with_window(Duration::from_millis(150));
        w.request(p.clone(), b"v1".to_vec());
        w.request(p.clone(), b"v2".to_vec());
        w.request(p.clone(), b"v3".to_vec());
        // Before the window elapses + before flush, the file must not
        // yet exist (the worker hasn't written anything).
        assert!(!p.exists());
        w.flush().unwrap();
        // Exactly one disk write, with the latest bytes.
        assert_eq!(std::fs::read(&p).unwrap(), b"v3");
    }

    #[test]
    fn window_expiry_flushes_without_explicit_flush() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("a.json");
        let w = DebouncedWriter::with_window(Duration::from_millis(30));
        w.request(p.clone(), b"timer-flush".to_vec());
        // Wait long enough for the debounce timer to fire on its own.
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(std::fs::read(&p).unwrap(), b"timer-flush");
    }

    #[test]
    fn flush_is_synchronous() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("a.json");
        // Long window so the timer can't possibly fire before flush.
        let w = DebouncedWriter::with_window(Duration::from_mins(1));
        w.request(p.clone(), b"sync".to_vec());
        // Right after `flush()` returns, the file must be on disk.
        w.flush().unwrap();
        assert!(p.exists(), "flush returned before file landed");
        assert_eq!(std::fs::read(&p).unwrap(), b"sync");
    }

    #[test]
    fn drop_drains_pending_request() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("a.json");
        {
            // Long window — drop alone must drain.
            let w = DebouncedWriter::with_window(Duration::from_mins(1));
            w.request(p.clone(), b"drop-drain".to_vec());
            // Going out of scope here triggers `Drop`.
        }
        assert!(p.exists(), "drop did not drain pending request");
        assert_eq!(std::fs::read(&p).unwrap(), b"drop-drain");
    }

    #[test]
    fn flush_from_inside_tokio_runtime_does_not_panic() {
        // Regression: a previous revision used `tokio::sync::oneshot`
        // for the flush done-signal, whose `blocking_recv` panics when
        // called inside a tokio runtime context. `flush()` is called from
        // `SessionStore::load/list/delete/flush`, all of which run under
        // `#[tokio::main]` during normal binary startup.
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("a.json");
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let w = DebouncedWriter::with_window(Duration::from_mins(1));
            w.request(p.clone(), b"from-runtime".to_vec());
            w.flush().unwrap();
        });
        assert_eq!(std::fs::read(&p).unwrap(), b"from-runtime");
    }

    #[test]
    fn atomic_write_leaves_no_temp_file() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("session.json");
        let w = DebouncedWriter::with_window(TEST_WINDOW);
        w.request(p.clone(), b"x".to_vec());
        w.flush().unwrap();
        // Directory should contain only the final file — no `.tmp*`
        // siblings left behind by tempfile::NamedTempFile.
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name())
            .collect();
        assert_eq!(entries, vec![p.file_name().unwrap().to_owned()]);
        assert_eq!(count_files(dir.path()), 1);
    }

    #[test]
    fn flush_surfaces_write_failure() {
        // #414 P8: a failing persist must be observable, not a silent Ok.
        // Make the destination's parent a *file* so `create_dir_all` (and thus
        // the atomic write) fails deterministically.
        let dir = TempDir::new().unwrap();
        let blocker = dir.path().join("sub");
        std::fs::write(&blocker, b"i am a file, not a dir").unwrap();
        let target = blocker.join("session.json"); // parent `sub` is a file
        let w = DebouncedWriter::with_window(TEST_WINDOW);
        w.request(target.clone(), b"data".to_vec());

        let flush_result = w.flush();
        assert!(
            flush_result.is_err(),
            "flush returned Ok despite a failed write: {flush_result:?}"
        );
        // And it stays observable as a health signal after the fact.
        assert!(
            w.last_error().is_some(),
            "failure not recorded in health slot"
        );
    }

    #[test]
    fn last_error_clears_after_a_successful_write() {
        let dir = TempDir::new().unwrap();
        // First: force a failure to a path whose parent is a file.
        let blocker = dir.path().join("blocked");
        std::fs::write(&blocker, b"file").unwrap();
        let bad = blocker.join("x.json");
        let w = DebouncedWriter::with_window(TEST_WINDOW);
        w.request(bad.clone(), b"a".to_vec());
        let _ = w.flush();
        assert!(w.last_error().is_some());
        // Then a successful write to a *different* healthy path leaves the bad
        // path's failure recorded (per-path health), but a later success to the
        // same bad path (after we remove the blocker) clears it.
        std::fs::remove_file(&blocker).unwrap();
        w.request(bad.clone(), b"b".to_vec());
        w.flush().unwrap();
        assert!(
            w.last_error().is_none(),
            "health slot not cleared on success"
        );
    }

    #[test]
    fn sustained_writes_flush_within_max_delay() {
        // #414 P10: with a long debounce window, a continuous request stream
        // must still be forced to disk by the max-delay bound rather than
        // starving the flush indefinitely.
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("hot.json");
        // Window far longer than the test; only the max-delay bound can flush.
        let w = DebouncedWriter::with_window_and_max_delay(
            Duration::from_secs(30),
            Duration::from_millis(80),
        );
        // Stream requests with sub-window gaps for well over max_delay, never
        // calling flush(). If the bound works, the file appears mid-stream.
        let mut landed = false;
        for i in 0..40 {
            w.request(p.clone(), format!("v{i}").into_bytes());
            std::thread::sleep(Duration::from_millis(10));
            if p.exists() {
                landed = true;
                break;
            }
        }
        assert!(
            landed,
            "sustained writes never flushed within the max-delay bound",
        );
    }
}
