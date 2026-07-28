# Sessions via the gonzalo facade + async debounce-writer rework — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route `caliban-sessions` persistence through a substrate-neutral `SessionBackend` trait so sessions persist to local `fs` (default, unchanged) or a remote `gonzalo` daemon by config, with the debounced writer reworked to await async `put`s and surface conflicts/errors.

**Architecture:** Keep `SessionStore` as the public facade; inject an `Arc<dyn SessionBackend>`. The `DebouncedWriter`'s terminal `write_atomic` becomes `backend.save(&session).await` on the worker's existing current-thread runtime. `FsSessionBackend` is always compiled (gonzalo-free); `GonzaloSessionBackend` is `#[cfg(feature = "gonzalo")]` (OCC get→put). A `build_session_backend` factory beside `startup/storage.rs` selects between them from the shared `StorageConfig`. This mirrors #470 (memory `TopicBackend`) and #473 (storage factory) exactly.

**Tech Stack:** Rust, `async-trait`, `tokio` (current-thread runtime on a dedicated OS thread), `serde_json`, gonzalo-core/gonzalo-store-fs `0.3` (optional), `gonzalo-store-server` `0.3` (binary crate, optional).

## Global Constraints

- `caliban-sessions` MUST compile with **zero** gonzalo references in the default (no-feature) build; all gonzalo code is `#[cfg(feature = "gonzalo")]`-gated — `cargo publish --dry-run` must stay green.
- Default / absent / `fs` config = **zero behavior change**: pretty-JSON files under the same root, same layout.
- Bearer token comes from an **env var** (`token_env`), never `settings.json` — reuse #473's `remote_store` helper.
- Factory error is **fatal at startup** — `std::process::exit(78)` (EX_CONFIG), matching the memory factory.
- gonzalo crates pinned at `0.3` (registry deps, `optional = true`), mirroring `caliban-memory`.
- `RecordKind::Session`; record key `RecordKey::new("caliban", format!("sessions:{workspace_slug}"), name)` (workspace-scoped).
- CI gate mirrors `.github/workflows/ci.yml`: `cargo fmt --all -- --check` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo build --workspace --all-targets` · `cargo test --workspace`, plus `--features gonzalo` clippy/build/test and `cargo publish --workspace --dry-run`.

---

## File Structure

- `crates/caliban-sessions/src/backend/mod.rs` (Create) — `SessionBackend` trait + conformance harness module wiring.
- `crates/caliban-sessions/src/backend/fs.rs` (Create) — `FsSessionBackend` (always compiled).
- `crates/caliban-sessions/src/backend/gonzalo.rs` (Create) — `GonzaloSessionBackend` (`#[cfg(feature = "gonzalo")]`).
- `crates/caliban-sessions/src/backend/conformance.rs` (Create) — shared `run_session_backend_conformance`.
- `crates/caliban-sessions/src/store.rs` (Modify) — `SessionStore` holds `Arc<dyn SessionBackend>`; `with_backend` ctor; `new` delegates to `FsSessionBackend`.
- `crates/caliban-sessions/src/debounced.rs` (Modify) — writer holds the backend; buffers `HashMap<String, PersistedSession>`; async drain; `Load`/`List`/`Delete` round-trip messages.
- `crates/caliban-sessions/src/error.rs` (Modify) — add `Conflict` variant.
- `crates/caliban-sessions/src/lib.rs` (Modify) — export the backend types.
- `crates/caliban-sessions/Cargo.toml` (Modify) — `gonzalo` feature + optional deps.
- `caliban/src/startup/storage.rs` (Modify) — add `build_session_backend` + gonzalo `remote_session_backend`.
- `caliban/Cargo.toml` (Modify) — `gonzalo` feature adds `"caliban-sessions/gonzalo"`.
- `caliban/src/startup/compose.rs` (Modify) — `resolve_session` uses an injected backend; add `session_store_needed`.
- `caliban/src/main.rs` (Modify) — build the session backend via factory (exit 78 on error) and thread it in.
- `caliban/src/startup/drivers.rs` (Modify) — `resolve_resume` fallback builds an fs-backed store.
- `caliban/tests/` (Create) — checkpoint-restore-over-gonzalo regression test (or colocated).

---

### Task 1: `SessionBackend` trait + conformance harness

**Files:**
- Create: `crates/caliban-sessions/src/backend/mod.rs`
- Create: `crates/caliban-sessions/src/backend/conformance.rs`
- Modify: `crates/caliban-sessions/src/lib.rs`
- Modify: `crates/caliban-sessions/src/error.rs`

**Interfaces:**
- Consumes: `PersistedSession` (`crate::session`), `SessionMetadata` (currently in `store.rs` — Task 1 moves nothing; it references the type via `crate::store::SessionMetadata`), `Result`/`Error` (`crate::error`).
- Produces:
  - `#[async_trait] pub trait SessionBackend: Send + Sync` with:
    - `async fn save(&self, session: &PersistedSession) -> Result<()>`
    - `async fn load(&self, name: &str) -> Result<Option<PersistedSession>>`
    - `async fn list(&self) -> Result<Vec<SessionMetadata>>`
    - `async fn delete(&self, name: &str) -> Result<()>`
  - `pub(crate) async fn run_session_backend_conformance<B: SessionBackend>(be: &B)`
  - `Error::Conflict { name: String }` variant.

- [ ] **Step 1: Add the `Conflict` error variant**

In `crates/caliban-sessions/src/error.rs`, add to `enum Error`:

```rust
    /// A remote optimistic-concurrency write lost a race: the store's current
    /// revision differed from the one this write expected. Surfaced (not
    /// union-merged) so a divergent multi-writer save is observable (#471).
    #[error("session '{name}' conflict: a concurrent write moved it")]
    Conflict {
        /// The session name that conflicted.
        name: String,
    },
```

- [ ] **Step 2: Write the failing trait/conformance test**

Create `crates/caliban-sessions/src/backend/mod.rs`:

