# crates.io Publishing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the caliban workspace publishable to crates.io and add a publish pipeline that runs *only* from `caliban-ai/caliban`, so users can `cargo install caliban`.

**Architecture:** Centralize inter-crate deps + versions in `[workspace.dependencies]`, drop `publish = false` on all 25 crates, add discovery/internal-unstable metadata, then add a tag-triggered `publish.yml` guarded three ways (repo `if`, repo-only `CARGO_REGISTRY_TOKEN` secret, tag↔version match) that runs `cargo publish --workspace`. A CI dry-run catches packaging breakage on PRs.

**Tech Stack:** Rust 2024 / cargo 1.95 (`cargo publish --workspace`), GitHub Actions, `cargo metadata` + `jq`.

**Design source:** `docs/superpowers/specs/2026-06-05-crates-io-publishing-design.md`

**Verification note:** There is no application code here, so tasks are verified with `cargo`/`grep`/`actionlint` commands and their expected output rather than unit tests.

**Internal-crate dependency map** (used to verify Task 2 — each lib's internal deps):

| Crate | Internal deps |
|---|---|
| caliban-common | *(none — leaf)* |
| caliban-provider | *(none — leaf)* |
| caliban-sandbox | *(none — leaf)* |
| caliban-worktrees | *(none — leaf)* |
| caliban-supervisor | *(none — leaf)* |
| caliban-provider-anthropic | common, provider |
| caliban-provider-google | common, provider |
| caliban-provider-ollama | common, provider |
| caliban-provider-openai | common, provider |
| caliban-provider-bedrock | common, provider, provider-anthropic |
| caliban-provider-vertex | common, provider, provider-anthropic |
| caliban-memory | common |
| caliban-telemetry | common, provider |
| caliban-agent-core | common, provider |
| caliban-images | common, provider |
| caliban-model-router | common, memory, provider |
| caliban-sessions | agent-core, common, provider |
| caliban-checkpoint | agent-core, common, provider, sessions |
| caliban-mcp-client | agent-core, common, provider |
| caliban-output-styles | agent-core, common |
| caliban-skills | agent-core, common, provider |
| caliban-tools-builtin | agent-core, common, memory, provider, sandbox |
| caliban-settings | agent-core, common, mcp-client |
| caliban-plugins | common |

---

## Task 1: Root manifest — centralize internal deps + workspace metadata

**Files:**
- Modify: `Cargo.toml` (the `[workspace.package]` and `[workspace.dependencies]` blocks)

- [ ] **Step 1: Add `repository` + `homepage` to `[workspace.package]`**

In `Cargo.toml`, inside `[workspace.package]` (currently holds `version`, `edition`, `license`, `authors`, `rust-version`), add two lines after `rust-version`:

```toml
repository = "https://github.com/caliban-ai/caliban"
homepage   = "https://github.com/caliban-ai/caliban"
```

- [ ] **Step 2: Add internal crates to `[workspace.dependencies]`**

At the top of the existing `[workspace.dependencies]` block in `Cargo.toml`, add all 24 internal crates with root-relative `path` + `version`:

```toml
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

- [ ] **Step 3: Allow `cargo_common_metadata` workspace-wide**

Removing `publish = false` (Tasks 2–3) activates clippy's `cargo_common_metadata`, which `-D warnings` (used in CI) promotes to an error demanding `readme`/`keywords`/`categories` on *every* crate. The internal libs deliberately don't carry curated metadata, so allow the lint. Add to `[workspace.lints.clippy]` in `Cargo.toml`:

```toml
cargo_common_metadata = "allow"
```

- [ ] **Step 4: Verify the workspace still resolves**

Run: `cargo metadata --no-deps --format-version 1 > /dev/null && echo OK`
Expected: `OK` (adding unreferenced workspace deps + metadata is inert; members still use their own path deps at this point).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml
git commit -m "build: centralize internal crate deps + add workspace repository metadata"
```

---

## Task 2: Convert the 24 library manifests

**Files:**
- Modify: `crates/*/Cargo.toml` (all 24 library crates)

**Uniform transformation rule** applied to every `crates/<name>/Cargo.toml`:

1. **Internal deps → workspace form.** Any dependency line referencing another caliban crate, in `[dependencies]`, `[dev-dependencies]`, or `[build-dependencies]`, changes from path form to workspace form, **preserving features**:
   - `caliban-common = { path = "../caliban-common" }` → `caliban-common = { workspace = true }`
   - `caliban-provider = { path = "../caliban-provider", features = ["mock"] }` → `caliban-provider = { workspace = true, features = ["mock"] }`
2. **Drop** the `publish     = false` line entirely.
3. **Add** `repository.workspace = true` in `[package]` (next to the existing `license.workspace = true`).
4. **Append the internal-unstable suffix** to the `description` value (inside the quotes), exactly:
   `  — internal crate for the caliban binary; no API stability, pin exact versions`

- [ ] **Step 1: Worked example — a leaf crate (`caliban-common`)**

Edit `crates/caliban-common/Cargo.toml` `[package]` block to:

```toml
[package]
name        = "caliban-common"
version.workspace = true
description = "Cross-crate plumbing (paths, env-var expansion, atomic writes, globs, tracing targets) for caliban — internal crate for the caliban binary; no API stability, pin exact versions"
edition.workspace      = true
license.workspace      = true
repository.workspace   = true
authors.workspace      = true
rust-version.workspace = true
```

(`publish = false` removed; `caliban-common` has no internal deps so only steps 2–4 of the rule apply.)

- [ ] **Step 2: Worked example — a multi-dep crate (`caliban-tools-builtin`)**

In `crates/caliban-tools-builtin/Cargo.toml`: remove `publish = false`, add `repository.workspace = true`, append the suffix to `description`, and convert its five internal deps:

```toml
caliban-agent-core    = { workspace = true }
caliban-common        = { workspace = true }
caliban-memory        = { workspace = true }
caliban-provider      = { workspace = true }
caliban-sandbox       = { workspace = true }
```

(Preserve any `features = [...]` already present on those lines — e.g. a dev-dependency `caliban-provider = { workspace = true, features = ["mock"] }`.)

- [ ] **Step 3: Apply the rule to all remaining lib crates**

Apply the same four-part rule to each of the other 22 lib manifests. Use the dependency map at the top of this plan to confirm each crate's internal deps were all converted. The crates: `caliban-provider`, `caliban-sandbox`, `caliban-worktrees`, `caliban-supervisor`, `caliban-provider-anthropic`, `caliban-provider-bedrock`, `caliban-provider-vertex`, `caliban-provider-openai`, `caliban-provider-ollama`, `caliban-provider-google`, `caliban-agent-core`, `caliban-sessions`, `caliban-checkpoint`, `caliban-memory`, `caliban-output-styles`, `caliban-skills`, `caliban-mcp-client`, `caliban-model-router`, `caliban-plugins`, `caliban-telemetry`, `caliban-images`, `caliban-settings`.

- [ ] **Step 4: Verify no internal path deps and no `publish = false` remain in libs**

Run:
```bash
rg -n 'path = "\.\.' crates/*/Cargo.toml; echo "---"; rg -n 'publish\s*=\s*false' crates/*/Cargo.toml; echo "done"
```
Expected: no `path = ".."` matches and no `publish = false` matches in `crates/*` — only the literal `done` (and `---`) printed.

- [ ] **Step 5: Verify the workspace still builds**

Run: `cargo build --workspace`
Expected: builds successfully (workspace deps resolve to the same local crates).

---

## Task 3: Convert the binary manifest + add its README

**Files:**
- Modify: `caliban/Cargo.toml`
- Create: `caliban/README.md`

- [ ] **Step 1: Convert `caliban/Cargo.toml` `[package]` block**

Remove `publish     = false`. Add `repository.workspace = true` plus discovery metadata. The `[package]` block becomes:

```toml
[package]
name        = "caliban"
version.workspace = true
description = "User-facing binary for the caliban agent harness"
edition.workspace      = true
license.workspace      = true
repository.workspace   = true
authors.workspace      = true
rust-version.workspace = true
keywords    = ["agent", "ai", "llm", "cli", "claude"]
categories  = ["command-line-utilities", "development-tools"]
readme      = "README.md"
```

(The binary keeps its real `description` — no internal-unstable suffix; it is the product.)

- [ ] **Step 2: Convert the binary's internal deps to workspace form**

In `caliban/Cargo.toml`, convert every `caliban-* = { path = "../crates/caliban-*" }` line in `[dependencies]` to `{ workspace = true }`, and the `[dev-dependencies]` line `caliban-provider = { path = "../crates/caliban-provider", features = ["mock"] }` to:

```toml
caliban-provider = { workspace = true, features = ["mock"] }
```

- [ ] **Step 3: Create `caliban/README.md`**

```markdown
# caliban

`caliban` is a provider-agnostic AI agent harness — a terminal-first coding
agent that talks to Anthropic, OpenAI, Google, Ollama, and cloud backends
(Bedrock, Vertex) through one interface.

## Install

```sh
cargo install caliban
```

## Usage

Run `caliban` in a project directory and follow the prompts. See the
[repository](https://github.com/caliban-ai/caliban) for configuration,
providers, and the full user guide.

## License

AGPL-3.0-only.
```

- [ ] **Step 4: Verify no path deps / no `publish = false` remain in the binary**

Run:
```bash
rg -n 'path = "\.\.|publish\s*=\s*false' caliban/Cargo.toml; echo done
```
Expected: only `done` printed (no matches).

---

## Task 4: Full workspace verification + commit manifests

**Files:** none (verification + commit)

- [ ] **Step 1: Build the whole workspace**

Run: `cargo build --workspace`
Expected: success.

- [ ] **Step 1b: Clippy with `-D warnings` (matches CI; catches `cargo_common_metadata`)**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: success — no `cargo_common_metadata` errors (allowed in Task 1) and no other lint regressions.

- [ ] **Step 2: Packaging dry-run across the workspace**

Run: `cargo publish --workspace --dry-run`
Expected: cargo packages and verify-builds every crate in dependency order with no "all dependencies must have a version specified" errors and no "publishing is not allowed" errors. (This is the real integration check that the manifests are publish-ready.)

- [ ] **Step 3: Commit the manifest conversion**

```bash
git add Cargo.toml crates/*/Cargo.toml caliban/Cargo.toml caliban/README.md
git commit -m "build: make all crates publishable (workspace deps, drop publish=false, metadata)"
```

---

## Task 5: Repo-guarded publish workflow

**Files:**
- Create: `.github/workflows/publish.yml`

- [ ] **Step 1: Write `publish.yml`**

```yaml
name: publish

on:
  push:
    tags:
      - "v*"

concurrency:
  group: publish-${{ github.ref }}
  cancel-in-progress: false

permissions:
  contents: read

jobs:
  publish:
    name: cargo publish --workspace
    # Guard 1: never run anywhere but the canonical public repo.
    if: github.repository == 'caliban-ai/caliban'
    runs-on: ubuntu-latest
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v5

      - uses: dtolnay/rust-toolchain@stable

      - uses: Swatinem/rust-cache@v2

      - name: Verify tag matches workspace version
        # Guard 3: tag v0.1.0 must equal the workspace package version.
        run: |
          tag="${GITHUB_REF_NAME#v}"
          ws="$(cargo metadata --no-deps --format-version 1 \
                | jq -r '.packages[] | select(.name=="caliban") | .version')"
          echo "tag=$tag workspace=$ws"
          if [ "$tag" != "$ws" ]; then
            echo "::error::tag v$tag does not match workspace version $ws"
            exit 1
          fi

      - name: Package dry-run
        run: cargo publish --workspace --dry-run

      - name: Publish to crates.io
        # Guard 2: token exists ONLY as a secret in caliban-ai/caliban.
        env:
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
        run: cargo publish --workspace
```

- [ ] **Step 2: Lint the workflow (if `actionlint` is available)**

Run: `command -v actionlint >/dev/null && actionlint .github/workflows/publish.yml || echo "actionlint not installed — skipping"`
Expected: no errors reported (or the skip message).

- [ ] **Step 3: Verify YAML parses**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/publish.yml')); print('valid yaml')"`
Expected: `valid yaml`

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/publish.yml
git commit -m "ci: tag-triggered crates.io publish guarded to caliban-ai/caliban"
```

---

## Task 6: Packaging dry-run in CI

**Files:**
- Modify: `.github/workflows/ci.yml` (add a `package-check` job)

- [ ] **Step 1: Add the `package-check` job**

Append this job under `jobs:` in `.github/workflows/ci.yml` (sibling of the existing `check` job):

```yaml
  package-check:
    name: cargo publish --workspace --dry-run
    runs-on: ubuntu-latest
    timeout-minutes: 20
    steps:
      - uses: actions/checkout@v5
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Verify all crates package cleanly
        run: cargo publish --workspace --dry-run
```

- [ ] **Step 2: Verify YAML parses**

Run: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml')); print('valid yaml')"`
Expected: `valid yaml`

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: dry-run cargo publish on PRs to catch packaging breakage"
```

---

## Task 7: Release runbook

**Files:**
- Create: `docs/releasing.md`

- [ ] **Step 1: Write `docs/releasing.md`**

```markdown
# Releasing caliban to crates.io

caliban publishes the binary plus its internal library crates to crates.io from
the **`caliban-ai/caliban`** public repository only. Publishing is guarded three
ways (see `.github/workflows/publish.yml`): a repo `if`, a repo-only
`CARGO_REGISTRY_TOKEN` secret, and a tag↔version check.

## One-time setup

1. Create a crates.io API token (ideally scoped to the `caliban` / `caliban-*`
   crates once they exist) and add it as the `CARGO_REGISTRY_TOKEN` repository
   secret in `caliban-ai/caliban` — **and nowhere else**.
2. After the first publish, add the org team as an owner on every crate so
   ownership is shared and the `caliban` root name is org-held (this also
   future-proofs RFC 3243 `caliban::*` namespacing):

   ```sh
   for c in caliban caliban-common caliban-provider caliban-provider-anthropic \
            caliban-provider-bedrock caliban-provider-vertex caliban-provider-openai \
            caliban-provider-ollama caliban-provider-google caliban-agent-core \
            caliban-tools-builtin caliban-sessions caliban-checkpoint caliban-memory \
            caliban-output-styles caliban-skills caliban-mcp-client caliban-model-router \
            caliban-sandbox caliban-plugins caliban-telemetry caliban-images \
            caliban-worktrees caliban-supervisor caliban-settings; do
     cargo owner --add github:caliban-ai:<team> "$c"
   done
   ```

## Cutting a release

1. Bump `version` in the root `Cargo.toml` `[workspace.package]` to `X.Y.Z`.
2. Commit and push to the `public` remote (`caliban-ai/caliban`).
3. Tag and push:

   ```sh
   git tag vX.Y.Z
   git push public vX.Y.Z
   ```

4. The `publish` workflow validates the guards and runs
   `cargo publish --workspace`, which uploads all crates in dependency order,
   waiting for each to become available before publishing its dependents.

## If a publish fails partway

crates.io releases are immutable, so already-published crates cannot be
re-uploaded at the same version. To recover:

- Re-run publishing only the crates that did not upload:
  `cargo publish -p <crate-a> -p <crate-b> …`, **or**
- Bump the patch version (`X.Y.Z+1`), retag, and re-run the workflow.
```

- [ ] **Step 2: Commit**

```bash
git add docs/releasing.md
git commit -m "docs: crates.io release runbook"
```

---

## Final verification

- [ ] **Step 1: Confirm full history of the branch**

Run: `git log --oneline main..HEAD`
Expected: the design-spec commit plus the five implementation commits (Tasks 1, 4, 5, 6, 7).

- [ ] **Step 2: Re-run the packaging dry-run from a clean state**

Run: `cargo publish --workspace --dry-run`
Expected: success — every crate packages and verify-builds.
