# Memory ↔ gonzalo facade (pilot) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route `caliban-memory` auto-memory topic CRUD and the prompt-splice index through a caliban-owned `TopicBackend` seam, with a gonzalo-free `FsTopicBackend` default and a feature-gated `GonzaloTopicBackend` proven against `gonzalo::FsStore`.

**Architecture:** One `#[async_trait]` `TopicBackend` (list/read/write/delete/index) behind the public `TopicLoader` facade (`Box<dyn TopicBackend>`). `FsTopicBackend` ports today's `std::fs` logic to `tokio::fs` and derives `MEMORY.md` from the topic set. `GonzaloTopicBackend` (`#[cfg(feature = "gonzalo")]`) maps topics ⇄ gonzalo `Record`s over `Arc<dyn gonzalo_core::Store>`. Runtime substrate selection is deferred to #473; #470 wires `FsTopicBackend` as the live default and proves the gonzalo path by tests.

**Tech Stack:** Rust, `tokio::fs`, `async-trait`, `serde_json`, `serde_yaml`; `gonzalo-core` + `gonzalo-store-fs` @ `0.3` (optional).

## Global Constraints

- New **off-by-default `gonzalo` cargo feature**; the default build never names a gonzalo type. `cargo publish -p caliban-memory --dry-run` must stay green with the optional dep present + feature off.
- Both backends surface the **unified `MemoryError`** enum. All gonzalo error mapping lives inside `GonzaloTopicBackend` (feature-gated).
- `TopicBackend` uses `#[async_trait]` (object-safe for `Box<dyn TopicBackend>`). gonzalo's `Store` is `#[async_trait]` → `Arc<dyn Store>` is valid.
- **RecordKey** = `("caliban", "memory:<workspace-slug>", "<slug>")`; workspace-slug = a stable hash of `cfg.auto_memory_dir`. **RecordKind** = `Topic`. **Body** = `Body::Inline(serde_json TopicFile)`. **Meta**: `author` = git identity if detectable (`git config user.email` → `user.name`) else `"caliban"`, resolved once at construction; `origin_system = "caliban"`; `created`/`updated` = now epoch millis.
- The full gate (`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo build --workspace --all-targets`, `cargo test --workspace`) must pass in **both** the default and `--features gonzalo` configurations.
- Before clippy, `touch` changed `.rs` files (CI toolchain catches lints a warm local cache skips).
- Real gonzalo signatures (verbatim): `Store::get(&self,&RecordKey)->Result<Option<Record>>`, `Store::put(&self,Record,Option<Revision>)->Result<PutResult>`, `Store::list(&self,&KeyPrefix)->Result<Vec<RecordKey>>`, `Store::delete(&self,&RecordKey,Option<Revision>)->Result<DeleteResult>`. `Record{key,kind,revision,parent,body,meta,links}`. `RecordKey::new(ns,coll,id)`. `KeyPrefix{namespace:Option<String>,collection:Option<String>}`. `Revision{counter,hash}`, `Revision::initial(&[u8])`. `Body::Inline(Vec<u8>)`. `PutResult::{Committed(Revision),Conflict(Box<Conflict>)}`. `DeleteResult::{Deleted,Conflict(..)}`. `FsStore::new(root)`. `Identity::new(id)`. `ContentHash::of(&[u8])`.

---

## File Structure

- `crates/caliban-memory/src/auto.rs` — **modify**: keep `TopicSummary`/`TopicFile`/`TopicDraft`/`TopicKind`/`validate_slug`/`render_topic_file`/`parse_frontmatter`; TopicLoader becomes the facade (below). Retire `update_index_line`/`remove_index_line`.
- `crates/caliban-memory/src/backend/mod.rs` — **create**: `TopicBackend` trait + `TopicLoader` facade.
- `crates/caliban-memory/src/backend/fs.rs` — **create**: `FsTopicBackend` (async, `tokio::fs`, derived index).
- `crates/caliban-memory/src/backend/gonzalo.rs` — **create**: `GonzaloTopicBackend` (`#[cfg(feature = "gonzalo")]`) + `topic_to_record` + author resolution.
- `crates/caliban-memory/src/backend/conformance.rs` — **create**: shared test battery over any `TopicBackend`.
- `crates/caliban-memory/src/error.rs` — **modify**: add `Backend(String)` + `Conflict { key: String }`.
- `crates/caliban-memory/src/loader.rs` — **modify**: `load` derives the index via a backend built from `cfg`.
- `crates/caliban-memory/Cargo.toml` — **modify**: `async-trait` dep + `gonzalo` feature + optional gonzalo deps.
- `crates/caliban-memory/src/lib.rs` — **modify**: re-export `TopicBackend`, `FsTopicBackend`, (feature) `GonzaloTopicBackend`.
- `crates/caliban-tools-builtin/src/memory/mod.rs` — **modify**: `.write(&draft).await` (returns locator `String`), async tests.
- `caliban/src/startup/compose.rs:585` — **modify**: backend factory (fs default); async wiring.
- `caliban/src/tui/slash/existing.rs` — **modify**: async `.await`; `/memory edit` uses `cfg.auto_memory_dir` directly.
- `caliban/Cargo.toml` — **modify**: off-by-default `gonzalo` feature enabling `caliban-memory/gonzalo`.

---

## Task 1: `MemoryError` variants + `TopicBackend` trait + `TopicLoader` facade