```rust
//! The storage seam for persisted sessions.
use async_trait::async_trait;

use crate::error::Result;
use crate::session::PersistedSession;
use crate::store::SessionMetadata;

pub(crate) mod fs;
pub use fs::FsSessionBackend;

#[cfg(feature = "gonzalo")]
pub mod gonzalo;
#[cfg(feature = "gonzalo")]
pub use gonzalo::GonzaloSessionBackend;

#[cfg(test)]
pub(crate) mod conformance;

/// Substrate-neutral CRUD over persisted sessions.
#[async_trait]
pub trait SessionBackend: Send + Sync {
    /// Persist `session` (create or overwrite by name).
    async fn save(&self, session: &PersistedSession) -> Result<()>;
    /// Load a session by name; `Ok(None)` if it does not exist.
    async fn load(&self, name: &str) -> Result<Option<PersistedSession>>;
    /// List session metadata, sorted by `updated_at` descending.
    async fn list(&self) -> Result<Vec<SessionMetadata>>;
    /// Delete a session by name; idempotent (`Ok(())` if absent).
    async fn delete(&self, name: &str) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MockBackend {
        saved: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl SessionBackend for MockBackend {
        async fn save(&self, session: &PersistedSession) -> Result<()> {
            self.saved.lock().unwrap().push(session.name.clone());
            Ok(())
        }
        async fn load(&self, _name: &str) -> Result<Option<PersistedSession>> {
            Ok(None)
        }
        async fn list(&self) -> Result<Vec<SessionMetadata>> {
            Ok(Vec::new())
        }
        async fn delete(&self, _name: &str) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn trait_is_object_safe_and_delegates_as_dyn() {
        let be: Box<dyn SessionBackend> = Box::new(MockBackend::default());
        be.save(&PersistedSession::new("s", "anthropic", "m"))
            .await
            .unwrap();
        assert!(be.load("s").await.unwrap().is_none());
        assert!(be.list().await.unwrap().is_empty());
    }
}
```

Add to `crates/caliban-sessions/src/lib.rs` (after `pub mod store;`):

```rust
pub mod backend;

pub use backend::{FsSessionBackend, SessionBackend};
```

(`GonzaloSessionBackend` is exported from `lib.rs` in Task 4, feature-gated.)

Create `crates/caliban-sessions/src/backend/fs.rs` with a **stub** so `mod.rs` compiles (Task 2 fills it):

```rust
//! Filesystem-backed session store (the gonzalo-free default).
use std::path::PathBuf;

use async_trait::async_trait;

use crate::backend::SessionBackend;
use crate::error::Result;
use crate::session::PersistedSession;
use crate::store::SessionMetadata;

/// Filesystem session store: one pretty-JSON `<name>.json` per session.
#[derive(Debug, Clone)]
pub struct FsSessionBackend {
    root: PathBuf,
}

impl FsSessionBackend {
    /// Construct a backend over `root`. The directory need not exist yet.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

#[async_trait]
impl SessionBackend for FsSessionBackend {
    async fn save(&self, _session: &PersistedSession) -> Result<()> {
        unimplemented!("Task 2")
    }
    async fn load(&self, _name: &str) -> Result<Option<PersistedSession>> {
        unimplemented!("Task 2")
    }
    async fn list(&self) -> Result<Vec<SessionMetadata>> {
        unimplemented!("Task 2")
    }
    async fn delete(&self, _name: &str) -> Result<()> {
        unimplemented!("Task 2")
    }
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p caliban-sessions backend::tests::trait_is_object_safe -- --nocapture`
Expected: FAIL to compile first if anything is missing, then (once compiling) PASS is not yet expected because `conformance.rs` is missing (referenced by `mod.rs`). Create `conformance.rs` next, then this compiles.

- [ ] **Step 4: Write the conformance harness**

Create `crates/caliban-sessions/src/backend/conformance.rs`:

```rust
//! Backend-agnostic conformance battery. Any `SessionBackend` must pass it.
#![cfg(test)]
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
```

- [ ] **Step 5: Run the object-safety test to verify it passes**

Run: `cargo test -p caliban-sessions backend::tests::trait_is_object_safe`
Expected: PASS (the `MockBackend` exercises the `dyn` trait; the conformance harness compiles under `#[cfg(test)]` but is not yet invoked — Tasks 2 & 4 invoke it).

- [ ] **Step 6: Commit**

```bash
git add crates/caliban-sessions/src/backend/mod.rs \
        crates/caliban-sessions/src/backend/fs.rs \
        crates/caliban-sessions/src/backend/conformance.rs \
        crates/caliban-sessions/src/error.rs \
        crates/caliban-sessions/src/lib.rs
git commit -m "feat(sessions): SessionBackend trait + conformance harness (#471)"
```

---

### Task 2: `FsSessionBackend` (lift current fs logic behind the trait)

**Files:**
- Modify: `crates/caliban-sessions/src/backend/fs.rs`
- Reference: `crates/caliban-sessions/src/store.rs:20-33` (`validate_name`, `MAX_NAME_LEN`), `:87-200` (current `load`/`list` logic).

**Interfaces:**
- Consumes: `SessionBackend` (Task 1), `PersistedSession`, `SessionMetadata`.
- Produces: a fully-implemented `FsSessionBackend` that passes `run_session_backend_conformance`. `validate_name` lives here now (`pub(crate)` so `store.rs` can drop its copy or re-use it).

- [ ] **Step 1: Write the failing conformance + fs-specifics tests**

