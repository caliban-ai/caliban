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
    // `remote_store` above already errors when `storage.remote` is `None`, so
    // it's provably `Some` here.
    let url = &storage
        .remote
        .as_ref()
        .expect("remote_store already validated Some")
        .url;
    backend
        .list()
        .await
        .map_err(|e| format!("gonzalo remote {url} unreachable/unauthorized: {e}"))?;
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

/// Stable per-workspace slug = blake3 hex of the configured memory-dir path.
/// Deterministic regardless of whether the dir exists yet, and independent of
/// symlink resolution. Reuses gonzalo's content hasher (no new dep). Matches
/// #470's `RecordKey` scheme.
#[cfg(feature = "gonzalo")]
fn workspace_slug(auto_memory_dir: &Path) -> String {
    gonzalo_core::ContentHash::of(auto_memory_dir.to_string_lossy().as_bytes()).0
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

    use caliban_settings::{RemoteStorageConfig, StorageConfig, StorageSubstrate};

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

    #[test]
    fn workspace_slug_is_deterministic_and_path_sensitive() {
        let a = workspace_slug(std::path::Path::new("/some/mem/dir"));
        let a2 = workspace_slug(std::path::Path::new("/some/mem/dir"));
        let b = workspace_slug(std::path::Path::new("/other/mem/dir"));
        assert_eq!(a, a2, "same path must hash identically");
        assert_ne!(a, b, "different paths must differ");
        assert!(!a.is_empty());
    }

    #[test]
    fn remote_store_ok_with_url_and_no_token() {
        let cfg = StorageConfig {
            substrate: StorageSubstrate::Remote,
            remote: Some(RemoteStorageConfig {
                url: "http://127.0.0.1:8080".into(),
                token_env: None,
            }),
        };
        // ServerStore::http is lazy (no connection), so construction succeeds without a daemon.
        assert!(remote_store(&cfg).is_ok());
    }

    #[test]
    fn remote_store_errors_without_remote_block() {
        let cfg = StorageConfig {
            substrate: StorageSubstrate::Remote,
            remote: None,
        };
        let e = remote_store(&cfg).err().unwrap();
        assert!(e.contains("requires a [storage.remote] block"), "got: {e}");
    }

    #[test]
    fn remote_store_errors_when_token_env_unset() {
        let cfg = StorageConfig {
            substrate: StorageSubstrate::Remote,
            remote: Some(RemoteStorageConfig {
                url: "http://127.0.0.1:8080".into(),
                token_env: Some("CALIBAN_TEST_TOKEN_DEFINITELY_UNSET_9df3".into()),
            }),
        };
        let e = remote_store(&cfg).err().unwrap();
        assert!(e.contains("not set"), "got: {e}");
    }
}