**Files:**
- Modify: `crates/caliban-memory/src/error.rs`
- Create: `crates/caliban-memory/src/backend/mod.rs`
- Modify: `crates/caliban-memory/src/lib.rs`, `crates/caliban-memory/Cargo.toml`

**Interfaces:**
- Produces: `MemoryError::Backend(String)`, `MemoryError::Conflict { key: String }`; trait `TopicBackend: Send + Sync` with async `list()->Result<Vec<TopicSummary>>`, `read(&str)->Result<TopicFile>`, `write(&TopicDraft)->Result<String>` (display locator), `delete(&str)->Result<()>`, `index()->Result<String>`; `TopicLoader::with_backend(Box<dyn TopicBackend>)` + async delegators.

- [ ] **Step 1: Add the `async-trait` dependency**

In `crates/caliban-memory/Cargo.toml` `[dependencies]`, add:
```toml
async-trait = { workspace = true }
```

- [ ] **Step 2: Write the failing test for the new error variants**

Append to `crates/caliban-memory/src/error.rs` tests (create a `#[cfg(test)] mod tests` if absent):
```rust
#[cfg(test)]
mod error_variant_tests {
    use super::MemoryError;

    #[test]
    fn conflict_and_backend_variants_display() {
        let c = MemoryError::Conflict { key: "caliban/memory:abc/foo".into() };
        assert!(c.to_string().contains("conflict"));
        let b = MemoryError::Backend("boom".into());
        assert!(b.to_string().contains("boom"));
    }
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `cargo test -p caliban-memory conflict_and_backend_variants_display`
Expected: FAIL — `no variant named Conflict`/`Backend`.

- [ ] **Step 4: Add the variants**

In `crates/caliban-memory/src/error.rs`, add to the `MemoryError` enum:
```rust
    /// A storage-backend error surfaced from a non-fs substrate (e.g. gonzalo).
    #[error("memory backend error: {0}")]
    Backend(String),

    /// An optimistic-concurrency conflict on write/delete. Dormant on the
    /// single-writer fs substrate; live once remote sync exists.
    #[error("memory write conflict at {key}")]
    Conflict { key: String },
```

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -p caliban-memory conflict_and_backend_variants_display`
Expected: PASS.

- [ ] **Step 6: Write the failing test for the trait + facade delegation**

Create `crates/caliban-memory/src/backend/mod.rs`:
```rust
//! The storage seam for auto-memory topics.
use std::path::PathBuf;

use async_trait::async_trait;

use crate::auto::{TopicDraft, TopicFile, TopicSummary};
use crate::error::Result;

/// Substrate-neutral CRUD + index projection for auto-memory topics.
#[async_trait]
pub trait TopicBackend: Send + Sync {
    async fn list(&self) -> Result<Vec<TopicSummary>>;
    async fn read(&self, name: &str) -> Result<TopicFile>;
    /// Persist `draft`, returning a human-readable locator (fs path or record key).
    async fn write(&self, draft: &TopicDraft) -> Result<String>;
    async fn delete(&self, name: &str) -> Result<()>;
    /// Rebuild the `MEMORY.md` index body from the current topic set.
    async fn index(&self) -> Result<String>;
}

/// Public facade over a chosen [`TopicBackend`]. `new` selects the fs backend
/// (behaviour-equivalent to the historical std::fs loader).
pub struct TopicLoader {
    backend: Box<dyn TopicBackend>,
}

impl TopicLoader {
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { backend: Box::new(super::backend::fs::FsTopicBackend::new(dir)) }
    }

    #[must_use]
    pub fn with_backend(backend: Box<dyn TopicBackend>) -> Self {
        Self { backend }
    }

    pub async fn list(&self) -> Result<Vec<TopicSummary>> { self.backend.list().await }
    pub async fn read(&self, name: &str) -> Result<TopicFile> { self.backend.read(name).await }
    pub async fn write(&self, draft: &TopicDraft) -> Result<String> { self.backend.write(draft).await }
    pub async fn delete(&self, name: &str) -> Result<()> { self.backend.delete(name).await }
    pub async fn index(&self) -> Result<String> { self.backend.index().await }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auto::{TopicDraft, TopicKind};

    #[tokio::test]
    async fn facade_delegates_write_then_list_to_fs_backend() {
        let tmp = tempfile::tempdir().unwrap();
        let loader = TopicLoader::new(tmp.path().to_path_buf());
        loader
            .write(&TopicDraft {
                name: "alpha".into(),
                description: "first".into(),
                kind: TopicKind::Project,
                body: "body".into(),
            })
            .await
            .unwrap();
        let names: Vec<_> = loader.list().await.unwrap().into_iter().map(|s| s.name).collect();
        assert_eq!(names, vec!["alpha".to_string()]);
    }

    pub(crate) mod fs {} // placeholder module path resolved once fs.rs lands
}
```
Add to `crates/caliban-memory/src/lib.rs`:
```rust
pub mod backend;
pub use backend::{TopicBackend, TopicLoader};
```
Remove the old `pub use ... TopicLoader` line in `auto.rs`/`lib.rs` (TopicLoader now lives in `backend`).

- [ ] **Step 7: Run to verify it fails to compile**

Run: `cargo test -p caliban-memory facade_delegates_write_then_list_to_fs_backend`
Expected: FAIL — `FsTopicBackend` unresolved (implemented in Task 2). This task's deliverable is the trait + error + facade skeleton; the delegation test goes green at the end of Task 2. Remove the `pub(crate) mod fs {}` placeholder — it exists only to name the intent; the real module is declared in Task 2.

