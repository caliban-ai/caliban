//! The storage seam for auto-memory topics.
use async_trait::async_trait;

use crate::auto::{TopicDraft, TopicFile, TopicSummary};
use crate::error::Result;

/// Substrate-neutral CRUD + index projection for auto-memory topics.
#[async_trait]
pub trait TopicBackend: Send + Sync {
    /// List all topics.
    async fn list(&self) -> Result<Vec<TopicSummary>>;
    /// Read a topic by name.
    async fn read(&self, name: &str) -> Result<TopicFile>;
    /// Persist `draft`, returning a human-readable locator (fs path or record key).
    async fn write(&self, draft: &TopicDraft) -> Result<String>;
    /// Delete a topic by name.
    async fn delete(&self, name: &str) -> Result<()>;
    /// Rebuild the `MEMORY.md` index body from the current topic set.
    async fn index(&self) -> Result<String>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auto::{TopicDraft, TopicKind};
    use std::sync::Mutex;

    /// Minimal in-memory backend proving the trait is object-safe (`Box<dyn>`),
    /// async, and Result-plumbed. Task 2 adds the real FsTopicBackend + facade.
    #[derive(Default)]
    struct MockBackend {
        writes: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl TopicBackend for MockBackend {
        async fn list(&self) -> Result<Vec<TopicSummary>> { Ok(Vec::new()) }
        async fn read(&self, _name: &str) -> Result<TopicFile> {
            Err(crate::error::MemoryError::Backend("mock".into()))
        }
        async fn write(&self, draft: &TopicDraft) -> Result<String> {
            self.writes.lock().unwrap().push(draft.name.clone());
            Ok(format!("mock:{}", draft.name))
        }
        async fn delete(&self, _name: &str) -> Result<()> { Ok(()) }
        async fn index(&self) -> Result<String> { Ok(String::new()) }
    }

    #[tokio::test]
    async fn trait_is_object_safe_and_delegates_as_dyn() {
        let backend: Box<dyn TopicBackend> = Box::new(MockBackend::default());
        let locator = backend
            .write(&TopicDraft {
                name: "alpha".into(),
                description: "first".into(),
                kind: TopicKind::Project,
                body: "body".into(),
            })
            .await
            .unwrap();
        assert_eq!(locator, "mock:alpha");
        assert!(backend.list().await.unwrap().is_empty());
    }
}
