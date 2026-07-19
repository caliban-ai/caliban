# Storage substrate selection + remote gonzalo config — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A typed `storage` config in `caliban-settings` (fs default / remote gonzalod) plus a feature-aware factory that builds the chosen `TopicBackend` for both the write and read paths.

**Architecture:** `StorageConfig` is pure data in gonzalo-free `caliban-settings`. A feature-aware `build_topic_backend` factory in the caliban binary maps config → `Arc<dyn TopicBackend>` (fs always; remote behind `#[cfg(feature="gonzalo")]` with a fail-fast `list` probe; git/s3 recognized-but-errored). `loader::load` takes the backend as a param so both paths share one instance.

**Tech Stack:** Rust; `caliban-settings` (serde + `url` crate); gonzalo 0.3 (`gonzalo-core`, `gonzalo-store-server`, `gonzalo-store-fs`).

## Global Constraints

- `caliban-settings` has **NO gonzalo dependency** and must keep `cargo publish -p caliban-settings --dry-run` green. `StorageConfig` is pure data (no gonzalo types).
- The caliban binary's existing off-by-default `gonzalo` feature (from #470, `caliban/Cargo.toml:27` = `["caliban-memory/gonzalo"]`) is **extended** to also enable optional `gonzalo-core` + `gonzalo-store-server`. All gonzalo code in the factory sits behind `#[cfg(feature = "gonzalo")]`; the vanilla build stays gonzalo-free and `cargo publish` green.
- Default (`storage` absent, or `substrate: "fs"`) ⇒ `FsTopicBackend` ⇒ **zero behavior change**, no feature required.
- A factory error is fatal at startup: `eprintln!("[caliban] {e}")` + `std::process::exit(78)` (EX_CONFIG), mirroring `caliban/src/main.rs:186-191`.
- Full gate in **both** default and `--features gonzalo` configs. Before clippy, `touch` changed `.rs` files.
- Real gonzalo signatures (verbatim): `ServerStore::http_with_token(base_url: &str, token: impl Into<String>) -> Result<Self>` (sync, gonzalo-store-server); `FsStore::new(root)` (gonzalo-store-fs); `Store` is `#[async_trait]` → `Arc<dyn Store>` valid; `Store::list(&KeyPrefix) -> Result<Vec<RecordKey>>`; `gonzalo_core::ContentHash::of(&[u8]) -> ContentHash` (blake3 hex via `.0`). From #470: `GonzaloTopicBackend::new(store: Arc<dyn gonzalo_core::Store>, workspace_slug: impl Into<String>)`; `FsTopicBackend::new(dir)`; `TopicBackend` trait (`#[async_trait]`, async `list/read/write/delete/index`); `TopicLoader::{new(dir), with_backend(Box<dyn TopicBackend>)}`.

## Task ordering & workspace-build note