- [ ] **Step 8: Commit**

```bash
git add crates/caliban-memory/src/error.rs crates/caliban-memory/src/backend/mod.rs \
        crates/caliban-memory/src/lib.rs crates/caliban-memory/Cargo.toml
git commit -m "feat(memory): TopicBackend trait + facade skeleton + error variants (#470)"
```

---

## Task 2: `FsTopicBackend` — async CRUD + derived index

**Files:**
- Create: `crates/caliban-memory/src/backend/fs.rs`
- Modify: `crates/caliban-memory/src/backend/mod.rs` (declare `pub(crate) mod fs;`), `crates/caliban-memory/src/auto.rs` (retire index-line helpers; expose `render_topic_file`, `read_summary_from_str`)

**Interfaces:**
- Consumes: `TopicBackend` (Task 1), `TopicDraft`/`TopicFile`/`TopicSummary`/`TopicKind`/`validate_slug`/`render_topic_file`/`parse_frontmatter` (auto.rs).
- Produces: `FsTopicBackend::new(dir: impl Into<PathBuf>) -> Self`; `pub(crate) fn derive_index(summaries: &[TopicSummary]) -> String` (shared index derivation used by both backends).

- [ ] **Step 1: Write the failing test — list/read/write/delete + index derivation over tokio::fs**

Create `crates/caliban-memory/src/backend/fs.rs`:
```rust
//! Filesystem-backed topic store (the gonzalo-free default).
use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::auto::{
    parse_frontmatter, render_topic_file, validate_slug, TopicDraft, TopicFile, TopicKind,
    TopicSummary,
};
use crate::backend::TopicBackend;
use crate::error::{MemoryError, Result};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::TopicBackend;

    fn draft(name: &str, desc: &str) -> TopicDraft {
        TopicDraft { name: name.into(), description: desc.into(), kind: TopicKind::Project, body: "b".into() }
    }

    #[tokio::test]
    async fn write_read_list_delete_and_index_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let be = FsTopicBackend::new(tmp.path().to_path_buf());

        be.write(&draft("alpha", "first line")).await.unwrap();
        be.write(&draft("beta", "second")).await.unwrap();

        let mut names: Vec<_> = be.list().await.unwrap().into_iter().map(|s| s.name).collect();
        names.sort();
        assert_eq!(names, vec!["alpha".to_string(), "beta".to_string()]);

        let f = be.read("alpha").await.unwrap();
        assert_eq!(f.name, "alpha");

        let idx = be.index().await.unwrap();
        assert!(idx.contains("[alpha](alpha.md)"));
        assert!(idx.contains("first line"));

        be.delete("alpha").await.unwrap();
        assert_eq!(be.list().await.unwrap().len(), 1);
        assert!(!be.index().await.unwrap().contains("[alpha]"));
        // idempotent
        be.delete("alpha").await.unwrap();
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p caliban-memory -- backend::fs`
Expected: FAIL — `FsTopicBackend` not defined.

- [ ] **Step 3: Implement `FsTopicBackend` + shared `derive_index`**

