# Memory ↔ gonzalo facade (pilot) — Design

**Date:** 2026-07-12
**Status:** Approved (design)
**Ticket:** caliban-ai/caliban#470 · EPIC #469 · driver caliban-ai/gonzalo#1
**Topic:** Route `caliban-memory` auto-memory topic CRUD (and the prompt-splice index read) through the gonzalo facade, behind an off-by-default `gonzalo` cargo feature.

## Goal

Make auto-memory topics persist through the **gonzalo `Store`** instead of direct `std::fs`, as the pilot that proves the facade pattern end-to-end for EPIC #469 (sessions #471, checkpoints #472, config #473, migrator #474 follow). The vanilla `cargo install caliban` build stays gonzalo-free; the gonzalo-backed path is opt-in and, in this ticket, proven by tests rather than wired for runtime selection (that is #473).

## Scope decisions (settled in brainstorming)

1. **Full end-to-end**, not write-path-only. Both the write path (memory tools, `/memory`) and the **prompt-splice read path** (`loader.rs::load`) route through the backend. `MEMORY.md` becomes an in-memory projection rebuilt from `store.list()`, never read from disk — so a remote substrate can feed the prompt with no code change (honors the epic AC).
2. **#470 ships plumbing + tests; runtime selection is #473.** `FsTopicBackend` is the live default (behavior-equivalent to today). `GonzaloTopicBackend` is feature-gated and proven end-to-end by integration tests against `gonzalo::FsStore`. #470 does **not** depend on #473.
3. **`RecordKind::Topic`** for the body, with a documented caveat (below).

### Rejected alternatives

- **"gonzalo everywhere" (no feature flag)** — depending on gonzalo unconditionally pulls its transitive tonic/reqwest tree into every `cargo install caliban`. Rejected; we feature-gate.
- **Scattered `#[cfg]` inside `TopicLoader`** — branch on the feature per method. Scatters conditionals, both paths hard to test. Rejected in favor of a trait seam.
- **Depend on gonzalo's `Store` trait directly in the default build** — impossible without pulling gonzalo into the vanilla build. A caliban-owned trait is therefore forced, not chosen.

## Architecture

```
caliban-memory
├── trait TopicBackend         (async: list / read / write / delete / index)  ← caliban-owned
├── FsTopicBackend             (default, ALWAYS compiled, no gonzalo dep)
│      today's logic ported to tokio::fs; index() derived from list()
├── GonzaloTopicBackend        (#[cfg(feature = "gonzalo")])
│      wraps Arc<dyn gonzalo_core::Store>; maps topics ⇄ Records
└── TopicLoader                (public facade; holds Box<dyn TopicBackend>, async methods)
```

- **One caliban-owned async trait, `TopicBackend`, is the seam.** Callers (memory tools, `/memory`, `loader::load`) talk only to `TopicLoader` → `dyn TopicBackend`; none knows the substrate.
- **`FsTopicBackend` is the wired default** and stays gonzalo-free, so the vanilla build and `cargo publish --dry-run` are unaffected. Its `index()` derives `MEMORY.md` from the topic set — the same derivation the gonzalo backend uses — retiring the fragile in-place `update_index_line`/`remove_index_line` string-rewriting.
- **`GonzaloTopicBackend` is feature-gated** behind a new off-by-default `gonzalo` feature (optional `gonzalo-core` + `gonzalo-store-fs` registry deps @ `0.3`).
- **`TopicLoader` remains the public facade** (callers keep `Arc<TopicLoader>`); its methods become async and delegate to the boxed backend.

## Data mapping