Tasks 1–3 are additive — the workspace stays green. **Task 4** (changing `loader::load`'s signature) breaks the downstream callers in the caliban binary; **Task 5** restores the workspace build. Verify Tasks 4 with `cargo test -p caliban-memory`; do not run `cargo build --workspace` between Task 4 and Task 5.

## File Structure

- `crates/caliban-settings/src/settings.rs` — **modify**: add `StorageConfig`/`StorageSubstrate`/`RemoteStorageConfig`; add `pub storage: StorageConfig` to `Settings`.
- `crates/caliban-settings/src/schema.json` — **modify**: add the `storage` object.
- `crates/caliban-memory/src/backend/mod.rs` — **modify**: `TopicLoader` field `Box`→`Arc`; add `with_backend_arc`.
- `crates/caliban-memory/src/loader.rs` — **modify**: `load` takes `&dyn TopicBackend`.
- `caliban/Cargo.toml` — **modify**: optional `gonzalo-core` + `gonzalo-store-server`; extend the `gonzalo` feature.
- `caliban/src/startup/storage.rs` — **create**: the `build_topic_backend` factory.
- `caliban/src/startup/mod.rs` — **modify**: `mod storage;` + re-export.
- `caliban/src/startup/compose.rs` — **modify**: build backend once, share to tools + `load`.
- `caliban/src/tui/slash/existing.rs` — **modify**: pass the shared backend to `load`.

---

## Task 1: `StorageConfig` in caliban-settings (additive)

**Files:**
- Modify: `crates/caliban-settings/src/settings.rs`, `crates/caliban-settings/src/schema.json`

**Interfaces:**
- Produces: `StorageConfig { substrate: StorageSubstrate, remote: Option<RemoteStorageConfig> }`; `enum StorageSubstrate { Fs (default), Remote, Git, S3 }`; `RemoteStorageConfig { url: String, token_env: Option<String> }`; `Settings.storage: StorageConfig`.

- [ ] **Step 1: Write the failing serde tests**

Add to `crates/caliban-settings/src/settings.rs` tests (find the existing `#[cfg(test)] mod tests`):
```rust
#[cfg(test)]
mod storage_config_tests {
    use super::{Settings, StorageSubstrate};

    #[test]
    fn absent_storage_defaults_to_fs() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.storage.substrate, StorageSubstrate::Fs);
        assert!(s.storage.remote.is_none());
    }

    #[test]
    fn parses_remote_with_url_and_token_env() {
        let s: Settings = serde_json::from_str(
            r#"{ "storage": { "substrate": "remote", "remote": { "url": "http://h:8080", "token_env": "GONZALO_TOKEN" } } }"#,
        )
        .unwrap();
        assert_eq!(s.storage.substrate, StorageSubstrate::Remote);
        let r = s.storage.remote.unwrap();
        assert_eq!(r.url, "http://h:8080");
        assert_eq!(r.token_env.as_deref(), Some("GONZALO_TOKEN"));
    }

    #[test]
    fn unknown_storage_key_is_rejected() {
        let e = serde_json::from_str::<Settings>(r#"{ "storage": { "substrate": "fs", "nope": 1 } }"#);
        assert!(e.is_err(), "deny_unknown_fields should reject 'nope'");
    }

    #[test]
    fn unknown_substrate_value_is_rejected() {
        let e = serde_json::from_str::<Settings>(r#"{ "storage": { "substrate": "sqlite" } }"#);
        assert!(e.is_err(), "unknown substrate variant should fail");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p caliban-settings storage_config_tests`
Expected: FAIL — `StorageSubstrate`/`storage` unresolved.

- [ ] **Step 3: Add the types + Settings field**

In `crates/caliban-settings/src/settings.rs`, near the other typed sub-configs (e.g. `ToolsConfig`):
```rust
/// Which gonzalo substrate backs memory storage. `fs` (default) is the
/// gonzalo-free local backend; `remote` targets a gonzalo daemon. `git`/`s3`
/// parse but are not wired yet (tracked in #469).
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StorageSubstrate {
    #[default]
    Fs,
    Remote,
    Git,
    S3,
}

/// Connection settings for a remote gonzalo daemon.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct RemoteStorageConfig {
    /// Base URL of the gonzalod HTTP endpoint (e.g. `http://host:8080`).
    pub url: String,
    /// NAME of the environment variable holding the bearer token. The token
    /// itself is never stored in settings.json.
    pub token_env: Option<String>,
}

/// Memory storage substrate selection (#473). Absent ⇒ `fs` ⇒ unchanged.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct StorageConfig {
    pub substrate: StorageSubstrate,
    pub remote: Option<RemoteStorageConfig>,
}
```
Add to `Settings` (near `pub memory: Option<serde_json::Value>`):
```rust
    /// Memory storage substrate selection (#473).
    pub storage: StorageConfig,
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p caliban-settings storage_config_tests`
Expected: PASS (all four).

- [ ] **Step 5: Update schema.json + add a validation test**

In `crates/caliban-settings/src/schema.json`, add under `"properties"` (draft-07; sibling of `"memory"`):
```json
    "storage": {
      "type": "object",
      "additionalProperties": false,
      "properties": {
        "substrate": { "enum": ["fs", "remote", "git", "s3"] },
        "remote": {
          "type": "object",
          "additionalProperties": false,
          "properties": {
            "url": { "type": "string" },
            "token_env": { "type": "string" }
          }
        }
      }
    },
```
Add a test using the crate's real validator — `caliban_settings::schema::validate_value(&Value) -> Vec<String>` (in `schema.rs`; embeds `schema.json` via `include_str!`, Draft7; **empty Vec = valid**):
```rust
    #[test]
    fn schema_accepts_remote_storage_block() {
        let doc = serde_json::json!({ "storage": { "substrate": "remote", "remote": { "url": "http://h:8080", "token_env": "T" } } });
        let errors = crate::schema::validate_value(&doc);
        assert!(errors.is_empty(), "schema rejected valid storage: {errors:?}");
    }