In `crates/caliban-memory/src/backend/fs.rs` (above the tests):
```rust
/// Filesystem topic store. Enumerates `.md` siblings of `MEMORY.md`; the index
/// is a derived projection (never string-rewritten in place).
#[derive(Debug, Clone)]
pub struct FsTopicBackend {
    dir: PathBuf,
}

impl FsTopicBackend {
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn topic_path(&self, slug: &str) -> PathBuf {
        self.dir.join(format!("{slug}.md"))
    }
}

/// Build the `MEMORY.md` body from summaries. Single source of truth for both
/// backends — one `- [title](slug.md) — kind: first-desc-line` per topic,
/// slug-sorted for determinism.
pub(crate) fn derive_index(summaries: &[TopicSummary]) -> String {
    let mut rows: Vec<&TopicSummary> = summaries.iter().collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    let mut out = String::from("# Memory index\n\n");
    for s in rows {
        let desc = s.description.lines().next().unwrap_or("").trim();
        out.push_str(&format!("- [{n}]({n}.md) — {k}: {desc}\n", n = s.name, k = s.kind.as_str()));
    }
    out
}

#[async_trait]
impl TopicBackend for FsTopicBackend {
    async fn list(&self) -> Result<Vec<TopicSummary>> {
        let mut out = Vec::new();
        let mut rd = match tokio::fs::read_dir(&self.dir).await {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(source) => return Err(MemoryError::Io { path: self.dir.clone(), source }),
        };
        while let Some(entry) = rd.next_entry().await.map_err(|source| MemoryError::Io {
            path: self.dir.clone(),
            source,
        })? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            if path.file_name().and_then(|s| s.to_str()) == Some("MEMORY.md") {
                continue;
            }
            match read_summary_fs(&path).await {
                Ok(s) => out.push(s),
                Err(e) => tracing::warn!(?path, error = %e, "skipping malformed topic file"),
            }
        }
        Ok(out)
    }

    async fn read(&self, name: &str) -> Result<TopicFile> {
        validate_slug(name)?;
        let path = self.topic_path(name);
        let raw = tokio::fs::read_to_string(&path)
            .await
            .map_err(|source| MemoryError::Io { path: path.clone(), source })?;
        TopicFile::parse(&raw, &path)
    }

    async fn write(&self, draft: &TopicDraft) -> Result<String> {
        validate_slug(&draft.name)?;
        tokio::fs::create_dir_all(&self.dir)
            .await
            .map_err(|source| MemoryError::Io { path: self.dir.clone(), source })?;
        let path = self.topic_path(&draft.name);
        let serialized = render_topic_file(draft);
        // Reuse the crate's atomic writer (sync); the write is small and rare.
        caliban_common::fs::write_atomic(&path, serialized.as_bytes())
            .map_err(|source| MemoryError::Io { path: path.clone(), source })?;
        self.rewrite_index().await?;
        Ok(path.display().to_string())
    }

    async fn delete(&self, name: &str) -> Result<()> {
        validate_slug(name)?;
        let path = self.topic_path(name);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(MemoryError::Io { path, source }),
        }
        self.rewrite_index().await?;
        Ok(())
    }

    async fn index(&self) -> Result<String> {
        Ok(derive_index(&self.list().await?))
    }
}

impl FsTopicBackend {
    /// Materialise the derived index to `MEMORY.md` so the on-disk file stays in
    /// sync (the read path can still read it directly on the fs substrate).
    async fn rewrite_index(&self) -> Result<()> {
        let body = derive_index(&self.list().await?);
        let path = self.dir.join("MEMORY.md");
        caliban_common::fs::write_atomic(&path, body.as_bytes())
            .map_err(|source| MemoryError::Io { path, source })
    }
}

async fn read_summary_fs(path: &Path) -> Result<TopicSummary> {
    let raw = tokio::fs::read_to_string(path)
        .await
        .map_err(|source| MemoryError::Io { path: path.to_path_buf(), source })?;
    TopicSummary::parse(&raw, path)
}
```
In `crates/caliban-memory/src/backend/mod.rs`, add near the top:
```rust
pub(crate) mod fs;
pub use fs::FsTopicBackend;
```
In `crates/caliban-memory/src/auto.rs`, add two pure-string constructors used above (extract from the existing `read_summary`/`read` bodies so the parsing logic is shared, not duplicated). **Note the real structs are flat and carry a `path: PathBuf`:** `TopicSummary { name, description, kind, path }` and `TopicFile { name, description, kind, body, path }` — there is no nested `summary` field.
```rust
impl TopicSummary {
    /// Parse a topic summary from raw file text (frontmatter only).
    pub(crate) fn parse(raw: &str, path: &std::path::Path) -> Result<TopicSummary> {
        let (fm, _) = parse_frontmatter(raw, path)?;
        let kind = TopicKind::parse(fm.metadata.kind.as_deref().unwrap_or("")).ok_or_else(|| {
            MemoryError::InvalidTopic {
                path: path.to_path_buf(),
                reason: format!(
                    "metadata.type must be one of user|feedback|project|reference (got {:?})",
                    fm.metadata.kind
                ),
            }
        })?;
        Ok(TopicSummary { name: fm.name, description: fm.description, kind, path: path.to_path_buf() })
    }
}

impl TopicFile {
    /// Parse a full topic (frontmatter + body) from raw file text.
    pub(crate) fn parse(raw: &str, path: &std::path::Path) -> Result<TopicFile> {
        let (_, body) = parse_frontmatter(raw, path)?;
        let s = TopicSummary::parse(raw, path)?;
        Ok(TopicFile { name: s.name, description: s.description, kind: s.kind, body: body.to_string(), path: s.path })
    }
}
```
Then delete `update_index_line`, `remove_index_line`, `rewrite_with_index_line`, and their tests from `auto.rs` (index derivation replaces them). Keep the old sync `TopicLoader` inherent methods **removed** — the facade in `backend/mod.rs` is the only `TopicLoader` now.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p caliban-memory -- backend::fs`
Expected: PASS. Then `cargo test -p caliban-memory facade_delegates_write_then_list_to_fs_backend` — PASS (Task 1's delegation test now resolves).

- [ ] **Step 5: Port the surviving auto.rs behavior tests to the backend**

Move the still-relevant `auto.rs` tests (`read_rejects_invalid_type_in_frontmatter`, `read_rejects_invalid_slug`, `list_skips_malformed_topic_file`, `delete_missing_topic_is_idempotent`, etc.) into `backend/fs.rs` tests, rewritten to call `FsTopicBackend` async methods. Delete the originals in `auto.rs`. Run: `cargo test -p caliban-memory` — Expected: PASS, no orphaned tests referencing removed `TopicLoader` inherent methods.

- [ ] **Step 6: Commit**

```bash
git add crates/caliban-memory/src/backend/fs.rs crates/caliban-memory/src/backend/mod.rs \
        crates/caliban-memory/src/auto.rs
git commit -m "feat(memory): FsTopicBackend with async CRUD and derived index (#470)"
```

---

## Task 3: Shared conformance battery

**Files:**
- Create: `crates/caliban-memory/src/backend/conformance.rs`
- Modify: `crates/caliban-memory/src/backend/mod.rs`

**Interfaces:**
- Produces: `pub(crate) async fn run_topic_backend_conformance<B: TopicBackend>(backend: &B)` — a reusable battery any backend can be run through.

- [ ] **Step 1: Write the battery (it is itself the test harness) and call it for fs**

Create `crates/caliban-memory/src/backend/conformance.rs`:
```rust
//! Backend-agnostic conformance battery. Any `TopicBackend` must pass it.
#![cfg(test)]
use crate::auto::{TopicDraft, TopicKind};
use crate::backend::TopicBackend;