| Concern | Mapping |
|---|---|
| **RecordKey** | `("caliban", "memory:<workspace-slug>", "<slug>")`. Namespace fixed. **Collection encodes the workspace** (derived from the memory-dir identity) so a shared/remote store keeps workspaces separate and `list(ns, "memory:<slug>")` enumerates exactly that workspace's topics. `id` = topic slug. |
| **RecordKind** | `Topic` (AppendOnly). See caveat. |
| **Body** | `Body::Inline(json)` where `json` is the serialized `TopicFile` — frontmatter (name, description, kind) **+ the markdown body**. Opaque and lossless; **not** the lossy `gonzalo::Topic{slug,bullets}`. Round-trips equivalent to today's `.md`. |
| **Meta** | `author` = **the git identity if detectable** (`git config user.email`, falling back to `user.name`), else the constant `"caliban"` `Identity` — resolved once at backend construction, not per write; `origin_system = "caliban"`; `created`/`updated` = now (epoch millis); `labels` carry the topic kind. |
| **Write (OCC)** | get current → `put(new_record, expected=Some(current.revision))`, or `expected=None` to create. Revision minted via gonzalo's `Revision` (counter + blake3 body hash). |
| **Delete** | `Store::delete(key, None)` — unconditional, idempotent (matches today's idempotent delete). |
| **Index** | `index()` rebuilds `MEMORY.md` from `list()` + summaries (`- [title](slug.md) — kind: desc`). The conventions block + 200-line / 25 KB caps stay in `loader.rs::load` where they are today. |

**RecordKind caveat.** We store an opaque, wholesale-replaced JSON body, for which `Topic`'s AppendOnly union-merge is not semantically honest under **multi-writer remote sync** — the same concern #471 raises for sessions. It is **inert for the fs single-writer pilot** (no sync, no conflict). We adopt `Topic` now (name-aligned) and track the merge-class question as a known follow-up tied to remote sync.

## Async ripple + error handling

Every caller is already in an async context, so the conversion is mechanical `.await` additions, not a threading rework.

- `TopicLoader` methods go async, delegating to `Box<dyn TopicBackend>`.
- **Backend factory:** `caliban/src/startup/compose.rs:585` (constructs the loader + registers the Read/Write memory tools) becomes the single backend-selection site — fs today; #473 adds config-driven selection.
- **Read path:** `caliban_memory::load(&cfg)` (`compose.rs:1669`, `tui/slash/existing.rs:116`) stays `&MemoryConfig` in/out; under full-end-to-end it derives the index via a backend it constructs from `cfg` internally (fs default) — no signature change to its callers.
- **`/memory` slash subcommands** (`tui/slash/existing.rs:143/179/208/249`) construct `TopicLoader` and call list/read/write/delete inside async handlers → `.await` additions.
- **Memory tools** (`WriteMemoryTopicTool`/`ReadMemoryTopicTool::invoke`) are already `async fn`.

**Errors.** Two new `MemoryError` variants absorb gonzalo: `Backend(String)` (wraps `gonzalo_core::CoreError`) and `Conflict { key }` (from `PutResult::Conflict`/`DeleteResult::Conflict`). `Conflict` is dormant on single-writer fs but required by the trait contract. The trait's error type is the unified `MemoryError`, so both backends surface the same enum — no caller-visible error-type churn beyond the new variants. **All gonzalo error mapping lives inside `GonzaloTopicBackend`** (feature-gated); the vanilla build never names a gonzalo type.

## Testing

- **Shared conformance battery, parametrized over both backends** — list/read/write/delete round-trips, idempotent delete, index derivation, invalid-slug rejection, malformed-topic skip. This is the "fs behavior equivalent" guarantee.
- **`FsTopicBackend`** — today's `auto.rs` tests port over (now async); they already cover the behaviors.
- **`GonzaloTopicBackend`** (`#[cfg(feature = "gonzalo")]` tests) — integration over `GonzaloTopicBackend::new(Arc::new(gonzalo::FsStore::new(tmp)))`: full CRUD + index end-to-end, the OCC get→put handshake, and the `Conflict` mapping (driven with a stale `expected`).
- **Equivalence test** — identical op-sequence against both backends yields identical `list()` summaries and identical `index()` output.

## Cargo feature + publish

- `caliban-memory/Cargo.toml`: `gonzalo = ["dep:gonzalo-core", "dep:gonzalo-store-fs"]`, both `optional = true` @ `"0.3"`. Propagated as an off-by-default `gonzalo` feature on the `caliban` binary crate.
- **Publish check (the deferred empirical validation):** `cargo publish -p caliban-memory --dry-run` with the optional dep present + feature off must stay green (gonzalo 0.3.0 resolves on crates.io). Green confirms the feature-gating design.
- Run the full gate (fmt/clippy/build/test) in **both** the default and `--features gonzalo` configurations.

## #474 (migrator) interplay

#470 factors the topic→`Record` mapping into a reusable `topic_to_record(key, draft, meta)` used by `write` (meta = now). The migrator (#474) reuses it with `meta` built from original file mtimes — authorship/timestamp preservation is a **parameter**, not a re-implementation. #470 provides the seam; #474 builds the migrator.

## Out of scope

- Runtime substrate selection / `StorageConfig` (#473).
- Sessions (#471), checkpoints (#472), the migrator itself (#474).
- Multi-writer remote-sync merge semantics for opaque memory bodies (tracked with #471's concern).
- git/s3/remote substrates (the pilot exercises `gonzalo::FsStore`; remote works by construction once #473 wires selection).
- **`/memory edit`** stays an fs-path affordance: it opens the raw `.md` in `$EDITOR` via `cfg.auto_memory_dir` directly (not through the backend), and `TopicLoader::dir()` is dropped from the abstraction. Reworking `/memory edit` for non-fs substrates (fetch → temp file → edit → write-back) is deferred to #473. All other `/memory` subcommands (list/show/delete) route through the async backend.