Append to `crates/caliban-sessions/src/backend/fs.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::conformance::run_session_backend_conformance;

    #[tokio::test]
    async fn fs_backend_passes_conformance() {
        let tmp = tempfile::tempdir().unwrap();
        let be = FsSessionBackend::new(tmp.path().to_path_buf());
        run_session_backend_conformance(&be).await;
    }

    #[tokio::test]
    async fn save_writes_pretty_json_file_named_by_session() {
        let tmp = tempfile::tempdir().unwrap();
        let be = FsSessionBackend::new(tmp.path().to_path_buf());
        be.save(&PersistedSession::new("mysess", "anthropic", "m"))
            .await
            .unwrap();
        let path = tmp.path().join("mysess.json");
        assert!(path.exists());
        let bytes = std::fs::read(&path).unwrap();
        // pretty JSON has newlines
        assert!(String::from_utf8_lossy(&bytes).contains('\n'));
    }

    #[tokio::test]
    async fn list_skips_broken_and_non_json_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path()).unwrap();
        std::fs::write(tmp.path().join("notjson.txt"), b"x").unwrap();
        std::fs::write(tmp.path().join("broken.json"), b"{ not json").unwrap();
        let be = FsSessionBackend::new(tmp.path().to_path_buf());
        be.save(&PersistedSession::new("good", "anthropic", "m"))
            .await
            .unwrap();
        let list = be.list().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "good");
    }

    #[tokio::test]
    async fn invalid_name_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let be = FsSessionBackend::new(tmp.path().to_path_buf());
        let bad = PersistedSession::new("../escape", "anthropic", "m");
        assert!(matches!(
            be.save(&bad).await,
            Err(crate::error::Error::InvalidName(_))
        ));
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p caliban-sessions backend::fs::tests::fs_backend_passes_conformance`
Expected: FAIL — the current `fs.rs` methods `unimplemented!()` (panic).

- [ ] **Step 3: Implement `FsSessionBackend`**

Replace the stub `impl SessionBackend for FsSessionBackend` (and add helpers) in `crates/caliban-sessions/src/backend/fs.rs`:

```rust
use std::cmp::Reverse;

use crate::error::Error;

const MAX_NAME_LEN: usize = 64;

/// Validate a session name: non-empty, `<= 64` chars, `[a-zA-Z0-9_-]+`.
pub(crate) fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > MAX_NAME_LEN {
        return Err(Error::InvalidName(name.into()));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(Error::InvalidName(name.into()));
    }
    Ok(())
}

impl FsSessionBackend {
    fn path_for(&self, name: &str) -> PathBuf {
        self.root.join(format!("{name}.json"))
    }
}

#[async_trait]
impl SessionBackend for FsSessionBackend {
    async fn save(&self, session: &PersistedSession) -> Result<()> {
        validate_name(&session.name)?;
        std::fs::create_dir_all(&self.root)?;
        let serialized = serde_json::to_vec_pretty(session)?;
        let target = self.path_for(&session.name);
        caliban_common::fs::write_atomic(&target, &serialized)
            .map_err(|e| Error::Persist(e.to_string()))?;
        Ok(())
    }

    async fn load(&self, name: &str) -> Result<Option<PersistedSession>> {
        validate_name(name)?;
        match std::fs::read(self.path_for(name)) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn list(&self) -> Result<Vec<SessionMetadata>> {
        let mut out = Vec::new();
        let entries = match std::fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e.into()),
        };
        for entry in entries {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let Ok(session): std::result::Result<PersistedSession, _> =
                serde_json::from_slice(&bytes)
            else {
                continue;
            };
            out.push(SessionMetadata::from_session(&session));
        }
        out.sort_by_key(|b| Reverse(b.updated_at));
        Ok(out)
    }

    async fn delete(&self, name: &str) -> Result<()> {
        validate_name(name)?;
        match std::fs::remove_file(self.path_for(name)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}
```

Add a `SessionMetadata::from_session` associated fn in `crates/caliban-sessions/src/store.rs` (next to the struct) so both backends derive metadata identically:

```rust
impl SessionMetadata {
    /// Derive list metadata from a full session (shared by all backends).
    #[must_use]
    pub fn from_session(session: &PersistedSession) -> Self {
        Self {
            name: session.name.clone(),
            updated_at: session.updated_at,
            turn_count: session.turn_count(),
            total_tokens: session
                .total_usage
                .input_tokens
                .saturating_add(session.total_usage.output_tokens),
        }
    }
}
```

(`turn_count()` already exists on `PersistedSession`, `session.rs:72`. Remove the duplicated inline metadata construction from the old `store.rs::list` in Task 3.)

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p caliban-sessions backend::fs`
Expected: PASS (all four fs tests + conformance).

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-sessions/src/backend/fs.rs crates/caliban-sessions/src/store.rs
git commit -m "feat(sessions): FsSessionBackend behind the trait, passes conformance (#471)"
```

---

### Task 3: `DebouncedWriter` rework + `SessionStore` over the backend

**Files:**
- Modify: `crates/caliban-sessions/src/debounced.rs`
- Modify: `crates/caliban-sessions/src/store.rs`

**Interfaces:**
- Consumes: `Arc<dyn SessionBackend>` (Task 1), `PersistedSession`, `SessionMetadata::from_session` (Task 2), `Error::Conflict`/`Error::Persist`.
- Produces:
  - `DebouncedWriter::new(backend: Arc<dyn SessionBackend>)` — buffers `HashMap<String, PersistedSession>`, async-drains via `backend.save`.
  - `DebouncedWriter::{request(session), flush() -> Result<(), String>, last_error() -> Option<String>, load(name) -> Result<Option<PersistedSession>, String>, list() -> Result<Vec<SessionMetadata>, String>, delete(name) -> Result<(), String>}`.
  - `SessionStore::with_backend(Arc<dyn SessionBackend>) -> Self`; `SessionStore::new(root)` builds `FsSessionBackend`. `SessionStore` read methods keep **sync** signatures.

**Key design:** the worker thread already owns a `current_thread` tokio runtime running `writer_loop` under `block_on`, so `drain_pending` awaits `backend.save(&session).await` directly. Reads route through new round-trip messages so the public API stays sync (avoids `block_on`-inside-`#[tokio::main]` panic).

- [ ] **Step 1: Write the failing tests (backend-driven writer)**

Replace the fs-path assumptions in `crates/caliban-sessions/src/debounced.rs` unit tests. Add a shared in-memory test backend at the top of the `#[cfg(test)] mod tests`:

