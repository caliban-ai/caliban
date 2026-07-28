//! Backend-agnostic conformance battery. Any `SessionBackend` must pass it.
//!
//! First invoked by the fs backend in Task 2 and the gonzalo backend in Task 4;
//! the `dead_code` allows drop away once those callers land.
#![cfg(test)]
#![allow(dead_code)]
use chrono::Duration;

use crate::backend::SessionBackend;
use crate::session::PersistedSession;

fn s(name: &str) -> PersistedSession {
    PersistedSession::new(name, "anthropic", "claude-test")
}

/// Exercise the full CRUD contract. Callers pass a fresh, empty backend.
pub(crate) async fn run_session_backend_conformance<B: SessionBackend>(be: &B) {
    // empty start
    assert!(be.list().await.unwrap().is_empty());
    assert!(be.load("missing").await.unwrap().is_none());

    // save + load roundtrip
    be.save(&s("alpha")).await.unwrap();
    be.save(&s("beta")).await.unwrap();
    let loaded = be.load("alpha").await.unwrap().expect("alpha present");
    assert_eq!(loaded.name, "alpha");

    // list returns both
    let mut names: Vec<_> = be
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|m| m.name)
        .collect();
    names.sort();
    assert_eq!(names, vec!["alpha".to_string(), "beta".to_string()]);

    // overwrite same name does not duplicate
    be.save(&s("alpha")).await.unwrap();
    assert_eq!(be.list().await.unwrap().len(), 2);

    // delete removes + is idempotent
    be.delete("alpha").await.unwrap();
    assert_eq!(be.list().await.unwrap().len(), 1);
    be.delete("alpha").await.unwrap();
    assert!(be.load("alpha").await.unwrap().is_none());

    // list() orders by updated_at descending: save an older session, then a
    // strictly newer one, and confirm the newer one sorts first.
    let mut older = s("chrono-older");
    older.updated_at = chrono::Utc::now() - Duration::hours(1);
    be.save(&older).await.unwrap();
    let mut newer = s("chrono-newer");
    newer.updated_at = chrono::Utc::now();
    be.save(&newer).await.unwrap();
    let ordered = be.list().await.unwrap();
    let older_idx = ordered
        .iter()
        .position(|m| m.name == "chrono-older")
        .expect("older session listed");
    let newer_idx = ordered
        .iter()
        .position(|m| m.name == "chrono-newer")
        .expect("newer session listed");
    assert!(
        newer_idx < older_idx,
        "expected newer session to sort before older session"
    );
}