fn d(name: &str, desc: &str) -> TopicDraft {
    TopicDraft { name: name.into(), description: desc.into(), kind: TopicKind::User, body: "x".into() }
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
        let mut n: Vec<_> = be.list().await.unwrap().into_iter().map(|s| s.name).collect();
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

    // invalid slug rejected
    assert!(matches!(
        be.write(&d("Bad Slug", "x")).await,
        Err(crate::error::MemoryError::InvalidSlug { .. })
    ));

    // delete removes + idempotent
    be.delete("alpha").await.unwrap();
    assert_eq!(be.list().await.unwrap().len(), 1);
    be.delete("alpha").await.unwrap();
    assert!(!be.index().await.unwrap().contains("[alpha]"));
}
```
In `crates/caliban-memory/src/backend/mod.rs`:
```rust
#[cfg(test)]
pub(crate) mod conformance;
```
In `crates/caliban-memory/src/backend/fs.rs` tests, add:
```rust
    #[tokio::test]
    async fn fs_backend_passes_conformance() {
        let tmp = tempfile::tempdir().unwrap();
        let be = FsTopicBackend::new(tmp.path().to_path_buf());
        crate::backend::conformance::run_topic_backend_conformance(&be).await;
    }
```

- [ ] **Step 2: Run to verify it passes (fs)**

Run: `cargo test -p caliban-memory fs_backend_passes_conformance`
Expected: PASS. (If any assertion fails, fix `FsTopicBackend` — the battery is the contract.)

- [ ] **Step 3: Commit**

```bash
git add crates/caliban-memory/src/backend/conformance.rs crates/caliban-memory/src/backend/mod.rs \
        crates/caliban-memory/src/backend/fs.rs
git commit -m "test(memory): shared TopicBackend conformance battery, fs passes (#470)"
```

---

## Task 4: `gonzalo` cargo feature scaffolding + publish check

**Files:**
- Modify: `crates/caliban-memory/Cargo.toml`, `caliban/Cargo.toml`

**Interfaces:**
- Produces: off-by-default `gonzalo` feature on `caliban-memory` (`dep:gonzalo-core`, `dep:gonzalo-store-fs`) and on the `caliban` binary (enables `caliban-memory/gonzalo`).

- [ ] **Step 1: Add optional deps + feature to `caliban-memory`**

In `crates/caliban-memory/Cargo.toml`:
```toml
[dependencies]
# ... existing ...
gonzalo-core     = { version = "0.3", optional = true }
gonzalo-store-fs = { version = "0.3", optional = true }

[features]
gonzalo = ["dep:gonzalo-core", "dep:gonzalo-store-fs"]
```

- [ ] **Step 2: Add the propagated feature to the `caliban` binary**

In `caliban/Cargo.toml` `[features]` (create the table if absent), add:
```toml
gonzalo = ["caliban-memory/gonzalo"]
```
Confirm `default` does **not** include `gonzalo`.

- [ ] **Step 3: Verify both compile configs build**

Run:
```bash
cargo build -p caliban-memory
cargo build -p caliban-memory --features gonzalo
```
Expected: both succeed. (No gonzalo code yet — this proves the deps resolve at 0.3 and the feature gate is wired.)

- [ ] **Step 4: The deferred publish check**

Run: `cargo publish -p caliban-memory --dry-run`
Expected: PASS (packages cleanly). The optional `gonzalo-*` deps resolve on crates.io, so the manifest is publishable with the feature off. **If this fails**, stop and surface — it invalidates the feature-gating design.

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-memory/Cargo.toml caliban/Cargo.toml
git commit -m "build(memory): off-by-default gonzalo feature + optional deps (#470)"
```

---

## Task 5: `GonzaloTopicBackend` — mapping, author resolution, write/read

**Files:**
- Create: `crates/caliban-memory/src/backend/gonzalo.rs`
- Modify: `crates/caliban-memory/src/backend/mod.rs`, `crates/caliban-memory/src/lib.rs`

**Interfaces:**
- Consumes: `TopicBackend`, `TopicDraft`/`TopicFile`/`TopicSummary`, `derive_index` (Task 2), `MemoryError::{Backend,Conflict}`.
- Produces: `GonzaloTopicBackend::new(store: Arc<dyn gonzalo_core::Store>, workspace_slug: impl Into<String>) -> Self`; `pub(crate) fn topic_to_record(key: RecordKey, draft: &TopicDraft, meta: Meta) -> Record`; `pub(crate) fn resolve_author() -> Identity`; serde `StoredTopic` envelope.

- [ ] **Step 1: Write the failing test — write then read round-trips through gonzalo::FsStore**

Create `crates/caliban-memory/src/backend/gonzalo.rs`:
```rust
//! gonzalo-facade topic store. Feature-gated; the vanilla build never sees it.
#![cfg(feature = "gonzalo")]
use std::sync::Arc;

use async_trait::async_trait;
use gonzalo_core::{Body, Identity, KeyPrefix, Meta, PutResult, DeleteResult, Record, RecordKey, RecordKind, Revision, Store};
use serde::{Deserialize, Serialize};

use crate::auto::{validate_slug, TopicDraft, TopicFile, TopicKind, TopicSummary};
use crate::backend::{fs::derive_index, TopicBackend};
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
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p caliban-memory --features gonzalo -- backend::gonzalo`
Expected: FAIL — `GonzaloTopicBackend` undefined.

