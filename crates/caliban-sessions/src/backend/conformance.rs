//! Backend-agnostic conformance battery. Any `SessionBackend` must pass it.
//!
//! First invoked by the fs backend in Task 2 and the gonzalo backend in Task 4;
//! the `dead_code` allows drop away once those callers land.
#![cfg(test)]
#![allow(dead_code)]
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
}
