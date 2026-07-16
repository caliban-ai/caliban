//! The storage seam for auto-memory topics.
use async_trait::async_trait;

use crate::auto::{TopicDraft, TopicFile, TopicSummary};
use crate::error::Result;

pub(crate) mod fs;
pub use fs::FsTopicBackend;

#[cfg(feature = "gonzalo")]
pub mod gonzalo;
#[cfg(feature = "gonzalo")]
pub use gonzalo::GonzaloTopicBackend;

#[cfg(test)]
pub(crate) mod conformance;

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

use std::path::PathBuf;

/// Public facade over a chosen [`TopicBackend`]. `new` selects the fs backend
/// (behaviour-equivalent to the historical `std::fs` loader).
pub struct TopicLoader {
    backend: Box<dyn TopicBackend>,
}

impl std::fmt::Debug for TopicLoader {
    // `TopicBackend` doesn't require `Debug` (it must stay object-safe and
    // substrate-neutral), so this facade can't derive it. Callers that embed
    // a `TopicLoader` in a `#[derive(Debug)]` struct still get a (redacted)
    // impl this way.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TopicLoader").finish_non_exhaustive()
    }
}

impl TopicLoader {
    /// Construct a loader backed by [`FsTopicBackend`] over `dir`. The directory
    /// does not have to exist yet — `list` returns an empty vec, and `write`
    /// will create it on demand.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            backend: Box::new(FsTopicBackend::new(dir)),
        }
    }

    /// Construct a loader over an arbitrary [`TopicBackend`] (e.g. a gonzalo
    /// substrate).
    #[must_use]
    pub fn with_backend(backend: Box<dyn TopicBackend>) -> Self {
        Self { backend }
    }

    /// List all topics.
    ///
    /// # Errors
    ///
    /// Returns any error surfaced by the underlying backend.
    pub async fn list(&self) -> Result<Vec<TopicSummary>> {
        self.backend.list().await
    }

    /// Read a topic by name.
    ///
    /// # Errors
    ///
    /// Returns any error surfaced by the underlying backend.
    pub async fn read(&self, name: &str) -> Result<TopicFile> {
        self.backend.read(name).await
    }

    /// Persist `draft`, returning a human-readable locator.
    ///
    /// # Errors
    ///
    /// Returns any error surfaced by the underlying backend.
    pub async fn write(&self, draft: &TopicDraft) -> Result<String> {
        self.backend.write(draft).await
    }

    /// Delete a topic by name.
    ///
    /// # Errors
    ///
    /// Returns any error surfaced by the underlying backend.
    pub async fn delete(&self, name: &str) -> Result<()> {
        self.backend.delete(name).await
    }

    /// Rebuild the `MEMORY.md` index body from the current topic set.
    ///
    /// # Errors
    ///
    /// Returns any error surfaced by the underlying backend.
    pub async fn index(&self) -> Result<String> {
        self.backend.index().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auto::{TopicDraft, TopicKind};
    use std::sync::Mutex;

    /// Minimal in-memory backend proving the trait is object-safe (`Box<dyn>`),
    /// async, and Result-plumbed. Task 2 adds the real `FsTopicBackend` + facade.
    #[derive(Default)]
    struct MockBackend {
        writes: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl TopicBackend for MockBackend {
        async fn list(&self) -> Result<Vec<TopicSummary>> {
            Ok(Vec::new())
        }
        async fn read(&self, _name: &str) -> Result<TopicFile> {
            Err(crate::error::MemoryError::Backend("mock".into()))
        }
        async fn write(&self, draft: &TopicDraft) -> Result<String> {
            self.writes.lock().unwrap().push(draft.name.clone());
            Ok(format!("mock:{}", draft.name))
        }
        async fn delete(&self, _name: &str) -> Result<()> {
            Ok(())
        }
        async fn index(&self) -> Result<String> {
            Ok(String::new())
        }
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

    #[tokio::test]
    async fn facade_new_uses_fs_backend() {
        let tmp = tempfile::tempdir().unwrap();
        let loader = TopicLoader::new(tmp.path().to_path_buf());
        loader
            .write(&TopicDraft {
                name: "alpha".into(),
                description: "d".into(),
                kind: TopicKind::Project,
                body: "b".into(),
            })
            .await
            .unwrap();
        let names: Vec<_> = loader
            .list()
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(names, vec!["alpha".to_string()]);
    }
}
