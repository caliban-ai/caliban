# Storage substrate selection + remote gonzalo config — Design

**Date:** 2026-07-19
**Status:** Approved (design)
**Ticket:** caliban-ai/caliban#473 · EPIC #469 · follow-on to the landed #470 pilot
**Topic:** A typed `storage` config in `caliban-settings` selecting the memory substrate (fs default; remote gonzalod), plus a feature-aware backend factory that constructs the chosen `TopicBackend` for both the write and read paths.

## Goal

Turn the `TopicBackend` selection that #470 deferred into a runtime, config-driven choice: `storage.substrate = "fs"` (default, unchanged) or `"remote"` (a gonzalo daemon). Absence of config ⇒ today's behavior. This delivers the epic's "local-first, remote-enabled" headline.

## Scope decisions (settled in brainstorming)

1. **Substrates: fs + remote only.** `git`/`s3` are recognized (parseable) but return a clear "not wired yet (#469)" error; a local gonzalo-`FsStore` substrate is also deferred. All substrate crates are published @0.3, so this is YAGNI scoping, not a dependency block.
2. **Connectivity = fail-fast at startup.** When `substrate=remote`, the factory probes the daemon with a `list` during construction; unreachable/unauthorized ⇒ a hard startup error. No new `doctor` subcommand.
3. **Default is always the gonzalo-free `FsTopicBackend`** — no `gonzalo` feature required, zero behavior change.

## Architecture

The design resolves one core tension: `StorageConfig` must live in `caliban-settings` (always compiled, **no gonzalo dependency** — the crate stays publishable), but constructing any gonzalo-backed store requires the `gonzalo` cargo feature in the caliban binary. Resolution: **config is pure data**; a **feature-aware factory in the caliban binary** maps config → backend and errors clearly when a gonzalo substrate is requested but the feature is absent.

```
caliban-settings (gonzalo-free)        caliban binary (feature-aware)         caliban-memory
  StorageConfig { substrate, remote } → build_topic_backend(cfg, dir) ──────→ Arc<dyn TopicBackend>
                                          ├ Fs     → FsTopicBackend (always)
                                          ├ Remote → GonzaloTopicBackend(ServerStore) + probe   [cfg(gonzalo)]
                                          └ Git/S3 → Err("not wired")
```

## The config type (`caliban-settings/src/settings.rs`)

Pure data, no gonzalo types:
```rust
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct StorageConfig {
    pub substrate: StorageSubstrate,        // default Fs
    pub remote: Option<RemoteStorageConfig>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StorageSubstrate {
    #[default] Fs,
    Remote,
    Git,   // parses → factory returns "recognized, not wired (#469)"
    S3,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct RemoteStorageConfig {
    pub url: String,                // gonzalod base URL
    pub token_env: Option<String>, // NAME of the env var holding the bearer token
}
```
- `pub storage: StorageConfig` added to `Settings` (`#[serde(default)]`). Absent ⇒ `Fs` ⇒ unchanged.
- `Git`/`S3` are kept as parseable variants so the factory emits a friendly "unsupported substrate" message rather than a raw serde "unknown variant" error.
- **`token_env`, not `token`** — secrets never live in a shared `settings.json`; the factory reads the named env var. Matches caliband #288's fail-closed token model.
- Ripples: `caliban-settings/src/schema.json` gains a `storage` object; `url` validated via the existing `url` crate dep.

## The backend factory (`caliban/src/startup/`)

```rust
pub async fn build_topic_backend(storage: &StorageConfig, auto_memory_dir: &Path)
    -> Result<Arc<dyn TopicBackend>, String>
{
    match storage.substrate {
        StorageSubstrate::Fs     => Ok(Arc::new(FsTopicBackend::new(auto_memory_dir))),
        StorageSubstrate::Remote => build_remote_backend(storage, auto_memory_dir).await,
        StorageSubstrate::Git | StorageSubstrate::S3 =>
            Err(format!("storage.substrate {:?} is recognized but not wired yet (tracked in #469); use fs or remote", storage.substrate)),
    }
}
```
`build_remote_backend` is split into two testable halves:
- **(a) config → `Arc<dyn Store>`** (`#[cfg(feature="gonzalo")]`): read `token_env` from the environment, `ServerStore::http_with_token(url, token)` (sync ctor).
- **(b) `Store` → probed backend**: compute `workspace_slug(auto_memory_dir)` (the stable dir hash #470 deferred), build `GonzaloTopicBackend::new(store, slug)`, then **probe** via `backend.list().await` — error ⇒ `"gonzalo remote <url> unreachable/unauthorized: <e>"`. Splitting at the `Store` boundary lets (b) be tested with an injected `gonzalo::FsStore` (no live daemon).

Under `#[cfg(not(feature = "gonzalo"))]`, `build_remote_backend` returns `"this build lacks gonzalo support; rebuild with --features gonzalo"`.

## Wiring both paths to one backend

Build the backend **once** at startup and share it (write and read paths must not diverge):
- `compose.rs:620-621`: `let backend = build_topic_backend(&settings.storage, &cfg.auto_memory_dir).await?;` → `TopicLoader::with_backend_arc(backend.clone())` for the memory tools.
- **`caliban_memory::loader::load` signature changes** (the one real ripple): `load(config: &MemoryConfig, backend: &dyn TopicBackend)` — no longer hardcodes `FsTopicBackend` (loader.rs:73). Its callers (`compose.rs:1702`, `/memory list` in `slash/existing.rs:116`) pass the shared backend. **Non-optional** so the compiler forbids an accidental fs fallback that would re-introduce write/read divergence.
- `TopicLoader` gains `with_backend_arc(Arc<dyn TopicBackend>)` (currently holds `Box`; hold via `Arc` so both paths share one instance).

## Errors

Factory `Err` ⇒ fatal startup error, **exit 78 (EX_CONFIG)** with the message. Distinct messages: feature-absent+remote; git/s3 not-wired; remote missing `[storage.remote]`; `token_env` unset; daemon unreachable (probe).

## Testing

- **caliban-settings** (no feature/daemon): `StorageConfig` serde — absent ⇒ `Fs`, `fs`, `remote{url,token_env}`, `deny_unknown_fields` rejects typos, `schema.json` validates, URL round-trips.
- **factory, default features**: `fs` → Ok; `remote` → "rebuild with feature" Err; `git`/`s3` → "not wired" Err.
- **factory, `--features gonzalo`**: half (b) tested with an injected `gonzalo::FsStore` — healthy store probes OK; a store that errors on `list` yields the "unreachable" error. Half (a)'s `ServerStore` URL/token construction unit-tested; a live `ServerStore`↔`gonzalod` round-trip deferred to gonzalo's crate tests + an optional smoke test.
- **read path**: `load(config, backend)` with injected fs and `FsStore`-backed gonzalo backends derives the index from *that* backend.
- Full gate in **both** configs; `cargo publish -p caliban-settings --dry-run` stays green (no new deps).

## Out of scope

- git/s3 substrates and a local gonzalo-`FsStore` substrate (deferred #469 follow-ups; recognized-with-error now).
- A dedicated `caliban doctor storage` subcommand (fail-fast startup probe covers the need).
- Sessions (#471), checkpoints (#472), migrator (#474).
- gRPC remote (`ServerStore::grpc_with_token`) — HTTP only for now; gRPC is a trivial future addition to the factory's (a) half.
