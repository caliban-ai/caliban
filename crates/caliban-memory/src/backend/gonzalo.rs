//! gonzalo-facade topic store. Feature-gated; the vanilla build never sees it.
#![cfg(feature = "gonzalo")]
use std::sync::Arc;

use async_trait::async_trait;
use gonzalo_core::{
    Body, Identity, KeyPrefix, Meta, PutResult, Record, RecordKey, RecordKind, Revision, Store,
};
use serde::{Deserialize, Serialize};

use crate::auto::{TopicDraft, TopicFile, TopicKind, TopicSummary, validate_slug};
use crate::backend::TopicBackend;
use crate::error::{MemoryError, Result};

const NAMESPACE: &str = "caliban";

/// The opaque JSON envelope stored in `Body::Inline`. Lossless vs today's `.md`.
#[derive(Serialize, Deserialize)]
struct StoredTopic {
    name: String,
    description: String,
    kind: String, // TopicKind::as_str()
    body: String,
}

/// gonzalo-backed topic store. Topics are `Record`s keyed
/// `caliban / memory:<workspace-slug> / <slug>`, bodies opaque JSON.
pub struct GonzaloTopicBackend {
    store: Arc<dyn Store>,
    collection: String,
    author: Identity,
}

impl GonzaloTopicBackend {
    /// Construct a backend writing topics under `caliban / memory:<workspace_slug> / *`.
    /// Resolves the write author (git identity, else `"caliban"`) once, here.
    #[must_use]
    pub fn new(store: Arc<dyn Store>, workspace_slug: impl Into<String>) -> Self {
        Self {
            store,
            collection: format!("memory:{}", workspace_slug.into()),
            author: resolve_author(),
        }
    }

    fn key(&self, slug: &str) -> RecordKey {
        RecordKey::new(NAMESPACE, self.collection.clone(), slug)
    }

    #[allow(dead_code)] // consumed by Task 6's list/index
    fn prefix(&self) -> KeyPrefix {
        KeyPrefix {
            namespace: Some(NAMESPACE.into()),
            collection: Some(self.collection.clone()),
        }
    }
}

/// Resolve the record author: git identity if detectable, else "caliban".
/// Resolved once at construction — never on the hot path.
pub(crate) fn resolve_author() -> Identity {
    for field in ["user.email", "user.name"] {
        if let Ok(out) = std::process::Command::new("git")
            .args(["config", field])
            .output()
            && out.status.success()
        {
            let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !v.is_empty() {
                return Identity::new(v);
            }
        }
    }
    Identity::new("caliban")
}

