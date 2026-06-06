# Design: Publish caliban to crates.io (guarded to `caliban-ai/caliban`)

**Date:** 2026-06-05
**Status:** Approved (design)
**Topic:** crates.io publishing pipeline + crate-readiness

## Goal & guarantee

Enable `cargo install caliban` by publishing the `caliban` binary together with
its 24 internal library crates to crates.io. Releases must happen **only** from
the public repository `caliban-ai/caliban` — never from the private `origin`
(`johnford2002/caliban`), never from a fork. The design is **fail-closed**: any
misconfiguration (wrong repo, missing token, mismatched version) prevents
publishing rather than publishing incorrectly.

## Background & decision record

The project is a workspace of 25 crates (24 libs + the `caliban` binary), all
currently `publish = false`, sharing one workspace version (`0.1.0`), with
inter-crate dependencies expressed as `path`-only (no version).

A binary cannot be installed from crates.io unless every crate in its dependency
tree is also on crates.io — `cargo install caliban` resolves dependencies from
the registry, not from local `path` entries. Three philosophies were considered:

- **A — collapse to a single crate** (flatten the 24 libs into modules). Rejected:
  destroys the workspace's incremental-compilation boundaries and cross-crate
  build parallelism, which are local-dev properties worth keeping.
- **B — publish the workspace, mark the libs internal/unstable.** **Chosen.**
  Matches the mainstream binary-app pattern (nushell publishes 18 internal
  `nu-*` crates, deno ~30 `deno_*`), keeps incremental compilation fully intact
  (publishing is orthogonal to local builds), and gets `cargo install caliban`.
- **C — publish a curated SDK subset.** Deferred: no concrete external-consumer
  demand today; can promote a specific crate later (the `nu-plugin` / `deno_core`
  move) if/when that demand appears.

The crate boundaries were reviewed and **locked as-is** (clean acyclic 4-layer
DAG, no orphans). crates.io has no npm-style `@scope/name`; the `caliban-*`
prefix is already the correct Rust convention, and owning the flat `caliban`
root name future-proofs the project for RFC 3243 `caliban::*` namespacing.

## Part A — Crate-readiness (manifests)

### A1. Centralize internal deps in `[workspace.dependencies]`

Add every internal crate once to the root `Cargo.toml`, carrying both `path`
(root-relative) and `version`:

```toml
[workspace.dependencies]
caliban-common             = { path = "crates/caliban-common",             version = "0.1.0" }
caliban-provider           = { path = "crates/caliban-provider",           version = "0.1.0" }
caliban-provider-anthropic = { path = "crates/caliban-provider-anthropic", version = "0.1.0" }
caliban-provider-bedrock   = { path = "crates/caliban-provider-bedrock",   version = "0.1.0" }
caliban-provider-vertex    = { path = "crates/caliban-provider-vertex",    version = "0.1.0" }
caliban-provider-openai    = { path = "crates/caliban-provider-openai",    version = "0.1.0" }
caliban-provider-ollama    = { path = "crates/caliban-provider-ollama",    version = "0.1.0" }
caliban-provider-google    = { path = "crates/caliban-provider-google",    version = "0.1.0" }
caliban-agent-core         = { path = "crates/caliban-agent-core",         version = "0.1.0" }
caliban-tools-builtin      = { path = "crates/caliban-tools-builtin",      version = "0.1.0" }
caliban-sessions           = { path = "crates/caliban-sessions",           version = "0.1.0" }
caliban-checkpoint         = { path = "crates/caliban-checkpoint",         version = "0.1.0" }
caliban-memory             = { path = "crates/caliban-memory",             version = "0.1.0" }
caliban-output-styles      = { path = "crates/caliban-output-styles",      version = "0.1.0" }
caliban-skills             = { path = "crates/caliban-skills",             version = "0.1.0" }
caliban-mcp-client         = { path = "crates/caliban-mcp-client",         version = "0.1.0" }
caliban-model-router       = { path = "crates/caliban-model-router",       version = "0.1.0" }
caliban-sandbox            = { path = "crates/caliban-sandbox",            version = "0.1.0" }
caliban-plugins            = { path = "crates/caliban-plugins",            version = "0.1.0" }
caliban-telemetry          = { path = "crates/caliban-telemetry",          version = "0.1.0" }
caliban-images             = { path = "crates/caliban-images",             version = "0.1.0" }
caliban-worktrees          = { path = "crates/caliban-worktrees",          version = "0.1.0" }
caliban-supervisor         = { path = "crates/caliban-supervisor",         version = "0.1.0" }
caliban-settings           = { path = "crates/caliban-settings",           version = "0.1.0" }
```

