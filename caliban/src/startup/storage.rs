//! Feature-aware memory-backend factory (config → `TopicBackend`).
// `build_topic_backend` and its helpers are wired into startup in Task 5 (#473);
// until then they are reachable only from tests, so allow dead_code module-wide.
// Remove this once the startup wiring calls the factory.
#![allow(dead_code)]
use std::path::Path;
use std::sync::Arc;

use caliban_memory::{FsTopicBackend, TopicBackend};
use caliban_settings::{StorageConfig, StorageSubstrate};

/// Build the memory backend the config selects. `fs` is always available;
/// `remote` requires the `gonzalo` feature; `git`/`s3` are recognized but
/// not wired yet (#469). Errors are fatal config errors at startup.
pub(crate) async fn build_topic_backend(
    storage: &StorageConfig,
    auto_memory_dir: &Path,
) -> Result<Arc<dyn TopicBackend>, String> {
    match storage.substrate {
        StorageSubstrate::Fs => Ok(Arc::new(FsTopicBackend::new(auto_memory_dir))),
        StorageSubstrate::Remote => build_remote_backend(storage, auto_memory_dir).await,
        other @ (StorageSubstrate::Git | StorageSubstrate::S3) => Err(format!(
            "storage.substrate {other:?} is recognized but not wired yet (tracked in #469); use fs or remote"
        )),
    }
}

#[cfg(not(feature = "gonzalo"))]
#[allow(clippy::unused_async)] // async for signature parity with the gonzalo variant
async fn build_remote_backend(
    _storage: &StorageConfig,
    _auto_memory_dir: &Path,
) -> Result<Arc<dyn TopicBackend>, String> {
    Err("this build lacks gonzalo support; rebuild with `--features gonzalo` to use a remote substrate".to_string())
}

#[cfg(feature = "gonzalo")]
async fn build_remote_backend(
    storage: &StorageConfig,
    auto_memory_dir: &Path,
) -> Result<Arc<dyn TopicBackend>, String> {
    use caliban_memory::GonzaloTopicBackend;
    let store = remote_store(storage)?;
    let slug = workspace_slug(auto_memory_dir);
    let backend = GonzaloTopicBackend::new(store, slug);
    // Fail-fast connectivity probe: a healthy daemon answers `list`.
    backend.list().await.map_err(|e| {
        let url = storage
            .remote
            .as_ref()
            .map(|r| r.url.as_str())
            .unwrap_or("<none>");
        format!("gonzalo remote {url} unreachable/unauthorized: {e}")
    })?;
    Ok(Arc::new(backend))
}

/// (a) config → Store. Reads the bearer token from the named env var.
#[cfg(feature = "gonzalo")]
fn remote_store(storage: &StorageConfig) -> Result<Arc<dyn gonzalo_core::Store>, String> {
    use gonzalo_store_server::ServerStore;
    let rc = storage
        .remote
        .as_ref()
        .ok_or("storage.substrate=remote requires a [storage.remote] block")?;
    let store = match &rc.token_env {
        Some(env_name) => {
            let token = std::env::var(env_name)
                .map_err(|_| format!("token env `{env_name}` is not set"))?;
            ServerStore::http_with_token(&rc.url, token).map_err(|e| e.to_string())?
        }
        None => ServerStore::http(&rc.url).map_err(|e| e.to_string())?,
    };
    Ok(Arc::new(store))
}

/// Stable per-workspace slug = blake3 hex of the canonical memory dir. Reuses
/// gonzalo's own content hasher (no new dep). Matches #470's RecordKey scheme.
#[cfg(feature = "gonzalo")]
fn workspace_slug(auto_memory_dir: &Path) -> String {
    let canon = auto_memory_dir
        .canonicalize()
        .unwrap_or_else(|_| auto_memory_dir.to_path_buf());
    gonzalo_core::ContentHash::of(canon.to_string_lossy().as_bytes()).0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(sub: StorageSubstrate) -> StorageConfig {
        StorageConfig {
            substrate: sub,
            remote: None,
        }
    }

    #[tokio::test]
    async fn fs_builds_without_feature() {
        let tmp = tempfile::tempdir().unwrap();
        let be = build_topic_backend(&cfg(StorageSubstrate::Fs), tmp.path())
            .await
            .unwrap();
        assert!(be.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn git_and_s3_error_as_not_wired() {
        let tmp = tempfile::tempdir().unwrap();
        for sub in [StorageSubstrate::Git, StorageSubstrate::S3] {
            let e = build_topic_backend(&cfg(sub), tmp.path())
                .await
                .err()
                .unwrap();
            assert!(e.contains("not wired"), "got: {e}");
        }
    }

    #[cfg(not(feature = "gonzalo"))]
    #[tokio::test]
    async fn remote_without_feature_errors_clearly() {
        let tmp = tempfile::tempdir().unwrap();
        let e = build_topic_backend(&cfg(StorageSubstrate::Remote), tmp.path())
            .await
            .err()
            .unwrap();
        assert!(e.contains("--features gonzalo"), "got: {e}");
    }
}

#[cfg(all(test, feature = "gonzalo"))]
mod gonzalo_tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn probe_succeeds_on_healthy_store() {
        use caliban_memory::GonzaloTopicBackend;
        use gonzalo_store_fs::FsStore;
        let tmp = tempfile::tempdir().unwrap();
        // A healthy fs-backed store: list() succeeds → the backend is usable.
        let store: Arc<dyn gonzalo_core::Store> = Arc::new(FsStore::new(tmp.path().to_path_buf()));
        let be = GonzaloTopicBackend::new(store, "wsslug");
        assert!(be.list().await.is_ok());
    }
}
