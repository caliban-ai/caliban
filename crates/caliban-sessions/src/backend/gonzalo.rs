//! gonzalo-facade session store. Feature-gated; the vanilla build never sees it.
#![cfg(feature = "gonzalo")]
use std::sync::Arc;

use async_trait::async_trait;
use gonzalo_core::{
    Body, DeleteResult, Identity, KeyPrefix, Meta, PutResult, Record, RecordKey, RecordKind,
    Revision, Store,
};

use crate::backend::SessionBackend;
use crate::error::{Error, Result};
use crate::session::PersistedSession;
use crate::store::SessionMetadata;

const NAMESPACE: &str = "caliban";

/// gonzalo-backed session store. Sessions are `Record`s keyed
/// `caliban / sessions:<workspace-slug> / <name>`, bodies opaque `PersistedSession` JSON.
pub struct GonzaloSessionBackend {
    pub(crate) store: Arc<dyn Store>,
    collection: String,
    author: Identity,
}

impl GonzaloSessionBackend {
    /// Construct a backend writing sessions under `caliban / sessions:<slug> / *`.
    #[must_use]
    pub fn new(store: Arc<dyn Store>, workspace_slug: impl Into<String>) -> Self {
        Self {
            store,
            collection: format!("sessions:{}", workspace_slug.into()),
            author: resolve_author(),
        }
    }

    pub(crate) fn key(&self, name: &str) -> RecordKey {
        RecordKey::new(NAMESPACE, self.collection.clone(), name)
    }

    fn prefix(&self) -> KeyPrefix {
        KeyPrefix {
            namespace: Some(NAMESPACE.into()),
            collection: Some(self.collection.clone()),
        }
    }

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

    /// Serialize a session into a fresh `Record` (no OCC lineage — used by tests
    /// and as the base for `save`'s get→put).
    pub(crate) fn session_to_record(&self, session: &PersistedSession) -> Result<Record> {
        let json = serde_json::to_vec(session)?;
        Ok(Record {
            key: self.key(&session.name),
            kind: RecordKind::Session,
            revision: Revision::initial(&json),
            parent: None,
            body: Body::Inline(json),
            meta: self.meta_now(),
            links: Vec::new(),
        })
    }
}

/// Resolve the record author: git identity if detectable, else "caliban".
fn resolve_author() -> Identity {
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

fn record_to_session(rec: &Record) -> Result<PersistedSession> {
    let bytes = match &rec.body {
        Body::Inline(b) => b.as_slice(),
        Body::Blob { .. } => {
            return Err(Error::Persist("unexpected blob body for session".into()));
        }
    };
    Ok(serde_json::from_slice(bytes)?)
}

#[async_trait]
impl SessionBackend for GonzaloSessionBackend {
    async fn save(&self, session: &PersistedSession) -> Result<()> {
        crate::backend::fs::validate_name(&session.name)?;
        let key = self.key(&session.name);
        let existing = self
            .store
            .get(&key)
            .await
            .map_err(|e| Error::Persist(e.to_string()))?;
        let mut record = self.session_to_record(session)?;
        let expected = existing.as_ref().map(|r| r.revision.clone());
        if let Some(prev) = existing {
            record.parent = Some(prev.revision.clone());
            record.revision = prev.revision.next(record.body.bytes());
            record.meta.created = prev.meta.created;
        }
        match self
            .store
            .put(record, expected)
            .await
            .map_err(|e| Error::Persist(e.to_string()))?
        {
            PutResult::Committed(_) => Ok(()),
            PutResult::Conflict(_) => Err(Error::Conflict {
                name: session.name.clone(),
            }),
        }
    }

    async fn load(&self, name: &str) -> Result<Option<PersistedSession>> {
        crate::backend::fs::validate_name(name)?;
        match self
            .store
            .get(&self.key(name))
            .await
            .map_err(|e| Error::Persist(e.to_string()))?
        {
            Some(rec) => Ok(Some(record_to_session(&rec)?)),
            None => Ok(None),
        }
    }

    async fn list(&self) -> Result<Vec<SessionMetadata>> {
        let keys = self
            .store
            .list(&self.prefix())
            .await
            .map_err(|e| Error::Persist(e.to_string()))?;
        let mut out = Vec::with_capacity(keys.len());
        for key in keys {
            if let Some(rec) = self
                .store
                .get(&key)
                .await
                .map_err(|e| Error::Persist(e.to_string()))?
            {
                match record_to_session(&rec) {
                    Ok(s) => out.push(SessionMetadata::from_session(&s)),
                    Err(e) => {
                        tracing::warn!(key = %key, error = %e, "skipping unparseable session record");
                    }
                }
            }
        }
        out.sort_by_key(|b| std::cmp::Reverse(b.updated_at));
        Ok(out)
    }

    async fn delete(&self, name: &str) -> Result<()> {
        crate::backend::fs::validate_name(name)?;
        match self
            .store
            .delete(&self.key(name), None)
            .await
            .map_err(|e| Error::Persist(e.to_string()))?
        {
            DeleteResult::Deleted => Ok(()),
            DeleteResult::Conflict(_) => Err(Error::Conflict { name: name.into() }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::conformance::run_session_backend_conformance;
    use gonzalo_store_fs::FsStore;

    fn be(tmp: &std::path::Path) -> GonzaloSessionBackend {
        GonzaloSessionBackend::new(Arc::new(FsStore::new(tmp.to_path_buf())), "wsslug")
    }

    #[tokio::test]
    async fn gonzalo_backend_passes_conformance() {
        let tmp = tempfile::tempdir().unwrap();
        run_session_backend_conformance(&be(tmp.path())).await;
    }

    #[tokio::test]
    async fn save_then_load_roundtrips() {
        let tmp = tempfile::tempdir().unwrap();
        let g = be(tmp.path());
        let mut s = PersistedSession::new("alpha", "anthropic", "m");
        s.model = "claude-x".into();
        g.save(&s).await.unwrap();
        let got = g.load("alpha").await.unwrap().unwrap();
        assert_eq!(got.name, "alpha");
        assert_eq!(got.model, "claude-x");
    }

    #[tokio::test]
    async fn stale_write_maps_to_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        let g = be(tmp.path());
        g.save(&PersistedSession::new("z", "anthropic", "m"))
            .await
            .unwrap();
        // Drive a raw conflict: put expected=None on an existing key.
        let rec = g
            .session_to_record(&PersistedSession::new("z", "anthropic", "m"))
            .unwrap();
        let r = g.store.put(rec, None).await.unwrap();
        assert!(matches!(r, gonzalo_core::PutResult::Conflict(_)));
    }
}
