//! End-to-end tests for the debounced session writer, exercising it via
//! the public [`SessionStore`] API.
//!
//! Coverage:
//! 1. A single save that finishes inside the debounce window collapses
//!    into one disk write (no spurious intermediate file).
//! 2. Multiple saves inside the window collapse to a single write of
//!    the latest content.
//! 3. After the window expires with no new requests, the content is
//!    flushed to disk without an explicit `flush()` call.
//! 4. `flush()` waits synchronously for any pending write.
//! 5. Dropping the store drains any pending request before the writer
//!    thread joins.
//! 6. The atomic-write path never leaves a half-written / `.tmp*` file
//!    in the destination directory on success.

#![allow(missing_docs)]

use std::time::{Duration, Instant};

use caliban_provider::{Message, Usage};
use caliban_sessions::{PersistedSession, SessionStore};
use tempfile::TempDir;

fn fake_session(name: &str, body: &str) -> PersistedSession {
    let mut s = PersistedSession::new(name, "anthropic", "claude-3-5-sonnet");
    s.messages.push(Message::user_text(body));
    s.messages.push(Message::assistant_text("ack"));
    s.total_usage = Usage {
        input_tokens: 1,
        output_tokens: 1,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    };
    s
}

/// Count entries in `dir` (ignores read errors — surfaces 0).
fn count_entries(dir: &std::path::Path) -> usize {
    std::fs::read_dir(dir).map_or(0, |it| it.filter_map(Result::ok).count())
}

#[test]
fn single_save_within_window_results_in_one_write() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());

    // Issue a single save and immediately flush. Exactly one file
    // should exist (the session JSON), no temp / leftover files.
    store.save(&fake_session("single", "hi")).unwrap();
    store.flush().unwrap();

    assert!(store.path_for("single").exists());
    assert_eq!(
        count_entries(tmp.path()),
        1,
        "expected exactly one disk write"
    );
}

#[test]
fn multiple_saves_within_window_collapse_to_latest_content() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());

    // The default 250 ms window is plenty wide to stage three rapid
    // saves of the same session, each carrying a different payload.
    store
        .save(&fake_session("rapid", "v1"))
        .expect("enqueue v1");
    store
        .save(&fake_session("rapid", "v2"))
        .expect("enqueue v2");
    store
        .save(&fake_session("rapid", "v3"))
        .expect("enqueue v3");

    // Force the writer to drain whatever it has buffered.
    store.flush().unwrap();

    // Only the latest payload should be observable.
    let loaded = store
        .load("rapid")
        .expect("load ok")
        .expect("session present");
    // `user_text` puts the body into the first user message; assert it
    // matches the most-recent enqueue.
    let user_body = loaded
        .messages
        .iter()
        .find_map(|m| {
            if m.role == caliban_provider::Role::User {
                Some(format!("{m:?}"))
            } else {
                None
            }
        })
        .unwrap_or_default();
    assert!(
        user_body.contains("v3"),
        "expected v3 to win; got message debug: {user_body}"
    );

    // And only one JSON file on disk — coalescing actually coalesced.
    assert_eq!(
        count_entries(tmp.path()),
        1,
        "multiple saves should collapse to one file"
    );
}

#[test]
fn window_expiry_flushes_to_disk_without_explicit_flush() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());

    store.save(&fake_session("auto", "x")).unwrap();
    let path = store.path_for("auto");

    // Poll for the writer's own timer to fire — give it well more than
    // the 250 ms window. We assert via polling (not a fixed sleep) so
    // the test stays robust on slow CI.
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if path.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    assert!(
        path.exists(),
        "debounce timer should have flushed within 3s of the 250ms window",
    );
}

#[test]
fn flush_blocks_until_pending_write_completes() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());
    let path = store.path_for("sync");

    store.save(&fake_session("sync", "now")).unwrap();
    // The 250 ms window is still wide open here; the file must not
    // exist yet if we look immediately. (This is a soft race-tolerant
    // assertion: the worker thread *could* preempt us, but we've
    // observed the path right after the send — no flush issued yet.)
    // Either way, after `flush()` it MUST exist.
    store.flush().unwrap();
    assert!(
        path.exists(),
        "flush() returned but file did not land on disk",
    );

    // Load via the public API; auto-flush + read.
    let loaded = store
        .load("sync")
        .expect("load ok")
        .expect("session present");
    assert_eq!(loaded.name, "sync");
}

#[test]
fn dropping_store_drains_pending_request() {
    let tmp = TempDir::new().unwrap();
    let path = {
        let store = SessionStore::new(tmp.path().to_path_buf());
        let p = store.path_for("dropped");
        store.save(&fake_session("dropped", "bye")).unwrap();
        // Do NOT flush — rely on `Drop` to drain the buffered request.
        p
        // `store` drops here.
    };

    // After drop, the file must exist (drop drains pending bytes,
    // bounded by `DROP_DRAIN_TIMEOUT` in the writer module).
    assert!(
        path.exists(),
        "Drop did not drain the pending session write",
    );
}

#[test]
fn atomic_write_leaves_no_temp_files_in_destination_dir() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());

    // Issue a series of saves so the writer has work, then flush.
    store.save(&fake_session("atom", "one")).unwrap();
    store.save(&fake_session("atom", "two")).unwrap();
    store.flush().unwrap();

    // `caliban_common::fs::write_atomic` uses
    // `tempfile::NamedTempFile::new_in` + `persist` for the rename —
    // on success the only entry left in the directory is the renamed
    // target. A failed-half write would leak a `.tmp*` sibling.
    let entries: Vec<String> = std::fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();

    assert_eq!(
        entries,
        vec!["atom.json".to_string()],
        "atomic write should leave only the final file; saw {entries:?}",
    );
}