```rust
    use crate::backend::SessionBackend;
    use crate::session::PersistedSession;
    use crate::store::SessionMetadata;
    use std::collections::HashMap as StdHashMap;
    use std::sync::Arc;

    #[derive(Default)]
    struct MemBackend {
        map: Mutex<StdHashMap<String, PersistedSession>>,
        fail: Mutex<bool>,
    }
    #[async_trait::async_trait]
    impl SessionBackend for MemBackend {
        async fn save(&self, session: &PersistedSession) -> Result<(), crate::error::Error> {
            if *self.fail.lock().unwrap() {
                return Err(crate::error::Error::Persist("boom".into()));
            }
            self.map
                .lock()
                .unwrap()
                .insert(session.name.clone(), session.clone());
            Ok(())
        }
        async fn load(&self, name: &str) -> Result<Option<PersistedSession>, crate::error::Error> {
            Ok(self.map.lock().unwrap().get(name).cloned())
        }
        async fn list(&self) -> Result<Vec<SessionMetadata>, crate::error::Error> {
            Ok(self
                .map
                .lock()
                .unwrap()
                .values()
                .map(SessionMetadata::from_session)
                .collect())
        }
        async fn delete(&self, name: &str) -> Result<(), crate::error::Error> {
            self.map.lock().unwrap().remove(name);
            Ok(())
        }
    }

    fn sess(name: &str) -> PersistedSession {
        PersistedSession::new(name, "anthropic", "m")
    }

    #[test]
    fn single_write_lands_after_flush() {
        let be = Arc::new(MemBackend::default());
        let w = DebouncedWriter::with_window(Arc::clone(&be) as Arc<dyn SessionBackend>, TEST_WINDOW);
        w.request(sess("a"));
        w.flush().unwrap();
        assert!(be.map.lock().unwrap().contains_key("a"));
    }

    #[test]
    fn writes_within_window_collapse_to_latest() {
        let be = Arc::new(MemBackend::default());
        let w = DebouncedWriter::with_window(
            Arc::clone(&be) as Arc<dyn SessionBackend>,
            Duration::from_millis(150),
        );
        let mut s1 = sess("a");
        s1.model = "v1".into();
        let mut s3 = sess("a");
        s3.model = "v3".into();
        w.request(s1);
        w.request(sess("a"));
        w.request(s3);
        assert!(be.map.lock().unwrap().is_empty());
        w.flush().unwrap();
        assert_eq!(be.map.lock().unwrap().get("a").unwrap().model, "v3");
    }

    #[test]
    fn drop_drains_pending() {
        let be = Arc::new(MemBackend::default());
        {
            let w = DebouncedWriter::with_window(
                Arc::clone(&be) as Arc<dyn SessionBackend>,
                Duration::from_mins(1),
            );
            w.request(sess("a"));
        }
        assert!(be.map.lock().unwrap().contains_key("a"));
    }

    #[test]
    fn flush_surfaces_backend_failure() {
        let be = Arc::new(MemBackend::default());
        *be.fail.lock().unwrap() = true;
        let w = DebouncedWriter::with_window(Arc::clone(&be) as Arc<dyn SessionBackend>, TEST_WINDOW);
        w.request(sess("a"));
        assert!(w.flush().is_err());
        assert!(w.last_error().is_some());
    }

    #[test]
    fn flush_from_inside_tokio_runtime_does_not_panic() {
        let be = Arc::new(MemBackend::default());
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let w =
                DebouncedWriter::with_window(Arc::clone(&be) as Arc<dyn SessionBackend>, Duration::from_mins(1));
            w.request(sess("a"));
            w.flush().unwrap();
        });
        assert!(be.map.lock().unwrap().contains_key("a"));
    }

    #[test]
    fn read_roundtrip_through_worker() {
        let be = Arc::new(MemBackend::default());
        let w = DebouncedWriter::with_window(Arc::clone(&be) as Arc<dyn SessionBackend>, TEST_WINDOW);
        w.request(sess("a"));
        // load() flushes pending first, then reads through the backend.
        let got = w.load("a").unwrap();
        assert!(got.is_some());
        assert_eq!(w.list().unwrap().len(), 1);
        w.delete("a").unwrap();
        assert!(w.load("a").unwrap().is_none());
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p caliban-sessions debounced::tests`
Expected: FAIL to compile — `DebouncedWriter::new`/`with_window` don't take a backend, `request` takes `(PathBuf, Vec<u8>)`, and `load`/`list`/`delete` don't exist.

- [ ] **Step 3: Rework `DebouncedWriter`**

In `crates/caliban-sessions/src/debounced.rs`:

1. Change `PersistRequest` to carry a session:

```rust
struct PersistRequest {
    session: PersistedSession,
}
```