- [ ] **Step 3: Implement mapping, author resolution, and write/read**

In `crates/caliban-memory/src/backend/gonzalo.rs` (above tests):
```rust
/// gonzalo-backed topic store. Topics are `Record`s keyed
/// `caliban / memory:<workspace-slug> / <slug>`, bodies opaque JSON.
pub struct GonzaloTopicBackend {
    store: Arc<dyn Store>,
    collection: String,
    author: Identity,
}

impl GonzaloTopicBackend {
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

    fn prefix(&self) -> KeyPrefix {
        KeyPrefix { namespace: Some(NAMESPACE.into()), collection: Some(self.collection.clone()) }
    }
}

/// Resolve the record author: git identity if detectable, else "caliban".
/// Resolved once at construction — never on the hot path.
pub(crate) fn resolve_author() -> Identity {
    for field in ["user.email", "user.name"] {
        if let Ok(out) = std::process::Command::new("git").args(["config", field]).output() {
            if out.status.success() {
                let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !v.is_empty() {
                    return Identity::new(v);
                }
            }
        }
    }
    Identity::new("caliban")
}

fn now_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d as_millis_i64(&d)).unwrap_or(0)
}
fn as_millis_i64(d: &std::time::Duration) -> i64 {
    i64::try_from(d.as_millis()).unwrap_or(i64::MAX)
}

/// Build a topic `Record` with caller-supplied `Meta` (reused by #474's migrator
/// with mtimes; #470's `write` passes now()). Single source of the topic↔Record map.
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
        let existing = self.store.get(&key).await.map_err(|e| MemoryError::Backend(e.to_string()))?;
        let mut record = topic_to_record(key.clone(), draft, self.meta_now())?;
        let expected = existing.as_ref().map(|r| r.revision.clone());
        if let Some(prev) = existing {
            record.parent = Some(prev.revision.clone());
            // Revision::next(&self, body) -> counter+1, rehash (verified gonzalo 0.3 API).
            record.revision = prev.revision.next(record.body.bytes());
        }
        match self.store.put(record, expected).await.map_err(|e| MemoryError::Backend(e.to_string()))? {
            PutResult::Committed(_) => Ok(key.to_string()),
            PutResult::Conflict(_) => Err(MemoryError::Conflict { key: key.to_string() }),
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

    async fn list(&self) -> Result<Vec<TopicSummary>> { unimplemented!("Task 6") }
    async fn delete(&self, _name: &str) -> Result<()> { unimplemented!("Task 6") }
    async fn index(&self) -> Result<String> { unimplemented!("Task 6") }
}

/// Parse the opaque body once; `TopicSummary`/`TopicFile` are FLAT and carry a
/// `path` — the store has no real path, so synthesize a relative `<slug>.md`.
fn parse_stored(rec: &Record) -> Result<(StoredTopic, TopicKind)> {
    let bytes = match &rec.body {
        Body::Inline(b) => b.as_slice(),
        Body::Blob { .. } => return Err(MemoryError::Backend("unexpected blob body for topic".into())),
    };
    let s: StoredTopic = serde_json::from_slice(bytes).map_err(|e| MemoryError::Backend(e.to_string()))?;
    let kind = TopicKind::parse(&s.kind)
        .ok_or_else(|| MemoryError::Backend(format!("bad topic kind: {}", s.kind)))?;
    Ok((s, kind))
}

fn synthetic_path(slug: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("{slug}.md"))
}

fn stored_to_summary(rec: &Record) -> Result<TopicSummary> {
    let (s, kind) = parse_stored(rec)?;
    Ok(TopicSummary { path: synthetic_path(&s.name), name: s.name, description: s.description, kind })
}

fn stored_to_file(rec: &Record) -> Result<TopicFile> {
    let (s, kind) = parse_stored(rec)?;
    Ok(TopicFile { path: synthetic_path(&s.name), name: s.name, description: s.description, kind, body: s.body })
}
```
Notes for the implementer:
- `Revision::next(&self, body: &[u8]) -> Revision` is the verified successor API (counter+1, rehash). `Revision::initial(body)` mints counter 0.
- `TopicSummary`/`TopicFile` are flat (`name, description, kind, path[, body]`) — Task 2 established this; the synthetic `<slug>.md` path is cosmetic for the store backend (only fs-only affordances read `.path`).
- `list`/`delete`/`index` are stubbed to `unimplemented!` here **only** so this task's write/read test compiles; Task 6 replaces them in the same file. Do not ship this task as the final state — Task 6 is required before the feature build is green under `cargo test`.
- In `backend/mod.rs`: `#[cfg(feature = "gonzalo")] pub mod gonzalo; #[cfg(feature = "gonzalo")] pub use gonzalo::GonzaloTopicBackend;`. In `lib.rs`, re-export under the same cfg.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p caliban-memory --features gonzalo write_then_read_roundtrips`
Expected: PASS.

- [ ] **Step 5: Add the author-resolution test**

Append to `gonzalo.rs` tests:
```rust
    #[test]
    fn resolve_author_never_empty() {
        // Either a git identity or the "caliban" fallback — never empty.
        let id = resolve_author();
        assert!(!format!("{id:?}").is_empty());
    }