In every member manifest, rewrite each internal dependency from
`caliban-foo = { path = "../caliban-foo" }` (and the binary's
`{ path = "../crates/caliban-foo" }`) to `caliban-foo = { workspace = true }`.
Feature-carrying references keep their features:
`caliban-provider = { workspace = true, features = ["mock"] }` (the binary's
`[dev-dependencies]` mock case). Apply to `[dependencies]`,
`[dev-dependencies]`, and any `[build-dependencies]`.

Result: the version is declared in exactly one place, and `cargo publish` gets
the registry version it requires for every inter-crate edge.

### A2. Drop `publish = false`

Remove the `publish = false` line from all 25 manifests — the 24 libs **and**
the `caliban` binary (publishing the binary is the objective).

### A3. Workspace-level metadata

In `[workspace.package]` add:

```toml
repository = "https://github.com/caliban-ai/caliban"
homepage   = "https://github.com/caliban-ai/caliban"
```

Each crate inherits with `repository.workspace = true` (and `homepage.workspace
= true` where useful). `license`, `authors`, `edition`, `rust-version`,
`version` are already inherited. The existing placeholder comment about setting
`repository` "when the repo is made public" is resolved by this change.

### A4. Binary discovery metadata

On the `caliban` crate add crates.io discovery fields:

```toml
keywords    = ["agent", "ai", "llm", "cli", "claude"]
categories  = ["command-line-utilities", "development-tools"]
readme      = "README.md"
```

Create a concise `caliban/README.md` (install + quickstart oriented). `readme`
must point inside the crate directory — a root-relative `../README.md` would not
be packaged, so the binary crate gets its own README file. The repo-root
`README.md` is unchanged.

### A5. Internal-unstable framing

Append a stability disclaimer to each of the 24 libs' `description`, e.g.:

> `"… — internal crate for the caliban binary; no API stability, pin exact versions."`