2. Extend `WriterMsg` with read round-trips (each carries a `std::sync::mpsc::Sender` of a typed result; `String` is the error, matching `flush`'s existing convention):

```rust
enum WriterMsg {
    Persist(PersistRequest),
    Flush(std::sync::mpsc::Sender<Result<(), String>>),
    Load(String, std::sync::mpsc::Sender<Result<Option<PersistedSession>, String>>),
    List(std::sync::mpsc::Sender<Result<Vec<SessionMetadata>, String>>),
    Delete(String, std::sync::mpsc::Sender<Result<(), String>>),
}
```

3. `WriterInner`/`DebouncedWriter` gain `backend: Arc<dyn SessionBackend>`, threaded into `run_writer_thread`/`writer_loop`. Constructors:

```rust
impl DebouncedWriter {
    pub(crate) fn new(backend: Arc<dyn SessionBackend>) -> Self {
        Self::with_window_and_max_delay(backend, DEBOUNCE_WINDOW, MAX_DELAY)
    }

    #[cfg(test)]
    pub(crate) fn with_window(backend: Arc<dyn SessionBackend>, window: Duration) -> Self {
        Self::with_window_and_max_delay(backend, window, MAX_DELAY)
    }

    pub(crate) fn with_window_and_max_delay(
        backend: Arc<dyn SessionBackend>,
        window: Duration,
        max_delay: Duration,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel::<WriterMsg>();
        let last_error: LastError = Arc::new(Mutex::new(None));
        let last_error_worker = Arc::clone(&last_error);
        let backend_worker = Arc::clone(&backend);
        let thread = std::thread::Builder::new()
            .name("caliban-session-writer".into())
            .spawn(move || {
                run_writer_thread(rx, window, max_delay, &last_error_worker, backend_worker);
            })
            .expect("spawn session writer thread");
        Self {
            inner: Arc::new(WriterInner {
                tx,
                last_error,
                thread: Mutex::new(Some(thread)),
            }),
        }
    }

    pub(crate) fn request(&self, session: PersistedSession) {
        let _ = self.inner.tx.send(WriterMsg::Persist(PersistRequest { session }));
    }
```

4. Change `LastError` to key by name: `type LastError = Arc<Mutex<Option<(String, String)>>>;` (`(name, message)`), and update `do_write` to key on `session.name`.

5. Add the read round-trips on the handle:

```rust
    pub(crate) fn load(&self, name: &str) -> Result<Option<PersistedSession>, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        if self.inner.tx.send(WriterMsg::Load(name.to_string(), tx)).is_err() {
            return Ok(None);
        }
        rx.recv().unwrap_or(Ok(None))
    }

    pub(crate) fn list(&self) -> Result<Vec<SessionMetadata>, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        if self.inner.tx.send(WriterMsg::List(tx)).is_err() {
            return Ok(Vec::new());
        }
        rx.recv().unwrap_or(Ok(Vec::new()))
    }

    pub(crate) fn delete(&self, name: &str) -> Result<(), String> {
        let (tx, rx) = std::sync::mpsc::channel();
        if self.inner.tx.send(WriterMsg::Delete(name.to_string(), tx)).is_err() {
            return Ok(());
        }
        rx.recv().unwrap_or(Ok(()))
    }
```

6. `writer_loop` gets `backend: Arc<dyn SessionBackend>`. Buffer becomes `HashMap<String, PersistedSession>` keyed by `session.name`. In both the idle-`recv` and `select!` arms, handle the new messages. The read arms **drain pending first** (so reads see the latest), then await the backend:

```rust
                Some(WriterMsg::Load(name, done)) => {
                    let _ = drain_pending(&mut pending, last_error, &backend).await;
                    oldest_dirty = None;
                    let r = backend.load(&name).await.map_err(|e| e.to_string());
                    let _ = done.send(r);
                }
                Some(WriterMsg::List(done)) => {
                    let _ = drain_pending(&mut pending, last_error, &backend).await;
                    oldest_dirty = None;
                    let r = backend.list().await.map_err(|e| e.to_string());
                    let _ = done.send(r);
                }
                Some(WriterMsg::Delete(name, done)) => {
                    let _ = drain_pending(&mut pending, last_error, &backend).await;
                    oldest_dirty = None;
                    let r = backend.delete(&name).await.map_err(|e| e.to_string());
                    let _ = done.send(r);
                }
```

7. `drain_pending`/`do_write` become `async` and call `backend.save`:

```rust
async fn drain_pending(
    pending: &mut HashMap<String, PersistedSession>,
    last_error: &LastError,
    backend: &Arc<dyn SessionBackend>,
) -> Result<(), String> {
    let mut first_err: Option<String> = None;
    let drained: Vec<PersistedSession> = pending.drain().map(|(_, v)| v).collect();
    for session in drained {
        if let Err(msg) = do_write(&session, last_error, backend).await {
            first_err.get_or_insert(msg);
        }
    }
    first_err.map_or(Ok(()), Err)
}

async fn do_write(
    session: &PersistedSession,
    last_error: &LastError,
    backend: &Arc<dyn SessionBackend>,
) -> Result<(), String> {
    match backend.save(session).await {
        Ok(()) => {
            if let Ok(mut slot) = last_error.lock()
                && slot.as_ref().is_some_and(|(n, _)| n == &session.name)
            {
                *slot = None;
            }
            Ok(())
        }
        Err(e) => {
            let msg = e.to_string();
            tracing::warn!(
                target: caliban_common::tracing_targets::TARGET_SESSIONS,
                error = %msg,
                session = %session.name,
                "debounced session write failed",
            );
            if let Ok(mut slot) = last_error.lock() {
                *slot = Some((session.name.clone(), msg.clone()));
            }
            Err(msg)
        }
    }
}
```

(Every `drain_pending(...)` call site in `writer_loop` gains `.await` and the `&backend` arg. The `run_writer_thread` signature gains `backend: Arc<dyn SessionBackend>` and passes it to `writer_loop`.)

- [ ] **Step 4: Rewire `SessionStore` onto the backend**

In `crates/caliban-sessions/src/store.rs`:

```rust
use std::sync::Arc;

use crate::backend::{FsSessionBackend, SessionBackend};
use crate::debounced::DebouncedWriter;
use crate::error::{Error, Result};
use crate::session::PersistedSession;

impl SessionStore {
    /// Construct a store backed by the filesystem at `root` (the default).
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self::with_backend(Arc::new(FsSessionBackend::new(root)))
    }

    /// Construct a store over an arbitrary [`SessionBackend`] (e.g. a gonzalo
    /// substrate). Spawns the shared debounced writer over that backend.
    #[must_use]
    pub fn with_backend(backend: Arc<dyn SessionBackend>) -> Self {
        Self {
            inner: Arc::new(StoreInner {
                writer: DebouncedWriter::new(backend),
            }),
        }
    }

    pub fn load(&self, name: &str) -> Result<Option<PersistedSession>> {
        self.inner.writer.load(name).map_err(Error::Persist)
    }

    pub fn save(&self, session: &PersistedSession) -> Result<()> {
        self.inner.writer.request(session.clone());
        Ok(())
    }

    pub fn flush(&self) -> Result<()> {
        self.inner.writer.flush().map_err(Error::Persist)
    }

    #[must_use]
    pub fn last_write_error(&self) -> Option<String> {
        self.inner.writer.last_error()
    }

    pub fn list(&self) -> Result<Vec<SessionMetadata>> {
        self.inner.writer.list().map_err(Error::Persist)
    }

    pub fn delete(&self, name: &str) -> Result<()> {
        self.inner.writer.delete(name).map_err(Error::Persist)
    }
}
```

`StoreInner` drops the `root: PathBuf` field (the backend owns the location now). `default_root()`, `path_for()` — **keep `default_root()`** (callers `overlay.rs:515`, `diagnostics.rs:149`, and `compose.rs` still resolve the default sessions dir through it). **Remove `path_for()`** (it was only used by the old inline fs writes) — but first grep; if any caller outside `store.rs` uses it, keep it delegating to an `FsSessionBackend`-style join. Move `validate_name` usage: `store.rs` no longer validates (the backend does), so drop the local `validate_name`/`MAX_NAME_LEN` from `store.rs` (now in `fs.rs`, Task 2). Note `save` no longer needs `create_dir_all`/serialize — the backend does that.

- [ ] **Step 5: Run to verify all pass**

Run: `cargo test -p caliban-sessions`
Expected: PASS (writer unit tests + fs conformance + store tests + integration `tests/debounced.rs` — update that integration test's construction to `SessionStore::new(dir)` which still works, and any assertion that inspected raw files still holds for the fs backend).

- [ ] **Step 6: Commit**

```bash
git add crates/caliban-sessions/src/debounced.rs crates/caliban-sessions/src/store.rs
git commit -m "feat(sessions): route SessionStore/DebouncedWriter through SessionBackend (#471)"
```

---

### Task 4: `GonzaloSessionBackend` (feature-gated OCC get→put)

**Files:**
- Create: `crates/caliban-sessions/src/backend/gonzalo.rs`
- Modify: `crates/caliban-sessions/src/backend/mod.rs` (already wires the `#[cfg]` module in Task 1)
- Modify: `crates/caliban-sessions/src/lib.rs` (feature-gated export)
- Modify: `crates/caliban-sessions/Cargo.toml`

**Interfaces:**
- Consumes: `SessionBackend`, `PersistedSession`, `SessionMetadata::from_session`, `Error::{Conflict, Backend/Persist}`. gonzalo-core `0.3`: `Body, DeleteResult, Identity, KeyPrefix, Meta, PutResult, Record, RecordKey, RecordKind, Revision, Store`.
- Produces: `GonzaloSessionBackend::new(store: Arc<dyn Store>, workspace_slug: impl Into<String>)`.

**Precedent to copy nearly verbatim:** `crates/caliban-memory/src/backend/gonzalo.rs` (OCC get→put, `resolve_author`, `now_millis`, `meta_now`, per-record parse). The only differences: `RecordKind::Session`, collection `sessions:<slug>`, body is the whole `PersistedSession` JSON, and list derives `SessionMetadata` (no index).

- [ ] **Step 1: Add the `gonzalo` feature to `caliban-sessions/Cargo.toml`**

```toml
[dependencies]
# ... existing ...
async-trait      = { workspace = true }
gonzalo-core     = { version = "0.3", optional = true }
gonzalo-store-fs = { version = "0.3", optional = true }

[features]
gonzalo = ["dep:gonzalo-core", "dep:gonzalo-store-fs"]

[dev-dependencies]
# ... existing tempfile, tokio ...
```

(Add `async-trait` to `[dependencies]` if not already present — Task 1 needs it regardless; confirm it's there.)

- [ ] **Step 2: Write the failing gonzalo tests**

Create `crates/caliban-sessions/src/backend/gonzalo.rs` ending with:

```rust
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
        g.save(&PersistedSession::new("z", "anthropic", "m")).await.unwrap();
        // Drive a raw conflict: put expected=None on an existing key.
        let key = g.key("z");
        let rec = g.session_to_record(&PersistedSession::new("z", "anthropic", "m")).unwrap();
        let r = g.store.put(rec, None).await.unwrap();
        assert!(matches!(r, gonzalo_core::PutResult::Conflict(_)));
    }
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p caliban-sessions --features gonzalo backend::gonzalo`
Expected: FAIL to compile — `GonzaloSessionBackend` undefined.

- [ ] **Step 4: Implement `GonzaloSessionBackend`**

Prepend to `crates/caliban-sessions/src/backend/gonzalo.rs`:

```rust
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
        Body::Blob { .. } => return Err(Error::Persist("unexpected blob body for session".into())),
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
                    Err(e) => tracing::warn!(key = %key, error = %e, "skipping unparseable session record"),
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
```

Add to `crates/caliban-sessions/src/lib.rs`:

```rust
#[cfg(feature = "gonzalo")]
pub use backend::GonzaloSessionBackend;
```

(`validate_name` in `fs.rs` must be `pub(crate)` — confirm from Task 2. `Body::bytes()` exists in gonzalo-core 0.3, mirroring memory's usage.)

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -p caliban-sessions --features gonzalo`
Expected: PASS (gonzalo conformance + roundtrip + conflict). Also confirm the default build is unaffected: `cargo test -p caliban-sessions` (no feature) PASS, and `cargo build -p caliban-sessions` compiles with **no** gonzalo symbols.

- [ ] **Step 6: Commit**

```bash
git add crates/caliban-sessions/src/backend/gonzalo.rs \
        crates/caliban-sessions/src/lib.rs \
        crates/caliban-sessions/Cargo.toml
git commit -m "feat(sessions): GonzaloSessionBackend (OCC get->put, RecordKind::Session) (#471)"
```

---

### Task 5: `build_session_backend` factory

**Files:**
- Modify: `caliban/src/startup/storage.rs`
- Modify: `caliban/Cargo.toml`

**Interfaces:**
- Consumes: `StorageConfig`/`StorageSubstrate` (caliban-settings, unchanged), `FsSessionBackend`/`SessionBackend` (caliban-sessions), `remote_store`/`workspace_slug` (existing helpers in `storage.rs`, already `#[cfg(feature = "gonzalo")]`).
- Produces: `pub(crate) async fn build_session_backend(storage: &StorageConfig, sessions_dir: &Path) -> Result<Arc<dyn SessionBackend>, String>`.

- [ ] **Step 1: Add the caliban-sessions gonzalo passthrough to `caliban/Cargo.toml`**

Change the `gonzalo` feature line to also enable the sessions crate's feature:

```toml
gonzalo = ["caliban-memory/gonzalo", "caliban-sessions/gonzalo", "dep:gonzalo-core", "dep:gonzalo-store-server"]
```

- [ ] **Step 2: Write the failing factory tests**

Append to `caliban/src/startup/storage.rs` `#[cfg(test)] mod tests`:

```rust
    #[tokio::test]
    async fn session_fs_builds_without_feature() {
        let tmp = tempfile::tempdir().unwrap();
        let be = build_session_backend(&cfg(StorageSubstrate::Fs), tmp.path())
            .await
            .unwrap();
        assert!(be.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn session_git_and_s3_error_as_not_wired() {
        let tmp = tempfile::tempdir().unwrap();
        for sub in [StorageSubstrate::Git, StorageSubstrate::S3] {
            let e = build_session_backend(&cfg(sub), tmp.path())
                .await
                .err()
                .unwrap();
            assert!(e.contains("not wired"), "got: {e}");
        }
    }

    #[cfg(not(feature = "gonzalo"))]
    #[tokio::test]
    async fn session_remote_without_feature_errors_clearly() {
        let tmp = tempfile::tempdir().unwrap();
        let e = build_session_backend(&cfg(StorageSubstrate::Remote), tmp.path())
            .await
            .err()
            .unwrap();
        assert!(e.contains("--features gonzalo"), "got: {e}");
    }
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p caliban startup::storage::tests::session_fs_builds_without_feature`
Expected: FAIL to compile — `build_session_backend` undefined.

- [ ] **Step 4: Implement `build_session_backend`**

Add to `caliban/src/startup/storage.rs` (import `caliban_sessions::{FsSessionBackend, SessionBackend}`):

```rust
/// Build the session backend the config selects. Mirrors `build_topic_backend`.
/// `fs` is always available; `remote` requires the `gonzalo` feature; `git`/`s3`
/// are recognized but not wired yet (#469). Errors are fatal config errors.
pub(crate) async fn build_session_backend(
    storage: &StorageConfig,
    sessions_dir: &Path,
) -> Result<Arc<dyn SessionBackend>, String> {
    match storage.substrate {
        StorageSubstrate::Fs => Ok(Arc::new(FsSessionBackend::new(sessions_dir))),
        StorageSubstrate::Remote => build_remote_session_backend(storage, sessions_dir).await,
        other @ (StorageSubstrate::Git | StorageSubstrate::S3) => Err(format!(
            "storage.substrate {other:?} is recognized but not wired yet (tracked in #469); use fs or remote"
        )),
    }
}

#[cfg(not(feature = "gonzalo"))]
#[allow(clippy::unused_async)]
async fn build_remote_session_backend(
    _storage: &StorageConfig,
    _sessions_dir: &Path,
) -> Result<Arc<dyn SessionBackend>, String> {
    Err("this build lacks gonzalo support; rebuild with `--features gonzalo` to use a remote substrate".to_string())
}

#[cfg(feature = "gonzalo")]
async fn build_remote_session_backend(
    storage: &StorageConfig,
    sessions_dir: &Path,
) -> Result<Arc<dyn SessionBackend>, String> {
    use caliban_sessions::GonzaloSessionBackend;
    let store = remote_store(storage)?;
    let slug = workspace_slug(sessions_dir);
    let backend = GonzaloSessionBackend::new(store, slug);
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
```

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -p caliban startup::storage` then `cargo test -p caliban --features gonzalo startup::storage`
Expected: PASS both (fs/git/s3/remote-without-feature; gonzalo probe reuses the existing helpers).

- [ ] **Step 6: Commit**

```bash
git add caliban/src/startup/storage.rs caliban/Cargo.toml
git commit -m "feat(config): build_session_backend factory (fs/remote/not-wired) (#471)"
```

---

### Task 6: Wire the factory into startup + threading + restore regression

**Files:**
- Modify: `caliban/src/startup/compose.rs:1575-1595` (`resolve_session`, add `session_store_needed`)
- Modify: `caliban/src/main.rs` (build the session backend, exit 78, thread in)
- Modify: `caliban/src/startup/drivers.rs:314-326` (`resolve_resume` fallback)
- Create/Modify: a checkpoint-restore-over-gonzalo regression test.

**Interfaces:**
- Consumes: `build_session_backend` (Task 5), `SessionStore::with_backend` (Task 3), `SessionBackend`.
- Produces: `pub(crate) fn session_store_needed(args: &Args) -> bool`; `resolve_session` accepts `session_backend: Option<Arc<dyn SessionBackend>>`.

- [ ] **Step 1: Extract the `needs_store` predicate**

In `caliban/src/startup/compose.rs`, add above `resolve_session`:

```rust
/// Whether any flag requires a session store: `--session`, `--continue`, or
/// `--resume`. Mirrors the gate that used to live inline in `resolve_session`,
/// so `main` can build the backend (which may probe a remote daemon) only when
/// a session is actually needed.
pub(crate) fn session_store_needed(args: &Args) -> bool {
    args.session.is_some() || args.continue_latest || args.resume.is_some()
}
```

- [ ] **Step 2: Change `resolve_session` to accept an injected backend**

```rust
pub(crate) fn resolve_session(
    args: &Args,
    model: &str,
    todos: &caliban_agent_core::SharedTodos,
    plan_mode: &caliban_agent_core::SharedPlanMode,
    session_backend: Option<Arc<dyn caliban_sessions::SessionBackend>>,
) -> Result<(Option<SessionStore>, Option<PersistedSession>)> {
    let store = session_backend.map(SessionStore::with_backend);
    // ... rest unchanged (the `let session = if let (Some(store), ...)` block
    //     and todos/plan_mode hydration stay exactly as they are) ...
```

(Delete the old `needs_store`/`SessionStore::new(...)` block — the backend is now built by `main` and passed in.)

- [ ] **Step 3: Build the backend in `main.rs` (exit 78 on error) and thread it**

At the `main.rs` site that calls `compose::resolve_session` (currently `main.rs:531`), build the backend first. The sessions dir mirrors the old resolution (`args.sessions_dir` else `SessionStore::default_root()?`):

```rust
let session_backend: Option<std::sync::Arc<dyn caliban_sessions::SessionBackend>> =
    if startup::compose::session_store_needed(&args) && !args.bare {
        let sessions_dir = match &args.sessions_dir {
            Some(d) => d.clone(),
            None => caliban_sessions::SessionStore::default_root()
                .map_err(|e| { eprintln!("[caliban] sessions dir error: {e}"); std::process::exit(78); })
                .unwrap(),
        };
        match startup::storage::build_session_backend(
            &settings_outcome.settings.storage,
            &sessions_dir,
        )
        .await
        {
            Ok(b) => Some(b),
            Err(e) => {
                eprintln!("[caliban] storage config error: {e}");
                std::process::exit(78);
            }
        }
    } else {
        None
    };

let (store, session) =
    startup::compose::resolve_session(&args, &model, &todos, &plan_mode, session_backend)?;
```

(Use the same `settings_outcome.settings.storage` value the memory factory reads. If `--bare`, `session_backend` is `None`, so `resolve_session` yields no store — matching the memory factory's `--bare` skip.)

- [ ] **Step 4: Fix the `resolve_resume` fallback in `drivers.rs`**

The defensive fallback (`drivers.rs:323`) builds a fresh `SessionStore::new(default_root)` when the passed store is `None` but a resume flag is set. With Step 3, `main` already builds the store whenever `session_store_needed` (which includes `--resume`), so this path is now only reached in tests or `--bare`. Keep it as an **fs** fallback (a last-resort local store) and add a comment:

```rust
// Last-resort local fallback: main builds the configured backend whenever a
// session is needed, so this only triggers in --bare/test paths. fs is the
// safe default here (no remote probe from a fallback).
```

No functional change needed beyond the comment; if it currently returns `Result`, leave it.

- [ ] **Step 5: Write the checkpoint-restore-over-gonzalo regression test**

Create `caliban/tests/session_gonzalo_restore.rs`:

```rust
//! #471 regression: a session persisted via the gonzalo backend loads and
//! restores through caliban-checkpoint's in-memory PersistedSession path.
#![cfg(feature = "gonzalo")]

use std::sync::Arc;

use caliban_sessions::{GonzaloSessionBackend, PersistedSession, SessionBackend};
use gonzalo_store_fs::FsStore;

#[tokio::test]
async fn gonzalo_persisted_session_loads_for_restore() {
    let tmp = tempfile::tempdir().unwrap();
    let be = GonzaloSessionBackend::new(Arc::new(FsStore::new(tmp.path().to_path_buf())), "ws");
    let mut s = PersistedSession::new("resume-me", "anthropic", "m");
    s.model = "claude-x".into();
    be.save(&s).await.unwrap();

    // Load back exactly as the restore path consumes it (a &mut PersistedSession).
    let mut loaded = be.load("resume-me").await.unwrap().expect("present");
    assert_eq!(loaded.name, "resume-me");
    assert_eq!(loaded.model, "claude-x");
    // caliban-checkpoint::restore mutates the in-memory session; assert it is a
    // normal owned value the restore signature accepts.
    loaded.messages.clear();
    assert!(loaded.messages.is_empty());
}
```

(If `caliban/tests/` needs `gonzalo-store-fs` as a dev-dep, it is already present in `caliban/Cargo.toml:88`.)

- [ ] **Step 6: Run the full gate**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features gonzalo -- -D warnings
cargo build --workspace --all-targets
cargo test --workspace
cargo test -p caliban --features gonzalo
```
Expected: all green. Also spot-check publish: `cargo publish -p caliban-sessions --dry-run` and `cargo publish -p caliban-settings --dry-run` green (no gonzalo symbols in default build).

- [ ] **Step 7: Commit**

```bash
git add caliban/src/startup/compose.rs caliban/src/main.rs \
        caliban/src/startup/drivers.rs caliban/tests/session_gonzalo_restore.rs
git commit -m "feat(sessions): wire session backend factory into startup + restore regression (#471)"
```

---

## Closing notes (out-of-band, not a code task)

- **File a gonzalo follow-up issue** (per the design ruling): `RecordKind::Session`
  declares `MergeClass::AppendOnly`, but caliban rewrites session bodies wholesale,
  so a future background reconciler would need `Session` to be `Opaque` (or a new
  opaque-classed session kind). Inert under OCC `put` in gonzalo 0.3 — conflicts
  surface correctly today. Create it with `gh issue create --repo caliban-ai/gonzalo`
  during ship, and cross-link from #471.
- The interactive TUI session commands (`/resume`, `/status` sessions dir) already
  route through `SessionStore`, so they inherit the configured backend for `list`;
  no separate follow-up (contrast memory's #501, which had raw-fs subcommands).

## Self-Review

**Spec coverage:** SessionBackend trait (T1) ✓ · FsSessionBackend (T2) ✓ · async debounce rework + sync read round-trip + Conflict error (T3) ✓ · GonzaloSessionBackend OCC/RecordKind::Session/workspace-scoped key (T4) ✓ · build_session_backend factory + Cargo features (T5) ✓ · startup wiring + exit 78 + --bare skip + restore regression (T6) ✓ · gonzalo follow-up (closing notes) ✓ · keep-debouncing decision (architecture) ✓.

**Placeholder scan:** no TBD/TODO; every code step carries complete code. The `path_for` removal in T3 is conditional on a grep (documented, not a placeholder).

**Type consistency:** `SessionMetadata::from_session` defined T2, used T2/T3/T4 ✓ · `validate_name` `pub(crate)` in `fs.rs` T2, reused T4 ✓ · `DebouncedWriter::{new,with_window}(backend, ...)` signature consistent T3 ✓ · `Error::Conflict { name }` T1, produced T4 ✓ · `build_session_backend(&StorageConfig, &Path) -> Result<Arc<dyn SessionBackend>, String>` T5, called T6 ✓ · `resolve_session(..., Option<Arc<dyn SessionBackend>>)` T6 ✓.