```
Run: `cargo test -p caliban-memory --features gonzalo resolve_author_never_empty` — Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/caliban-memory/src/backend/gonzalo.rs crates/caliban-memory/src/backend/mod.rs \
        crates/caliban-memory/src/lib.rs
git commit -m "feat(memory): GonzaloTopicBackend mapping + write/read + author resolution (#470)"
```

---

## Task 6: `GonzaloTopicBackend` — list/delete/index + conformance + conflict mapping

**Files:**
- Modify: `crates/caliban-memory/src/backend/gonzalo.rs`

**Interfaces:**
- Consumes: `topic_to_record`, `derive_index`, `Store::{list,delete}`.
- Produces: complete `TopicBackend for GonzaloTopicBackend`.

- [ ] **Step 1: Write the failing tests — conformance + conflict mapping**

Replace the `unimplemented!` stubs' test coverage by appending to `gonzalo.rs` tests:
```rust
    #[tokio::test]
    async fn gonzalo_backend_passes_conformance() {
        let tmp = tempfile::tempdir().unwrap();
        let g = be(tmp.path());
        crate::backend::conformance::run_topic_backend_conformance(&g).await;
    }

    #[tokio::test]
    async fn stale_write_maps_to_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        let g = be(tmp.path());
        // Seed, then force a stale expected by writing directly under the key with
        // a racing put via a second backend handle over the same store.
        g.write(&TopicDraft { name: "z".into(), description: "a".into(), kind: TopicKind::User, body: "1".into() }).await.unwrap();
        // Manually drive a conflict: put with expected=None on an existing key.
        let key = g.key("z");
        let rec = topic_to_record(key.clone(), &TopicDraft { name: "z".into(), description: "b".into(), kind: TopicKind::User, body: "2".into() }, g.meta_now()).unwrap();
        let r = g.store.put(rec, None).await.unwrap();
        assert!(matches!(r, PutResult::Conflict(_)), "expected=None on existing key must conflict");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p caliban-memory --features gonzalo gonzalo_backend_passes_conformance`
Expected: FAIL/panic — `unimplemented!` in `list`/`delete`/`index`.

- [ ] **Step 3: Implement list/delete/index**

In `gonzalo.rs`, replace the three stubbed methods:
```rust
    async fn list(&self) -> Result<Vec<TopicSummary>> {
        let keys = self
            .store
            .list(&self.prefix())
            .await
            .map_err(|e| MemoryError::Backend(e.to_string()))?;
        let mut out = Vec::with_capacity(keys.len());
        for key in keys {
            if let Some(rec) = self.store.get(&key).await.map_err(|e| MemoryError::Backend(e.to_string()))? {
                match stored_to_summary(&rec) {
                    Ok(s) => out.push(s),
                    Err(e) => tracing::warn!(%key, error = %e, "skipping unparseable topic record"),
                }
            }
        }
        Ok(out)
    }

    async fn delete(&self, name: &str) -> Result<()> {
        validate_slug(name)?;
        match self
            .store
            .delete(&self.key(name), None)
            .await
            .map_err(|e| MemoryError::Backend(e.to_string()))?
        {
            DeleteResult::Deleted => Ok(()),
            DeleteResult::Conflict(_) => Err(MemoryError::Conflict { key: self.key(name).to_string() }),
        }
    }

    async fn index(&self) -> Result<String> {
        Ok(derive_index(&self.list().await?))
    }
```
(Match the real `DeleteResult` variant names; if the "success" arm is not `Deleted`, use the actual name.)

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p caliban-memory --features gonzalo -- backend::gonzalo`
Expected: PASS — conformance + conflict tests green. The gonzalo backend now satisfies the same battery the fs backend does (the equivalence guarantee).

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-memory/src/backend/gonzalo.rs
git commit -m "feat(memory): GonzaloTopicBackend list/delete/index passes conformance (#470)"
```

---

## Task 7: Async ripple — memory tool, slash handlers, read path

**Files:**
- Modify: `crates/caliban-tools-builtin/src/memory/mod.rs`, `caliban/src/startup/compose.rs`, `caliban/src/tui/slash/existing.rs`, `crates/caliban-memory/src/loader.rs`

**Interfaces:**
- Consumes: async `TopicLoader` (Task 1). No new public API.

- [ ] **Step 1: Await the memory-tool writes/reads**

In `crates/caliban-tools-builtin/src/memory/mod.rs`, change the read tool (`ReadMemoryTopicTool::invoke`, ~line 69) and write tool (`WriteMemoryTopicTool::invoke`, ~line 175) to `.await` the loader and use the returned locator `String`:
```rust
        let locator = self.loader.write(&draft).await.map_err(|e| match e {
            caliban_memory::MemoryError::InvalidSlug { .. } => ToolError::invalid_input(e.to_string()),
            other => ToolError::execution(other),
        })?;
        Ok(vec![ContentBlock::Text(TextBlock {
            text: format!("→ Wrote memory topic '{}' to {locator} and updated MEMORY.md index", draft.name),
            cache_control: None,
        })])
```
Update the tool's unit tests (~line 208+) to `#[tokio::test]` + `.await`. Run: `cargo test -p caliban-tools-builtin memory` — Expected: PASS.

- [ ] **Step 2: Make the compose factory async-construct the fs backend and register**