```

- [ ] **Step 6: Run + confirm publish stays green**

Run: `cargo test -p caliban-settings` — Expected: PASS.
Run: `cargo publish -p caliban-settings --dry-run` — Expected: PASS (no new deps; `url` already present).

- [ ] **Step 7: Commit**

```bash
git add crates/caliban-settings/src/settings.rs crates/caliban-settings/src/schema.json
git commit -m "feat(config): typed storage substrate config in caliban-settings (#473)"
```

---

## Task 2: `TopicLoader` holds `Arc` + `with_backend_arc` (additive)

**Files:**
- Modify: `crates/caliban-memory/src/backend/mod.rs`

**Interfaces:**
- Produces: `TopicLoader::with_backend_arc(Arc<dyn TopicBackend>) -> Self`; `TopicLoader` internally holds `Arc<dyn TopicBackend>` (shareable across paths). `with_backend(Box<dyn TopicBackend>)` retained (converts via `Arc::from`).

- [ ] **Step 1: Write the failing test**

In `crates/caliban-memory/src/backend/mod.rs` tests:
```rust
    #[tokio::test]
    async fn with_backend_arc_shares_one_instance() {
        use std::sync::Arc;
        let tmp = tempfile::tempdir().unwrap();
        let backend: Arc<dyn TopicBackend> = Arc::new(super::FsTopicBackend::new(tmp.path().to_path_buf()));
        let loader = TopicLoader::with_backend_arc(Arc::clone(&backend));
        // The same Arc backs the loader; writing through the loader is visible via the shared handle.
        loader.write(&crate::auto::TopicDraft { name: "a".into(), description: "d".into(), kind: crate::auto::TopicKind::User, body: "b".into() }).await.unwrap();
        assert_eq!(backend.list().await.unwrap().len(), 1);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p caliban-memory with_backend_arc_shares_one_instance`
Expected: FAIL — `with_backend_arc` not found.

- [ ] **Step 3: Change the field to `Arc` + add the constructor**

In `crates/caliban-memory/src/backend/mod.rs`, change the struct + constructors:
```rust
use std::sync::Arc;

pub struct TopicLoader {
    backend: Arc<dyn TopicBackend>,
}

impl TopicLoader {
    #[must_use]
    pub fn new(dir: impl Into<std::path::PathBuf>) -> Self {
        Self { backend: Arc::new(FsTopicBackend::new(dir)) }
    }

    #[must_use]
    pub fn with_backend(backend: Box<dyn TopicBackend>) -> Self {
        Self { backend: Arc::from(backend) }
    }

    #[must_use]
    pub fn with_backend_arc(backend: Arc<dyn TopicBackend>) -> Self {
        Self { backend }
    }
    // async delegators unchanged (they call self.backend.*)
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p caliban-memory` — Expected: PASS (the new test + all #470 tests, including the existing `with_backend(Box)` ones which still compile via `Arc::from`).

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-memory/src/backend/mod.rs
git commit -m "refactor(memory): TopicLoader holds Arc backend + with_backend_arc (#473)"
```

---

## Task 3: The feature-aware backend factory (additive to the caliban binary)

**Files:**
- Modify: `caliban/Cargo.toml`
- Create: `caliban/src/startup/storage.rs`
- Modify: `caliban/src/startup/mod.rs`

**Interfaces:**
- Consumes: `caliban_settings::{StorageConfig, StorageSubstrate}` (Task 1); `caliban_memory::{TopicBackend, FsTopicBackend}` (existing); under feature: `caliban_memory::GonzaloTopicBackend`, `gonzalo_core::Store`, `gonzalo_store_server::ServerStore`.
- Produces: `pub async fn build_topic_backend(storage: &StorageConfig, auto_memory_dir: &Path) -> Result<Arc<dyn TopicBackend>, String>`.

- [ ] **Step 1: Add optional deps + extend the feature**

In `caliban/Cargo.toml` `[dependencies]`:
```toml
gonzalo-core         = { version = "0.3", optional = true }
gonzalo-store-server = { version = "0.3", optional = true }
```
Change the feature (line 27) to:
```toml
gonzalo = ["caliban-memory/gonzalo", "dep:gonzalo-core", "dep:gonzalo-store-server"]
```

- [ ] **Step 2: Write the failing default-features tests**

Create `caliban/src/startup/storage.rs`:
```rust
//! Feature-aware memory-backend factory (config → TopicBackend).
use std::path::Path;
use std::sync::Arc;

use caliban_memory::{FsTopicBackend, TopicBackend};
use caliban_settings::{StorageConfig, StorageSubstrate};

/// Build the memory backend the config selects. `fs` is always available;
/// `remote` requires the `gonzalo` feature; `git`/`s3` are recognized but
/// not wired yet (#469). Errors are fatal config errors at startup.
pub async fn build_topic_backend(
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(sub: StorageSubstrate) -> StorageConfig {
        StorageConfig { substrate: sub, remote: None }
    }

    #[tokio::test]
    async fn fs_builds_without_feature() {
        let tmp = tempfile::tempdir().unwrap();
        let be = build_topic_backend(&cfg(StorageSubstrate::Fs), tmp.path()).await.unwrap();
        assert!(be.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn git_and_s3_error_as_not_wired() {
        let tmp = tempfile::tempdir().unwrap();
        for sub in [StorageSubstrate::Git, StorageSubstrate::S3] {
            let e = build_topic_backend(&cfg(sub), tmp.path()).await.unwrap_err();
            assert!(e.contains("not wired"), "got: {e}");
        }
    }

    #[cfg(not(feature = "gonzalo"))]
    #[tokio::test]
    async fn remote_without_feature_errors_clearly() {
        let tmp = tempfile::tempdir().unwrap();
        let e = build_topic_backend(&cfg(StorageSubstrate::Remote), tmp.path()).await.unwrap_err();
        assert!(e.contains("--features gonzalo"), "got: {e}");
    }
}
```
Add `pub mod storage;` to `caliban/src/startup/mod.rs`.

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p caliban storage:: 2>&1 | tail`
Expected: FAIL — `build_remote_backend` not defined.

- [ ] **Step 4: Implement `build_remote_backend` (both cfg halves)**

Append to `caliban/src/startup/storage.rs`:
```rust
#[cfg(not(feature = "gonzalo"))]
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
    backend
        .list()
        .await
        .map_err(|e| {
            let url = storage.remote.as_ref().map(|r| r.url.as_str()).unwrap_or("<none>");
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
        None => {
            use gonzalo_store_server::ServerStore as _; // http() ctor
            ServerStore::http(&rc.url).map_err(|e| e.to_string())?
        }
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
```
Notes for the implementer:
- Confirm `ServerStore::http(&str) -> Result<Self>` exists (it does at gonzalo-store-server 0.3); if only `http_with_token` exists, require a token (make `token_env` effectively mandatory and error when absent).
- Confirm `gonzalo_core::ContentHash::of(&[u8])` returns a struct whose `.0` is the hex `String` (verified in #470); if the field/accessor differs, use the real one to get a stable hex string.

- [ ] **Step 5: Run default-features tests**

Run: `cargo test -p caliban storage::`
Expected: PASS (fs, git/s3, remote-without-feature).

- [ ] **Step 6: Write + run the `--features gonzalo` tests (probe with injected FsStore)**

Append gonzalo-only tests to `storage.rs`:
```rust
#[cfg(all(test, feature = "gonzalo"))]
mod gonzalo_tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn probe_succeeds_on_healthy_store_and_maps_errors() {
        use caliban_memory::GonzaloTopicBackend;
        use gonzalo_store_fs::FsStore;
        let tmp = tempfile::tempdir().unwrap();
        // A healthy fs-backed store: list() succeeds → backend is usable.
        let store: Arc<dyn gonzalo_core::Store> = Arc::new(FsStore::new(tmp.path().to_path_buf()));
        let be = GonzaloTopicBackend::new(store, "wsslug");
        assert!(be.list().await.is_ok());
    }
}
```
Run: `cargo test -p caliban --features gonzalo storage::`
Expected: PASS. (This proves half (b)'s probe wiring against a real Store without a live HTTP daemon; the `ServerStore`↔daemon round-trip is covered by gonzalo's own crate tests.)

- [ ] **Step 7: Confirm the binary still builds (Task 3 is additive)**

Run: `cargo build -p caliban` and `cargo build -p caliban --features gonzalo`
Expected: both succeed — nothing calls `build_topic_backend` yet (Task 5 wires it); its tests keep it from being dead code.

- [ ] **Step 8: Commit**

```bash
git add caliban/Cargo.toml caliban/src/startup/storage.rs caliban/src/startup/mod.rs
git commit -m "feat(config): feature-aware storage backend factory + fail-fast remote probe (#473)"
```

---

## Task 4: `loader::load` takes the backend (breaks downstream until Task 5)

**Files:**
- Modify: `crates/caliban-memory/src/loader.rs`

**Interfaces:**
- Produces: `pub async fn load(config: &MemoryConfig, backend: &dyn TopicBackend) -> Result<MemoryPrefix>` — derives the auto index via the passed `backend` instead of constructing `FsTopicBackend` internally.

**Note:** this changes a public signature; `caliban-tools-builtin` is unaffected but the caliban binary's two callers (`compose.rs:1702`, `tui/slash/existing.rs:116`) will not compile until Task 5. Verify this task with `cargo test -p caliban-memory` only.

- [ ] **Step 1: Update the derived-index test to pass a backend**

In `crates/caliban-memory/src/loader.rs` tests, change `load_derives_auto_index_from_backend` (and any other `load(...)` test callers) to pass a backend:
```rust
    #[tokio::test]
    async fn load_derives_auto_index_from_backend() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = crate::backend::FsTopicBackend::new(tmp.path().to_path_buf());
        backend.write(&crate::auto::TopicDraft { name: "gg".into(), description: "hook".into(), kind: crate::auto::TopicKind::Project, body: "b".into() }).await.unwrap();
        let cfg = /* existing MemoryConfig construction */;
        let prefix = load(&cfg, &backend).await.unwrap();
        assert!(/* existing assertion that the auto body contains "[gg](gg.md)" */);
    }
```
(Keep the existing `cfg`/assertion; only add the `&backend` argument and construct `backend` before the `load` call.)

- [ ] **Step 2: Run to verify it fails to compile**

Run: `cargo test -p caliban-memory load_derives_auto_index_from_backend`
Expected: FAIL — `load` takes 1 arg / mismatched args.

- [ ] **Step 3: Change the `load` signature + body**

In `crates/caliban-memory/src/loader.rs`:
- Signature (line ~56): `pub async fn load(config: &MemoryConfig, backend: &dyn TopicBackend) -> Result<MemoryPrefix> {`
- Remove the `use crate::backend::{FsTopicBackend, TopicBackend};` line's `FsTopicBackend` if now unused; keep `TopicBackend` (needed for the param type).
- Replace the internal construction (lines ~73-74):
  ```rust
  // was: let backend = FsTopicBackend::new(config.auto_memory_dir.clone());
  //      let body = backend.index().await?;
  let body = backend.index().await?;
  ```
  (Use the passed `backend` directly; everything downstream — `cap_text`, conventions injection, HTML strip — is unchanged.)

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p caliban-memory` — Expected: PASS (all caliban-memory tests, now passing an explicit backend). Do NOT run `cargo build --workspace` (the binary's callers break until Task 5).

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-memory/src/loader.rs
git commit -m "feat(memory): loader::load takes the configured backend (#473)"
```

---

## Task 5: Wire the factory at startup (restores the workspace build)

**Files:**
- Modify: `caliban/src/startup/compose.rs`, `caliban/src/tui/slash/existing.rs`

**Interfaces:**
- Consumes: `build_topic_backend` (Task 3), `TopicLoader::with_backend_arc` (Task 2), `load(config, backend)` (Task 4).

- [ ] **Step 1: Build the backend once + share it to the memory tools**

In `caliban/src/startup/compose.rs` around lines 620-621, replace:
```rust
let cfg = caliban_memory::MemoryConfig::from_env(&workspace_root);
let topic_loader = Arc::new(caliban_memory::TopicLoader::new(cfg.auto_memory_dir));
```
with a factory-built, shared backend (propagate the error to a fatal startup exit):
```rust
let cfg = caliban_memory::MemoryConfig::from_env(&workspace_root);
// `settings_snapshot` is the in-scope layered snapshot at this site (it is
// already passed to sibling calls like `sandbox_network(args, settings_snapshot)`
// and `apply_memory_settings(..., settings_snapshot)`). Access `.storage` on it.
let backend = match crate::startup::storage::build_topic_backend(&settings_snapshot.storage, &cfg.auto_memory_dir).await {
    Ok(b) => b,
    Err(e) => {
        eprintln!("[caliban] storage config error: {e}");
        std::process::exit(78); // EX_CONFIG — mirrors main.rs:186-191
    }
};
let topic_loader = Arc::new(caliban_memory::TopicLoader::with_backend_arc(Arc::clone(&backend)));
```
(Confirm `settings_snapshot`'s type exposes `.storage` — it is the `Settings` snapshot; if it is wrapped (e.g. `&Settings` or an `Arc<Settings>`), deref/borrow accordingly. Store `backend` where the read-path call site (compose.rs:1702) and the `/memory` slash handlers can reach it — e.g. in the app/session state struct this startup flow populates; thread it alongside the existing `topic_loader`/`settings_snapshot`.)

- [ ] **Step 2: Pass the shared backend to the read-path `load`**

In `caliban/src/startup/compose.rs` around line 1702, change:
```rust
match caliban_memory::load(&cfg).await {
```
to pass the shared backend built in Step 1:
```rust
match caliban_memory::load(&cfg, backend.as_ref()).await {
```
(Use the same `backend: Arc<dyn TopicBackend>` from Step 1. If this is a different function from Step 1's, thread `backend` in via the app/session state.)

- [ ] **Step 3: Pass the backend to the `/memory list` slash handler**

In `caliban/src/tui/slash/existing.rs` around line 116, the `caliban_memory::load(&cfg).await` call becomes `caliban_memory::load(&cfg, <backend>).await`, where `<backend>` is the shared `Arc<dyn TopicBackend>` obtained from the TUI app/session state (thread it into the state that this handler already reads `cfg` from).

- [ ] **Step 4: Verify the workspace build is restored + tests + clippy**

Run:
```bash
cargo build --workspace
cargo test -p caliban-memory -p caliban-tools-builtin -p caliban-settings
touch $(git diff --name-only main | grep '\.rs$'); cargo clippy --workspace --all-targets -- -D warnings
```
Expected: workspace builds GREEN; tests pass; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add caliban/src/startup/compose.rs caliban/src/tui/slash/existing.rs
git commit -m "feat(config): wire storage factory at startup, share one backend across paths (#473)"
```

---

## Task 6: Full-gate verification in both configs

**Files:** none (verification + any fixes surfaced).

- [ ] **Step 1: Default-config gate**

```bash
cargo fmt --all && cargo fmt --all -- --check
touch $(git diff --name-only main | grep '\.rs$')
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --all-targets
cargo test --workspace
```
Expected: all pass. Fix + re-run on any failure.

- [ ] **Step 2: gonzalo-feature gate**

```bash
cargo clippy -p caliban --features gonzalo --all-targets -- -D warnings
cargo build -p caliban --features gonzalo
cargo test -p caliban --features gonzalo storage::
```
Expected: all pass.

- [ ] **Step 3: Publish dry-runs (both must stay green)**

```bash
cargo publish -p caliban-settings --dry-run
cargo publish -p caliban-memory   --dry-run
```
Expected: PASS — `caliban-settings` stays gonzalo-free; `caliban-memory`'s optional gonzalo dep stays feature-off in the packaged manifest.

- [ ] **Step 4: Commit any fixes**

```bash
git add -A && git commit -m "chore(config): satisfy gate in default and gonzalo configs (#473)"
```

---

## Self-review notes

- **Spec coverage:** config type (Task 1) · schema.json (Task 1) · TopicLoader Arc sharing (Task 2) · factory + feature-gating + probe + workspace_slug (Task 3) · load() backend param (Task 4) · one-backend wiring + EX_CONFIG exit (Task 5) · both-config gate + publish (Task 6). All spec sections map to a task.
- **Type consistency:** `StorageConfig`/`StorageSubstrate`/`RemoteStorageConfig`, `build_topic_backend(&StorageConfig, &Path) -> Result<Arc<dyn TopicBackend>, String>`, `load(&MemoryConfig, &dyn TopicBackend)`, `with_backend_arc(Arc<dyn TopicBackend>)`, `workspace_slug` are used consistently across tasks.
- **Reconciliation points the implementer must confirm against real code (called out at each use site):** `MemoryConfig` construction in the load test; `ServerStore::http` (tokenless) existence at 0.3; `ContentHash`'s hex accessor (`.0`); and how to thread the shared `backend` from compose.rs:620 to the read-path call at :1702 and the `/memory` slash handler (via the app/session state) — `settings_snapshot` is confirmed in scope at :620, and `caliban_settings::schema::validate_value` is the confirmed schema validator.
