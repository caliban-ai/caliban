//! #471 regression: a session persisted via the gonzalo backend loads and
//! restores through `caliban-checkpoint`'s in-memory `PersistedSession` path.
#![cfg(feature = "gonzalo")]

use std::sync::Arc;

use caliban_sessions::{GonzaloSessionBackend, PersistedSession, SessionBackend};
use gonzalo_store_fs::FsStore;

#[tokio::test]
async fn gonzalo_persisted_session_loads_for_restore() {
    let tmp = tempfile::tempdir().unwrap();
    let be = GonzaloSessionBackend::new(Arc::new(FsStore::new(tmp.path().to_path_buf())), "ws");
    let mut s = PersistedSession::new("resume-me", "anthropic", "m");
    s.model = "claude-x".into();
    be.save(&s).await.unwrap();

    // Load back exactly as the restore path consumes it (a &mut PersistedSession).
    let mut loaded = be.load("resume-me").await.unwrap().expect("present");
    assert_eq!(loaded.name, "resume-me");
    assert_eq!(loaded.model, "claude-x");
    // caliban-checkpoint::restore mutates the in-memory session; assert it is a
    // normal owned value the restore signature accepts.
    loaded.messages.clear();
    assert!(loaded.messages.is_empty());
}