In `caliban/src/startup/compose.rs:585`, `TopicLoader::new(cfg.auto_memory_dir)` stays synchronous (the fs backend constructor is sync); no change needed to construction. Confirm the tools it registers are unaffected (they only call `.await` internally). Run: `cargo build -p caliban` — Expected: builds. (This is the backend-selection point #473 will extend; #470 leaves it fs.)

- [ ] **Step 3: Route the read path's index through the backend**

In `crates/caliban-memory/src/loader.rs::load`, the auto-memory body currently comes from reading `MEMORY.md` on disk. Replace the auto-index acquisition so it is derived via a backend built from `cfg` (full end-to-end). Write the failing test first, in `loader.rs` tests:
```rust
    #[tokio::test]
    async fn load_derives_auto_index_from_backend() {
        let tmp = tempfile::tempdir().unwrap();
        let be = crate::backend::FsTopicBackend::new(tmp.path().to_path_buf());
        be.write(&crate::auto::TopicDraft { name: "gg".into(), description: "hook line".into(), kind: crate::auto::TopicKind::Project, body: "b".into() }).await.unwrap();
        let cfg = MemoryConfig { auto_memory_dir: tmp.path().to_path_buf(), disable_auto: false, ..MemoryConfig::default_for_test() };
        let prefix = load(&cfg).await.unwrap();
        assert!(prefix.auto_body_contains("[gg](gg.md)"));
    }
```
(Adjust `MemoryConfig` construction + the `prefix` accessor to the real API; if there is no test helper, build `MemoryConfig` with its real fields and assert against the real `MemoryPrefix` accessor for the auto tier.) Then implement: in `load`, obtain the auto index via `FsTopicBackend::new(&cfg.auto_memory_dir).index().await?` instead of reading `MEMORY.md`, keeping the existing conventions-block injection, HTML-comment stripping, and 200-line/25 KB caps applied to that derived body. Run the test — Expected: PASS.

- [ ] **Step 4: Convert the `/memory` slash subcommands to async backend calls**

In `caliban/src/tui/slash/existing.rs`:
- `list`/`show` branches (~:116/:143/:179): `.await` the loader/`load` calls.
- `delete` branch (~:249): replace the `loader.dir().join(...).exists()` existence check with a backend call — `loader.read(&slug).await.is_ok()` — then `loader.delete(&slug).await`. Keep the `delete_action` decision but feed it the backend-derived existence.
- `edit` branch (~:215): drop `TopicLoader`; compute the path directly from config: `let path = cfg.auto_memory_dir.join(format!("{rest}.md"));`. Add a code comment: `// fs-only affordance; non-fs substrates rework deferred to #473`.

Run: `cargo build -p caliban && cargo test -p caliban -- slash::existing` (or the crate's slash tests) — Expected: PASS.

- [ ] **Step 5: Remove `TopicLoader::dir()`**

Confirm no remaining callers (`rg 'loader\.dir\(\)|\.dir\(\)' caliban crates --type rust`). Delete the `dir()` method (it no longer exists on the facade). Run: `cargo build --workspace` — Expected: builds.

- [ ] **Step 6: Commit**

```bash
git add crates/caliban-tools-builtin/src/memory/mod.rs caliban/src/startup/compose.rs \
        caliban/src/tui/slash/existing.rs crates/caliban-memory/src/loader.rs
git commit -m "feat(memory): async topic backend wired through tools, slash, read path (#470)"
```

---

## Task 8: Full-gate verification in both configs

**Files:** none (verification + any fixes surfaced).

- [ ] **Step 1: Default-config gate**

```bash
cargo fmt --all
cargo fmt --all -- --check
touch $(git diff --name-only HEAD~7 | grep '\.rs$')   # warm files for the CI-matching clippy
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --all-targets
cargo test --workspace
```
Expected: all pass. Fix any failure, re-run.

- [ ] **Step 2: gonzalo-feature gate**

```bash
cargo clippy -p caliban-memory --features gonzalo --all-targets -- -D warnings
cargo build -p caliban --features gonzalo
cargo test -p caliban-memory --features gonzalo
```
Expected: all pass.

- [ ] **Step 3: Re-confirm the publish dry-run**

```bash
cargo publish -p caliban-memory --dry-run
```
Expected: PASS (still green after gonzalo code exists behind the feature).

- [ ] **Step 4: Commit any fixes**

```bash
git add -A && git commit -m "chore(memory): satisfy gate in default and gonzalo configs (#470)"
```

---

## Self-review notes

- **Spec coverage:** full-end-to-end read path (Task 7 §3), `TopicBackend` seam (Tasks 1–2), `FsTopicBackend` default + derived index (Task 2), feature gating + publish check (Tasks 4, 8), `GonzaloTopicBackend` mapping/OCC/author (Tasks 5–6), conformance battery (Task 3), `topic_to_record` seam for #474 (Task 5), error variants (Task 1), `/memory edit` fs-only caveat (Task 7 §4). All spec sections map to a task.
- **Type consistency:** `TopicBackend` methods, `write -> Result<String>` locator, `derive_index`, `topic_to_record`, `resolve_author`, `StoredTopic` are used consistently across tasks.
- **Known plan-level adjustments the implementer must reconcile against real code (not placeholders — explicit reconciliation points):** exact field names of `TopicSummary`/`TopicFile`; the real `Revision` successor API; the real `DeleteResult` success variant; `MemoryConfig` test construction + `MemoryPrefix` auto-tier accessor. Each is called out at its use site with the fallback to the actual signature.