This mirrors the nushell ("internal protocols") and deno (`deno_lib`: "highly
unstable"; `deno_path_util`: "does not follow semver … pin to a patch version")
contract. No docs.rs `documentation` field is curated for the libs. Versioning
stays lockstep at the workspace version.

### A6. Ownership / namespace future-proofing (one-time, post-publish)

Documented manual step, **not** in CI: after the first successful publish, add
the `caliban-ai` GitHub team as an owner on every crate:

```
cargo owner --add github:caliban-ai:<team> <crate>   # for each of the 25
```

This gives tokio-style grouped ownership and — because owning the flat `caliban`
root name is the prerequisite for RFC 3243 `caliban::*` namespacing — positions
the project to adopt enforced namespaces if/when they ship.

## Part B — Repo-guarded publish workflow

New file: `.github/workflows/publish.yml`.

- **Trigger:** push of a tag matching `v*` (e.g. `v0.1.0`). Chosen over manual
  dispatch / GitHub Release as the standard Rust convention.
- **Guards (fail-closed, defense in depth):**
  1. **Repo guard** — job-level `if: github.repository == 'caliban-ai/caliban'`.
     The entire job is skipped on the private `origin`, on forks, anywhere else.
  2. **Token guard** — `CARGO_REGISTRY_TOKEN` is configured as a repository
     secret **only** in `caliban-ai/caliban`. Even if guard 1 were removed, no
     token exists elsewhere, so publishing fails. This is the hard backstop.
  3. **Version-match guard** — a step parses the tag (`v0.1.0` → `0.1.0`) and
     asserts it equals `[workspace.package].version`; mismatch fails the job
     before any publish.
- **Steps:**
  1. `actions/checkout@v5`
  2. `dtolnay/rust-toolchain@stable` (reads the 1.95.0 pin from
     `rust-toolchain.toml`)
  3. `Swatinem/rust-cache@v2`
  4. Version-match check
  5. `cargo publish --workspace --dry-run` (final pre-flight)
  6. `cargo publish --workspace` with `CARGO_REGISTRY_TOKEN` in env — cargo
     resolves topological order and waits for each crate to be available on the
     registry before publishing its dependents.
- **Permissions:** `contents: read` only.
- **System deps:** none beyond the base ubuntu runner — this mirrors the
  existing `ci.yml` workspace build, which already compiles the default
  `clipboard`/arboard path on `ubuntu-latest` without an apt step.

`cargo publish --workspace` is available in the pinned cargo 1.95.0 (verified:
`--workspace`, `-p`, `--exclude`, `--dry-run` are present).

## Part C — Packaging dry-run in CI

Add a `package-check` job (in `ci.yml` or a small dedicated workflow) that runs
`cargo publish --workspace --dry-run` on pull requests touching `**/Cargo.toml`
or `src/**`. This surfaces packaging breakage (a dependency missing its version,
an excluded file, a metadata error) on the PR rather than at release time. It
reuses the same toolchain + cache setup as the existing `check` job.

## Part D — Release runbook (documentation)

Document the human release procedure:

1. Bump `[workspace.package].version` to `X.Y.Z`; commit.
2. Push the commit to `caliban-ai/caliban` (the `public` remote).
3. Tag `vX.Y.Z` on that commit and push the tag to `public`.
4. The `publish.yml` workflow runs the guards and publishes all 25 crates.

Plus the one-time setup notes:

- Create the `CARGO_REGISTRY_TOKEN` repository secret in `caliban-ai/caliban`
  (a crates.io API token, ideally scoped to publish per RFC 2947 once the crate
  names exist).
- After the first publish, run the Part A6 `cargo owner` team additions.

## Data flow

```
git tag vX.Y.Z (push to caliban-ai/caliban)
  └─> GitHub Actions: publish.yml
        ├─ guard: repository == caliban-ai/caliban
        ├─ guard: tag version == workspace version
        ├─ guard: CARGO_REGISTRY_TOKEN present
        ├─ cargo publish --workspace --dry-run
        └─ cargo publish --workspace   (topological, waits per crate)
              └─> crates.io holds all 25 crates at vX.Y.Z
                    └─> `cargo install caliban` works for end users
```

## Error handling

- **Wrong repo / fork:** job skipped by the repo guard; no token present anyway.
- **Version mismatch:** version-match guard fails before any upload.
- **Partial publish failure (the one real operational risk):** crates.io
  releases are immutable. If `cargo publish --workspace` fails after publishing
  some crates, a naive re-run errors with "crate already published at this
  version." **Recovery:** re-run publishing only the remaining crates via
  `cargo publish -p <remaining> …`, or bump the patch version and retag. This is
  the native-tooling tradeoff and is documented in the runbook. If recovery ever
  becomes painful, `release-plz` automates it — explicitly deferred (YAGNI).
- **Token scope:** prefer a crates.io token scoped to these crate names once
  they exist (RFC 2947 token scopes).

## Testing

- `cargo publish --workspace --dry-run` is the verification mechanism — run
  locally during implementation and in CI (Part C) on every relevant PR.
- The repo guard is standard GitHub Actions expression logic; not unit-testable,
  but the token guard provides an independent fail-closed backstop.

## Explicitly out of scope (YAGNI)

- No `release-plz` / `cargo-release` / changelog automation; no auto
  version-bump release PRs.
- No GitHub Release creation, prebuilt-binary artifacts, or `cargo-dist` +
  `cargo-binstall` (a separate future distribution concern).
- No crate-boundary changes (the current 24-lib layout is locked).
- No promotion of any lib to a stable public SDK contract (deferred to real
  external demand).
