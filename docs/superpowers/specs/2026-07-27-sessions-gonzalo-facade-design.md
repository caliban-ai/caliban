# Sessions via the gonzalo facade + async debounce-writer rework — Design

**Ticket:** caliban-ai/caliban#471 · **Epic:** #469 (phase 3, after #470 memory pilot and #473 storage config) · **Driver:** caliban-ai/gonzalo#1

## Goal

Route `caliban-sessions` persistence through the substrate-neutral facade
established by #470/#473, so a session persists to local `fs` (default,
unchanged) or a remote `gonzalo` daemon by config — and rework the debounced
writer so it awaits async/remote `put`s, surfaces conflicts/errors instead of
fire-and-forget `warn!`, and still drains reliably on shutdown.

## Background

`SessionStore` (`crates/caliban-sessions/src/store.rs`) is a synchronous CRUD
facade over `PersistedSession`, fronting a `DebouncedWriter`
(`crates/caliban-sessions/src/debounced.rs`) — a dedicated OS thread running a
current-thread tokio runtime that coalesces bursts of `save()` into a single
`caliban_common::fs::write_atomic` inside a 250 ms window. `save()` is
fire-and-forget (only `warn!`s on failure, though the failure is observable via
`flush()`'s returned outcome and `last_write_error()`). Reads (`load`/`list`)
and `delete` call `flush()` first so the on-disk view is current. Flush-on-drop
(bounded by `DROP_DRAIN_TIMEOUT = 2s`) is the only shutdown drain — no
production path calls `flush()` explicitly.

Consumer surface (binary crate only; workers and `caliban-checkpoint` do **not**
use `SessionStore`):

- **Construction (single production site):** `caliban/src/startup/compose.rs`
  `resolve_session()` builds `SessionStore::new(SessionStore::default_root()?)`
  only when `--session`/`--continue`/`--resume` is set. A defensive fallback
  builds a fresh store in `caliban/src/startup/drivers.rs::resolve_resume()`.
- **Hot-path save:** `caliban/src/tui/events.rs` (per-turn RunEnd),
  `caliban/src/startup/drivers.rs::persist_session()` (headless / single-prompt
  run tail).
- **Cold-path:** `load` (compose startup, headless `session_loader`), `list`
  (`--continue`, `/resume` slash), cold-shutdown `save` (`caliban/src/tui.rs`).
- **Threading:** owned `Option<SessionStore>` in the TUI `App`, all three
  drivers, and `resolve_session`'s return — **never** `Arc<dyn>`. `SessionStore`
  is `Clone` (internal `Arc`).
- **`delete`/`flush`/`last_write_error` have no production callers**; `restore`
  in `caliban-checkpoint` imports only the `PersistedSession` **value type** and
  mutates it in place — no `SessionStore` round-trip.

The #470/#473 precedent to mirror: `caliban-memory`'s
`backend/{mod.rs,fs.rs,gonzalo.rs,conformance.rs}` (`#[async_trait] TopicBackend`
+ `FsTopicBackend` always-compiled + `GonzaloTopicBackend` feature-gated + shared
conformance harness), selected by `caliban/src/startup/storage.rs`
`build_topic_backend(&StorageConfig, dir) -> Arc<dyn TopicBackend>`.

## Architecture

Keep `SessionStore` as the public facade; inject an `Arc<dyn SessionBackend>`
into it. The `DebouncedWriter`'s terminal `write_atomic` becomes
`backend.save(&session).await`. This preserves the 250 ms coalescing (**more**
valuable over the network), preserves flush-on-drop, and keeps the entire
`Option<SessionStore>` threading unchanged — only the one production
construction site swaps `SessionStore::new(root)` for
`SessionStore::with_backend(<factory output>)`.

**Decision — keep client-side debouncing:** yes. Network `put`s are more
expensive than disk writes, so burst-coalescing is more valuable remotely, and
keeping it substrate-neutral means one tested code path serves both.

### Components

1. **`SessionBackend` trait** — new, in `caliban-sessions`, `#[async_trait]`,
   `Send + Sync`, substrate-neutral:
   - `async fn save(&self, session: &PersistedSession) -> Result<()>`
   - `async fn load(&self, name: &str) -> Result<Option<PersistedSession>>`
   - `async fn list(&self) -> Result<Vec<SessionMetadata>>`
   - `async fn delete(&self, name: &str) -> Result<()>`

   `SessionMetadata` (the existing struct) moves to being produced by the
   backend, since gonzalo derives it from records rather than files. A shared
   `conformance::run_session_backend_conformance<B: SessionBackend>(&B)` harness
   (mirroring `memory/backend/conformance.rs`) asserts the CRUD contract:
   save→load roundtrip, list-after-save, delete, load-missing→`None`,
   list-sorted-by-`updated_at`-desc.

2. **`FsSessionBackend`** — always compiled, gonzalo-free. Lifts the current
   `store.rs` fs logic behind the trait: `save` = `write_atomic` of
   `to_vec_pretty`; `load` = `read` + `from_slice` (`NotFound` → `None`);
   `list` = `read_dir`, skip non-`.json` and broken files, sort by `updated_at`
   desc; `delete` = `remove_file` (`NotFound` → `Ok`). Name validation
   (`validate_name`, `MAX_NAME_LEN = 64`, ascii-alnum/`_`/`-`) stays in the
   backend. Behaviour-equivalent to today.

3. **`GonzaloSessionBackend`** — `#[cfg(feature = "gonzalo")]`. OCC get→put
   mirroring `memory/backend/gonzalo.rs`:
   - Key: `RecordKey::new("caliban", format!("sessions:{workspace_slug}"), name)`.
     **Workspace-scoped** (like #470's `memory:<slug>`) so same-named sessions
     across workspaces don't collide on a shared remote.
   - `RecordKind::Session`. (See "RecordKind decision" below.)
   - Opaque JSON `Body::Inline` — `serde_json::to_vec(&PersistedSession)`,
     lossless vs. the `.json` file. `save` OCC: `get` current → build record →
     if existing, set `parent`/`revision = prev.revision.next(body)` and preserve
     `meta.created`; `put(record, expected)`; `PutResult::Conflict` → a
     `SessionBackendError::Conflict`.
   - `load` = `get` → deserialize; missing → `None`. `list` = `list(&prefix)` →
     `get` each → derive `SessionMetadata` (skip unparseable with `warn!`).
     `delete` = `store.delete(key, None)`.
   - Author resolved once at construction: git `user.email`/`user.name` if
     detectable, else `Identity::new("caliban")` (reuse the #470 resolver logic).

4. **`DebouncedWriter` rework** — holds `Arc<dyn SessionBackend>`; buffers
   `HashMap<String /*name*/, PersistedSession>` (coalesce-by-name; latest wins),
   replacing `HashMap<PathBuf, Vec<u8>>`. The worker's current-thread runtime
   already runs `writer_loop` under `block_on`, so drain awaits
   `backend.save(&session).await` directly. The debounce window + `MAX_DELAY`
   ceiling + `oldest_dirty` bound are unchanged. `do_write`/`drain_pending`
   become async and record backend errors (incl. `Conflict`) in the `last_error`
   health slot (now keyed by session name).

   **Reads stay sync at the public API.** `SessionStore::{load,list,delete}` keep
   their synchronous signatures by routing through the worker thread via the same
   `std::sync::mpsc` round-trip `flush()` already uses — new `WriterMsg::Load` /
   `List` / `Delete` variants each carry a response `Sender`. On receipt the
   worker drains pending first (equivalent to today's flush-before-read), then
   awaits the backend op, then sends the typed result back. This avoids the
   `block_on`-inside-`#[tokio::main]` panic the flush code already documents, and
   keeps all ~6 read call sites unchanged.

5. **`build_session_backend` factory** — new fn beside
   `startup/storage.rs::build_topic_backend`, reading the **same** shared
   `StorageConfig`/`StorageSubstrate`:
   - `Fs` → `Arc::new(FsSessionBackend::new(sessions_root))` (always).
   - `Remote` → `#[cfg(feature = "gonzalo")]` `GonzaloSessionBackend` over
     `ServerStore` (token from `token_env`, reusing `storage.rs`'s `remote_store`
     helper), with the same fail-fast `list` connectivity probe;
     `#[cfg(not(feature = "gonzalo"))]` → the "rebuild with `--features gonzalo`"
     error.
   - `Git`/`S3` → recognized-but-not-wired error (tracked in #469).
   - `workspace_slug` = blake3 hex of the sessions-dir path (reuse #473's helper
     approach; independent of symlink resolution).

   `caliban-sessions/Cargo.toml` gains `[features] gonzalo = ["dep:gonzalo-core",
   "dep:gonzalo-store-fs"]` + optional `gonzalo-core`/`gonzalo-store-fs` `0.3`
   (mirroring `caliban-memory`); `gonzalo-store-fs` also a dev-dep for tests.
   `caliban/Cargo.toml`'s `gonzalo` feature gains
   `"caliban-sessions/gonzalo"`.

### RecordKind decision

`RecordKind::Session` maps to `MergeClass::AppendOnly` (union-merge), but caliban
rewrites session bodies **wholesale** each save (`merge_run` replaces `messages`
entirely), so a union-merge of two full JSON documents would corrupt. Verified:
`merge_class` is **not consulted anywhere in gonzalo 0.3's `Store::put` path** —
`put` is pure OCC ("if the store's current revision differs, returns `Conflict`",
`gonzalo-core-0.3.0/src/store.rs:48`) regardless of kind. So the merge-class is an
inert annotation for a future reconciler that does not exist in 0.3. This is the
identical situation #470 accepted for memory (`RecordKind::Topic` is also
`AppendOnly` and also wholesale-rewritten).

**Use `RecordKind::Session`** (semantically correct name; conflicts surface
correctly via OCC today). **File a gonzalo follow-up** flagging the
`AppendOnly`-vs-wholesale-rewrite mismatch — to revisit (make `Session` `Opaque`,
or add an opaque-classed session kind) if/when a background reconciler lands.

### Error / conflict surfacing

Extend the sessions error type with a `Conflict` variant. A remote OCC conflict
propagates through the **existing** `last_error` slot + `flush()`-returns-outcome
channel rather than the old silent fire-and-forget `warn!`. `save()` stays
enqueue-and-return; failures remain observable via `flush()` /
`last_write_error()`.

### Scope guards

- `caliban-checkpoint::restore` is unaffected — it operates on the in-memory
  `PersistedSession` value, not `SessionStore`. Add a regression test asserting a
  gonzalo-persisted session loads and restores correctly.
- `delete`/`flush`/`last_write_error` have no production callers today, but the
  trait carries them (conformance-tested) for completeness and #474's migrator.
- `--bare` (ADR 0025 CI mode) and `--no-save`: sessions are only constructed
  when a session flag is present; `--bare` never constructs one, so the factory
  and probe are skipped exactly as they are for memory in #473.

## Testing

- `conformance::run_session_backend_conformance` run against **both** backends.
- `FsSessionBackend` behaviour-equivalence: file layout, pretty-JSON,
  broken-file skip, sort order, name validation.
- `GonzaloSessionBackend`: save→load roundtrip; update preserves `meta.created`,
  advances `updated`; stale write (`expected=None` on existing key) → `Conflict`;
  workspace-scoped key isolation.
- `DebouncedWriter` (async backend): single-write-lands, coalesce-to-latest,
  window-expiry flush, sync `flush()` from inside a runtime does not panic,
  drop-drains-pending, `Conflict`/error recorded in `last_error`, sustained-write
  max-delay bound. Reads via the worker round-trip return correct results.
- Factory: fs builds without feature; git/s3 not-wired; remote-without-feature
  errors clearly; (gonzalo) `workspace_slug` deterministic + path-sensitive,
  `remote_store` ok/errors, probe succeeds on a healthy fs-backed store.
- Checkpoint-restore-over-gonzalo regression test.

## Global constraints

- **`cargo publish` stays green:** `caliban-sessions` must compile with zero
  gonzalo references in the default (no-feature) build; all gonzalo code
  `#[cfg(feature = "gonzalo")]`-gated. The `caliban-settings` `StorageConfig`
  surface is already gonzalo-free (#473) and is reused unchanged.
- **Default = fs = zero behavior change.** Absent/`fs` config persists sessions
  exactly as today (pretty JSON files under the same root).
- **Bearer token from env only** (`token_env`), never `settings.json` — reuse
  #473's `remote_store`.
- **Factory error is fatal** at startup (the memory factory uses `exit 78`,
  EX_CONFIG; match that for sessions).
- gonzalo crates pinned at `0.3` (registry, optional), mirroring `caliban-memory`.
- CI gate: `cargo fmt --all -- --check` · `cargo clippy --workspace
  --all-targets -D warnings` · `cargo build --workspace --all-targets` · `cargo
  test --workspace`, plus `--features gonzalo` clippy/build/test and both
  publish dry-runs.
