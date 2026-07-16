//! Backend-agnostic conformance battery. Any `TopicBackend` must pass it.
#![cfg(test)]
use crate::auto::{TopicDraft, TopicKind};
use crate::backend::TopicBackend;

fn d(name: &str, desc: &str) -> TopicDraft {
    TopicDraft {
        name: name.into(),
        description: desc.into(),
        kind: TopicKind::User,
        body: "x".into(),
    }
}

/// Exercise the full contract. Callers construct a fresh, empty backend.
pub(crate) async fn run_topic_backend_conformance<B: TopicBackend>(be: &B) {
    // empty start
    assert!(be.list().await.unwrap().is_empty());
    assert_eq!(be.index().await.unwrap().trim(), "# Memory index");

    // write + list + read
    be.write(&d("alpha", "line one\nline two")).await.unwrap();
    be.write(&d("beta", "bee")).await.unwrap();
    let names = {
        let mut n: Vec<_> = be
            .list()
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect();
        n.sort();
        n
    };
    assert_eq!(names, vec!["alpha".to_string(), "beta".to_string()]);
    assert_eq!(be.read("alpha").await.unwrap().name, "alpha");

    // index derives one row per topic, first desc line only
    let idx = be.index().await.unwrap();
    assert!(idx.contains("[alpha](alpha.md)"));
    assert!(idx.contains("line one"));
    assert!(!idx.contains("line two"));

    // update in place (same slug) does not duplicate
    be.write(&d("alpha", "updated")).await.unwrap();
    assert_eq!(be.list().await.unwrap().len(), 2);
    assert!(be.index().await.unwrap().contains("updated"));

    // invalid slug (path traversal) rejected — matches the documented contract
    assert!(matches!(
        be.write(&d("../escape", "x")).await,
        Err(crate::error::MemoryError::InvalidSlug { .. })
    ));

    // delete removes + idempotent
    be.delete("alpha").await.unwrap();
    assert_eq!(be.list().await.unwrap().len(), 1);
    be.delete("alpha").await.unwrap();
    assert!(!be.index().await.unwrap().contains("[alpha]"));
}