fn now_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Build a topic `Record` with caller-supplied `Meta` (reused by #474's migrator
/// with mtimes; #470's `write` passes `now()`). Single source of the topic↔Record map.
pub(crate) fn topic_to_record(key: RecordKey, draft: &TopicDraft, meta: Meta) -> Result<Record> {
    let stored = StoredTopic {
        name: draft.name.clone(),
        description: draft.description.clone(),
        kind: draft.kind.as_str().to_string(),
        body: draft.body.clone(),
    };
    let json = serde_json::to_vec(&stored).map_err(|e| MemoryError::Backend(e.to_string()))?;
    Ok(Record {
        key,
        kind: RecordKind::Topic,
        revision: Revision::initial(&json),
        parent: None,
        body: Body::Inline(json),
        meta,
        links: Vec::new(),
    })
}

impl GonzaloTopicBackend {
    fn meta_now(&self) -> Meta {
        let ts = now_millis();
        Meta {
            author: self.author.clone(),
            origin_system: NAMESPACE.to_string(),
            created: ts,
            updated: ts,
            labels: std::collections::BTreeMap::new(),
        }
    }
}

#[async_trait]
impl TopicBackend for GonzaloTopicBackend {
    async fn write(&self, draft: &TopicDraft) -> Result<String> {
        validate_slug(&draft.name)?;
        let key = self.key(&draft.name);
        // OCC get→put: overwrite requires the current revision as `expected`.
        let existing = self
            .store
            .get(&key)
            .await
            .map_err(|e| MemoryError::Backend(e.to_string()))?;
        let mut record = topic_to_record(key.clone(), draft, self.meta_now())?;
        let expected = existing.as_ref().map(|r| r.revision.clone());
        if let Some(prev) = existing {
            record.parent = Some(prev.revision.clone());
            // Revision::next(&self, body) -> counter+1, rehash (verified gonzalo 0.3 API).
            record.revision = prev.revision.next(record.body.bytes());
        }
        match self
            .store
            .put(record, expected)
            .await
            .map_err(|e| MemoryError::Backend(e.to_string()))?
        {
            PutResult::Committed(_) => Ok(key.to_string()),
            PutResult::Conflict(_) => Err(MemoryError::Conflict {
                key: key.to_string(),
            }),
        }
    }

    async fn read(&self, name: &str) -> Result<TopicFile> {
        validate_slug(name)?;
        let key = self.key(name);
        let rec = self
            .store
            .get(&key)
            .await
            .map_err(|e| MemoryError::Backend(e.to_string()))?
            .ok_or_else(|| MemoryError::Backend(format!("no such topic: {name}")))?;
        stored_to_file(&rec)
    }

    async fn list(&self) -> Result<Vec<TopicSummary>> {
        unimplemented!("Task 6")
    }
    async fn delete(&self, _name: &str) -> Result<()> {
        unimplemented!("Task 6")
    }
    async fn index(&self) -> Result<String> {
        unimplemented!("Task 6")
    }
}

/// Parse the opaque body once; `TopicSummary`/`TopicFile` are FLAT and carry a
/// `path` — the store has no real path, so synthesize a relative `<slug>.md`.
fn parse_stored(rec: &Record) -> Result<(StoredTopic, TopicKind)> {
    let bytes = match &rec.body {
        Body::Inline(b) => b.as_slice(),
        Body::Blob { .. } => {
            return Err(MemoryError::Backend(
                "unexpected blob body for topic".into(),
            ));
        }
    };
    let s: StoredTopic =
        serde_json::from_slice(bytes).map_err(|e| MemoryError::Backend(e.to_string()))?;
    let kind = TopicKind::parse(&s.kind)
        .ok_or_else(|| MemoryError::Backend(format!("bad topic kind: {}", s.kind)))?;
    Ok((s, kind))
}

fn synthetic_path(slug: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("{slug}.md"))
}

#[allow(dead_code)] // consumed by Task 6's list
fn stored_to_summary(rec: &Record) -> Result<TopicSummary> {
    let (s, kind) = parse_stored(rec)?;
    Ok(TopicSummary {
        path: synthetic_path(&s.name),
        name: s.name,
        description: s.description,
        kind,
    })
}

fn stored_to_file(rec: &Record) -> Result<TopicFile> {
    let (s, kind) = parse_stored(rec)?;
    Ok(TopicFile {
        path: synthetic_path(&s.name),
        name: s.name,
        description: s.description,
        kind,
        body: s.body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gonzalo_store_fs::FsStore;

    fn be(tmp: &std::path::Path) -> GonzaloTopicBackend {
        GonzaloTopicBackend::new(Arc::new(FsStore::new(tmp.to_path_buf())), "wsslug")
    }

    #[tokio::test]
    async fn write_then_read_roundtrips() {
        let tmp = tempfile::tempdir().unwrap();
        let g = be(tmp.path());
        g.write(&TopicDraft {
            name: "alpha".into(),
            description: "desc".into(),
            kind: TopicKind::Project,
            body: "the body".into(),
        })
        .await
        .unwrap();
        let f = g.read("alpha").await.unwrap();
        assert_eq!(f.name, "alpha");
        assert_eq!(f.body, "the body");
        assert_eq!(f.kind, TopicKind::Project);
    }

    #[test]
    fn resolve_author_never_empty() {
        // Either a git identity or the "caliban" fallback — never empty.
        let id = resolve_author();
        assert!(!format!("{id:?}").is_empty());
    }
}
