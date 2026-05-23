# Layer 0 (Bootstrap) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the `caliban` Cargo workspace skeleton, five ADRs, README, and CI workflow so that subsequent Layer-1 crates (provider, agent-core, tools-builtin) can be added with `cargo new` + a one-line workspace-members edit.

**Architecture:** Cargo workspace at the repo root. Two initial members: library `caliban-core` at `crates/caliban-core/` (seeds the trait-crate where the provider trait will eventually live) and binary `caliban` at `caliban/`. Cross-cutting config (lints, dependency versions, package metadata) inherited from workspace root via `[workspace.lints] workspace = true` and `[workspace.dependencies]`. CI runs `fmt → clippy → build → test` on every push and PR. ADRs document the cross-cutting decisions in MADR-lite format.

**Tech Stack:** Rust 1.85.0 (edition 2024), Cargo workspace, `tokio` (full features), `thiserror` 1, `anyhow` 1, `clippy` (pedantic, denied), `rustfmt` (defaults), GitHub Actions, `dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache@v2`.

**Spec:** [`docs/superpowers/specs/2026-05-22-layer-0-bootstrap-design.md`](../specs/2026-05-22-layer-0-bootstrap-design.md)

---

## File Structure

Files this plan creates or modifies, grouped by the task that owns them:

```
Cargo.toml                                # Task 1
rust-toolchain.toml                       # Task 1
rustfmt.toml                              # Task 1
LICENSE                                   # Task 1
.gitignore                                # Task 1 (augment existing)
crates/caliban-core/Cargo.toml            # Task 2
crates/caliban-core/src/lib.rs            # Task 2
caliban/Cargo.toml                        # Task 3
caliban/src/main.rs                       # Task 3
caliban/tests/cli.rs                      # Task 3
adrs/README.md                            # Task 4
adrs/0001-async-runtime.md                # Task 4
adrs/0002-error-model.md                  # Task 4
adrs/0003-license-agpl-3.0.md             # Task 4
adrs/0004-naming-conventions.md           # Task 4
adrs/0005-workspace-layout.md             # Task 4
README.md                                 # Task 5
.github/workflows/ci.yml                  # Task 6
```

**Responsibility per file:**
- Workspace root (`Cargo.toml`, `rust-toolchain.toml`, `rustfmt.toml`) — shared config that every member inherits.
- `crates/caliban-core/` — library crate; placeholder for the future provider trait surface, present so the workspace has ≥1 lib member from day one.
- `caliban/` — binary crate, the user-facing entrypoint. Owns CLI argument parsing and `main`. Will grow into the orchestrator-driver later.
- `adrs/` — durable architectural decisions. One file per decision, plus an index.
- `.github/workflows/ci.yml` — pre-merge gate. Single job, no matrix yet.
- `LICENSE`, `README.md`, `.gitignore` — repo hygiene.

---

## Task 1: Workspace root configuration & static files

**Files:**
- Create: `Cargo.toml`
- Create: `rust-toolchain.toml`
- Create: `rustfmt.toml`
- Create: `LICENSE`
- Modify: `.gitignore`

- [ ] **Step 1: Verify starting state**

Run: `ls -la`
Expected: shows `.claude/`, `.git/`, `.gitignore`, `.superpowers/`, `docs/`. No `Cargo.toml` yet.

- [ ] **Step 2: Create the workspace `Cargo.toml`**

Create `Cargo.toml` at the repo root with this exact content:

```toml
[workspace]
resolver = "2"
members = []

[workspace.package]
edition = "2024"
license = "AGPL-3.0-only"
authors = ["John Ford <john.ford2002@gmail.com>"]
rust-version = "1.85"
# repository = TODO: set once the GitHub repo exists

[workspace.dependencies]
tokio     = { version = "1", features = ["full"] }
thiserror = "1"
anyhow    = "1"

[workspace.lints.rust]
unsafe_code      = "forbid"
missing_docs     = "warn"
unreachable_pub  = "warn"
rust_2018_idioms = "warn"

[workspace.lints.clippy]
all      = { level = "deny", priority = -1 }
pedantic = { level = "deny", priority = -1 }
cargo    = "warn"
module_name_repetitions = "allow"
must_use_candidate      = "allow"
```

Note: `members = []` is intentional; Tasks 2 and 3 will append member paths.

- [ ] **Step 3: Create `rust-toolchain.toml`**

```toml
[toolchain]
channel    = "1.85.0"
components = ["rustfmt", "clippy"]
profile    = "minimal"
```

- [ ] **Step 4: Create `rustfmt.toml`**

```toml
edition = "2024"
```

(Otherwise stdlib `rustfmt` defaults. Empty file would also work, but explicit `edition` is clearer.)

- [ ] **Step 5: Create `LICENSE`**

Download the canonical AGPL-3.0 text into `LICENSE`. Run:

```bash
curl -fsSL https://www.gnu.org/licenses/agpl-3.0.txt -o LICENSE
```

Verify: `head -3 LICENSE` should show:

```
                    GNU AFFERO GENERAL PUBLIC LICENSE
                       Version 3, 19 November 2007

```

If `curl` is unavailable, copy the AGPL-3.0 text from https://www.gnu.org/licenses/agpl-3.0.txt manually.

- [ ] **Step 6: Augment `.gitignore`**

Read the current `.gitignore`. It contains:

```
.superpowers/
```

Append Rust-specific entries so the file reads:

```
# Rust
target/
**/*.rs.bk
Cargo.lock.bak

# Editor / OS
*.swp
.DS_Store

# Superpowers brainstorm artifacts
.superpowers/
```

Note: `Cargo.lock` is *not* ignored — workspaces with binaries commit it.

- [ ] **Step 7: Verify the workspace parses**

Run: `cargo metadata --format-version 1 --no-deps 2>&1 | head -5`
Expected: JSON output beginning with `{"packages":[]`. No error.

If `cargo` is not on PATH, run: `rustup which cargo` to confirm the toolchain is available. The `rust-toolchain.toml` will trigger toolchain install on first `cargo` invocation, which may take a minute.

- [ ] **Step 8: Verify cargo check**

Run: `cargo check --workspace`
Expected: exits 0 with no output (no members to check).

- [ ] **Step 9: Commit**

```bash
git add Cargo.toml rust-toolchain.toml rustfmt.toml LICENSE .gitignore
git commit -m "$(cat <<'EOF'
feat: workspace root config + AGPL-3.0 license

Establishes the Cargo workspace skeleton with shared package metadata,
workspace.dependencies for tokio/thiserror/anyhow, strict workspace.lints
(unsafe forbidden, clippy pedantic denied), AGPL-3.0 license file, and
pinned Rust 1.85.0 toolchain.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `caliban-core` library crate

**Files:**
- Create: `crates/caliban-core/Cargo.toml`
- Create: `crates/caliban-core/src/lib.rs`
- Modify: `Cargo.toml` (workspace members)

- [ ] **Step 1: Write the failing test inline in `lib.rs`**

Create `crates/caliban-core/src/lib.rs`:

```rust
//! Foundational types and traits for the caliban agent harness.
//!
//! Layer-0 placeholder. The `Provider` trait and shared message types
//! will land here in Layer 1.

#[cfg(test)]
mod tests {
    #[test]
    fn smoke_test_harness_runs() {
        // Exists solely to confirm the test harness is wired up.
        assert_eq!(2 + 2, 4);
    }
}
```

- [ ] **Step 2: Create the crate's `Cargo.toml`**

Create `crates/caliban-core/Cargo.toml`:

```toml
[package]
name        = "caliban-core"
version     = "0.0.0"
description = "Foundational types and traits for the caliban agent harness"
edition.workspace      = true
license.workspace      = true
authors.workspace      = true
rust-version.workspace = true
publish     = false

[lints]
workspace = true
```

`publish = false` prevents accidental `cargo publish` until we explicitly choose to release.

- [ ] **Step 3: Verify the crate is not yet recognized by the workspace**

Run: `cargo test -p caliban-core 2>&1 | head -5`
Expected: ERROR like `package ID specification 'caliban-core' did not match any packages`.

This proves the test we just wrote isn't yet runnable — it's the "failing test" state.

- [ ] **Step 4: Add the crate to workspace members**

Edit `Cargo.toml` at repo root. Change:

```toml
members = []
```

to:

```toml
members = [
    "crates/caliban-core",
]
```

- [ ] **Step 5: Run the test, verify it passes**

Run: `cargo test -p caliban-core`
Expected:

```
   Compiling caliban-core v0.0.0 ...
    Finished test [unoptimized + debuginfo] target(s) ...
     Running unittests src/lib.rs ...

running 1 test
test tests::smoke_test_harness_runs ... ok

test result: ok. 1 passed; 0 failed; ...
```

- [ ] **Step 6: Verify lints inherit from workspace**

Run: `cargo clippy -p caliban-core --all-targets -- -D warnings`
Expected: exits 0, no warnings.

- [ ] **Step 7: Verify `unsafe_code = "forbid"` is active**

Temporarily append to `crates/caliban-core/src/lib.rs`:

```rust
fn _try_unsafe() {
    unsafe { std::ptr::null::<u8>(); }
}
```

Run: `cargo build -p caliban-core 2>&1 | head -10`
Expected: compile error containing `unsafe code` or `unsafe_code` lint denial.

Then **remove** the `_try_unsafe` function from `lib.rs`. Run `cargo build -p caliban-core` again and confirm it succeeds.

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml crates/caliban-core/
git commit -m "$(cat <<'EOF'
feat: add caliban-core library crate

Placeholder library that seeds the workspace's first member and proves
the test harness, workspace lints inheritance, and unsafe_code = "forbid"
all wire through correctly.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `caliban` binary crate

**Files:**
- Create: `caliban/Cargo.toml`
- Create: `caliban/src/main.rs`
- Create: `caliban/tests/cli.rs`
- Modify: `Cargo.toml` (workspace members)

- [ ] **Step 1: Write the failing integration test first**

Create `caliban/tests/cli.rs`:

```rust
//! Integration tests for the `caliban` binary.

use std::process::Command;

#[test]
fn version_flag_prints_version_and_exits_zero() {
    let exe = env!("CARGO_BIN_EXE_caliban");
    let output = Command::new(exe)
        .arg("--version")
        .output()
        .expect("failed to invoke caliban binary");

    assert!(
        output.status.success(),
        "expected exit 0, got {:?}; stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout is not UTF-8");
    assert!(
        stdout.starts_with("caliban "),
        "expected stdout to start with 'caliban ', got: {stdout:?}",
    );

    let expected_version = env!("CARGO_PKG_VERSION");
    assert!(
        stdout.contains(expected_version),
        "expected stdout to contain version {expected_version:?}, got: {stdout:?}",
    );
}
```

The `CARGO_BIN_EXE_caliban` env var is set automatically by Cargo when building integration tests; it points to the compiled binary.

- [ ] **Step 2: Create the binary `Cargo.toml`**

Create `caliban/Cargo.toml`:

```toml
[package]
name        = "caliban"
version     = "0.0.0"
description = "User-facing binary for the caliban agent harness"
edition.workspace      = true
license.workspace      = true
authors.workspace      = true
rust-version.workspace = true
publish     = false

[[bin]]
name = "caliban"
path = "src/main.rs"

[dependencies]
anyhow = { workspace = true }

[lints]
workspace = true
```

- [ ] **Step 3: Create a minimal `main.rs` that does NOT yet handle --version**

Create `caliban/src/main.rs`:

```rust
//! caliban — agent harness binary entrypoint.

fn main() -> anyhow::Result<()> {
    Ok(())
}
```

This compiles but exits 0 with no output — the test will fail on the "starts with 'caliban '" assertion.

- [ ] **Step 4: Add the binary to workspace members**

Edit root `Cargo.toml`. Change:

```toml
members = [
    "crates/caliban-core",
]
```

to:

```toml
members = [
    "crates/caliban-core",
    "caliban",
]
```

- [ ] **Step 5: Run the failing integration test**

Run: `cargo test -p caliban --test cli`
Expected: test compiles and runs, **fails** with a message like:

```
assertion failed: stdout.starts_with("caliban ")
  expected stdout to start with 'caliban ', got: ""
```

- [ ] **Step 6: Implement `--version` handling in `main.rs`**

Replace `caliban/src/main.rs` with:

```rust
//! caliban — agent harness binary entrypoint.

use std::env;
use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const NAME: &str = env!("CARGO_PKG_NAME");

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("--version" | "-V") => {
            println!("{NAME} {VERSION}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown argument: {other}");
            ExitCode::from(2)
        }
        None => {
            eprintln!("caliban: no command given (this is a Layer-0 stub)");
            ExitCode::from(2)
        }
    }
}
```

Note: we don't introduce `clap` yet — Layer 0 keeps the binary dependency-free beyond `anyhow`. A real argument parser comes in with the CLI crate later.

Note: `anyhow` is in `Cargo.toml` for future use but isn't imported by this stub. That's fine; cargo only warns on truly unused deps via `cargo udeps` (a separate tool), not the standard build.

- [ ] **Step 7: Run the integration test, verify it passes**

Run: `cargo test -p caliban --test cli`
Expected:

```
running 1 test
test version_flag_prints_version_and_exits_zero ... ok

test result: ok. 1 passed; 0 failed; ...
```

- [ ] **Step 8: Verify the binary prints expected output manually**

Run: `cargo run --bin caliban -- --version`
Expected stdout: `caliban 0.0.0`
Expected exit code: 0 (verify with `echo $?`).

- [ ] **Step 9: Verify clippy is clean across the workspace**

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: exits 0 with no warnings.

If clippy complains about `unused import: anyhow` or similar, that's fine — `anyhow` is reserved as a workspace dep we'll use in Task 3 sequels. If it's a hard error, add `#![allow(unused_imports)]` is **wrong** — instead, drop the `anyhow` line from `caliban/Cargo.toml` and re-run. (Clippy in workspace lints config will not warn on unused deps; only `cargo udeps` does.)

- [ ] **Step 10: Commit**

```bash
git add Cargo.toml caliban/
git commit -m "$(cat <<'EOF'
feat: add caliban binary with --version flag

Minimal Layer-0 stub. Parses --version/-V (printing "caliban <version>"
and exiting 0); any other input exits 2. No CLI library yet — clap and
friends arrive with the dedicated CLI crate in a later layer.

Integration test in caliban/tests/cli.rs verifies the version output
contract via CARGO_BIN_EXE_caliban.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: ADRs

**Files:**
- Create: `adrs/README.md`
- Create: `adrs/0001-async-runtime.md`
- Create: `adrs/0002-error-model.md`
- Create: `adrs/0003-license-agpl-3.0.md`
- Create: `adrs/0004-naming-conventions.md`
- Create: `adrs/0005-workspace-layout.md`

No tests — these are documentation. But there's still a verification step.

- [ ] **Step 1: Create `adrs/README.md` (the index)**

```markdown
# Architecture Decision Records

This directory contains durable architectural decisions for caliban, in
[MADR-lite](https://adr.github.io/madr/) format: each file states a single
decision with context, the decision itself, and consequences.

## Status legend

- **accepted** — the decision is currently in effect.
- **superseded** — the decision was replaced by a later ADR; the file is kept for history and links to its successor.
- **proposed** — under discussion; not in effect.
- **rejected** — considered and explicitly declined.

## Index

| # | Title | Status |
|---|---|---|
| [0001](0001-async-runtime.md) | Async runtime → `tokio` | accepted |
| [0002](0002-error-model.md) | Error model → `thiserror` for libs, `anyhow` for binary | accepted |
| [0003](0003-license-agpl-3.0.md) | License → `AGPL-3.0-only` | accepted |
| [0004](0004-naming-conventions.md) | Naming → `caliban-*` libraries, `caliban` binary | accepted |
| [0005](0005-workspace-layout.md) | Workspace layout → `crates/` for libs, binaries at root | accepted |

## Adding a new ADR

1. Pick the next available number.
2. Copy an existing ADR as a template.
3. Set status to `proposed` while open for discussion; flip to `accepted` once decided.
4. Add an entry to the table above.
```

- [ ] **Step 2: Create `adrs/0001-async-runtime.md`**

```markdown
# ADR 0001 · Async runtime → `tokio`

- **Status:** accepted
- **Date:** 2026-05-22

## Context

caliban's foundation is heavily I/O-bound: provider HTTPS calls, streaming
responses from LLM endpoints, MCP transports, and eventually a multi-session
orchestrator. Rust's async story is fragmented across runtimes (`tokio`,
`async-std`, `smol`, `embassy`), and futures from one runtime cannot
always be polled by another. Picking a runtime up front prevents subtle
cross-runtime breakage as the workspace grows.

## Decision

Standardize on `tokio` (multi-threaded scheduler, `features = ["full"]`)
across every crate in the workspace. The workspace root pins the version
in `[workspace.dependencies]`; member crates declare `tokio.workspace = true`
and may select their own feature subset.

No nested runtimes. Each binary creates a single `tokio::runtime::Runtime`
(or uses `#[tokio::main]`) for its entire lifetime.

## Consequences

- **Positive:** direct compatibility with `reqwest`, `tower`, `hyper`,
  `axum`, `tonic`, every major MCP transport, and most LLM SDKs.
  Predictable async behavior across the workspace. Easy onboarding —
  tokio is the de facto Rust async runtime.
- **Negative:** locks the workspace out of `smol`/`embassy` ecosystems
  (acceptable — no embedded targets planned). Binary size larger than
  a minimal runtime would produce.
- **Revisit if:** caliban needs to run in a `no_std` or `embassy`-only
  environment, or if a critical dependency requires a different runtime.
```

- [ ] **Step 3: Create `adrs/0002-error-model.md`**

```markdown
# ADR 0002 · Error model → `thiserror` for libraries, `anyhow` for binary

- **Status:** accepted
- **Date:** 2026-05-22

## Context

Rust libraries benefit from precise error enums — consumers want to
match on variants and react differently to different failure modes.
Binaries benefit from ergonomic context propagation — operators want
a readable error chain showing where things went wrong, not pattern-
matching on every variant.

A shared "uber error" crate that every other crate depends on creates
a foundation-coupling crate and forces every error change to ripple
through the workspace. We want errors to be local.

## Decision

Every `caliban-*` library crate defines its own `Error` enum using
`thiserror`, and exposes:

```rust
pub type Result<T> = std::result::Result<T, Error>;
```

Cross-crate errors convert at boundaries with `#[from]` or explicit
`From` impls. No shared error crate.

The `caliban` binary uses `anyhow::Result` in `main()` and top-level
command handlers. `?` propagates errors with context using
`.context("...")` from `anyhow::Context`.

## Consequences

- **Positive:** adding a new error variant is local to one crate.
  Library consumers can match precisely; binary code gets readable
  context. No god-error-crate.
- **Negative:** slight boilerplate per library (the `Error` enum
  and `Result` alias). `From` impls must be added at boundaries.
- **Revisit if:** a real shared error type emerges (e.g., a
  cross-crate "Cancelled" or "Timeout" that every layer must surface
  identically).
```

- [ ] **Step 4: Create `adrs/0003-license-agpl-3.0.md`**

```markdown
# ADR 0003 · License → `AGPL-3.0-only`

- **Status:** accepted
- **Date:** 2026-05-22

## Context

caliban is private now but designed to be open-sourced. The author
explicitly rejects permissive defaults (MIT, Apache-2.0): the goal is
to enforce community contribution from downstream users and hosted-
service providers, not maximize commercial adoption.

The relevant tiers of copyleft are:

- **GPL-3.0** — strong copyleft on distribution; SaaS providers can
  modify and host without releasing source (the "SaaS loophole").
- **AGPL-3.0** — closes the SaaS loophole: hosting modified code as
  a network service triggers the obligation to release source.
- **SSPL** — stronger than AGPL but not OSI-recognized as open source.
- **MPL-2.0** — file-level (weak) copyleft; consumers don't have to
  copyleft their downstream code.

## Decision

Every crate's `Cargo.toml` declares `license = "AGPL-3.0-only"` via
`license.workspace = true`. The full AGPL-3.0 text lives in `LICENSE`
at the workspace root. The README states the license prominently and
explains the implications for service operators and forks.

## Consequences

- **Positive:** forks and hosted services must release modifications.
  Aligns with Mastodon, Nextcloud, Grafana (pre-2018), MongoDB (pre-2018).
  Author's stated philosophy of community contribution is enforced.
- **Negative:** caliban crates won't compose into permissive Rust
  projects on crates.io — depending on `caliban-*` makes the consumer
  AGPL. This is *intentional*: caliban is an end product, not a
  general-purpose library to be embedded.
- **Revisit if:** the AGPL is preventing a legitimate non-commercial
  use case the author wants to support. A future ADR could carve out
  exceptions or dual-license specific crates.
```

- [ ] **Step 5: Create `adrs/0004-naming-conventions.md`**

```markdown
# ADR 0004 · Naming → `caliban-*` libraries, `caliban` binary

- **Status:** accepted
- **Date:** 2026-05-22

## Context

Crate names on crates.io are globally unique. If we eventually publish,
we need names that aren't already taken and that signal ownership.
Within the workspace, naming conventions also affect ergonomics —
module paths, import statements, and clippy's `module_name_repetitions`
lint all interact with crate names.

## Decision

- **Library crates** use the `caliban-` prefix: `caliban-core`,
  `caliban-provider`, `caliban-agent-core`, etc. Directory name
  matches the package name.
- **Binary crate** is named `caliban` with `[[bin]] name = "caliban"`.
- **Internal module paths** drop the prefix where it would be
  redundant: `caliban_provider::ProviderClient`, NOT
  `caliban_provider::CalibanProviderClient`.
- **Clippy's `module_name_repetitions` lint is allowed** at the
  workspace level to support the internal-naming convention without
  fighting clippy on every type.

## Consequences

- **Positive:** all caliban crates can be reserved on crates.io ahead
  of public release. `cargo install caliban` works once published.
  Internal type names stay terse.
- **Negative:** ~9 extra characters of typing per crate reference in
  `Cargo.toml` dependency lists. Slight redundancy in long import
  paths (`caliban_core::caliban_core_specific::...` — avoided in
  practice by short module names).
- **Revisit if:** the workspace gains so many crates that the prefix
  becomes overhead, or if a sub-org / sub-product emerges that
  warrants its own prefix.
```

- [ ] **Step 6: Create `adrs/0005-workspace-layout.md`**

```markdown
# ADR 0005 · Workspace layout → `crates/` for libraries, binaries at root

- **Status:** accepted
- **Date:** 2026-05-22

## Context

The workspace is planned to grow to ~11 crates across 4 layers
(foundation, integration, routing, UX surfaces). Layout patterns
seen in Rust workspaces:

- **Flat** (`crate1/`, `crate2/` at root) — used by tokio, serde,
  axum. Simpler for small workspaces, clutters root past ~8 crates.
- **All-in-`crates/`** — used by ruff. Binary and libraries
  intermingled; clean root but binary entry points are buried.
- **Apps/libs split** (`crates/` for libs, `apps/` for bins) —
  principled but less common; over-engineered for our size.
- **Binaries at root, libraries in `crates/`** — used by deno
  (with `cli/`), zed, helix. Entry points are top-level visible;
  libraries are clearly cataloged.

## Decision

Adopt the last pattern: library crates under `crates/caliban-<name>/`,
binary crates at the workspace root (`caliban/`, future
`caliban-tui/`, `caliban-orchestrator/`). Workspace members are listed
explicitly in root `Cargo.toml`, no globs.

## Consequences

- **Positive:** root-level `ls` reveals entry points (binaries) and
  config files. `crates/` reveals reusable libraries. Explicit member
  list catches typos and missing members at workspace-parse time.
- **Negative:** new-crate workflow has two patterns rather than one
  (`cargo new --lib crates/<name>` for libraries,
  `cargo new <name>` at root for binaries). Documented in README.
- **Revisit if:** the workspace stays small (<5 crates) and the
  `crates/` directory feels like overhead, or grows past ~25 crates
  where a flat-but-grouped layout (e.g. `crates/layer-1/`,
  `crates/layer-2/`) becomes warranted.
```

- [ ] **Step 7: Verify the ADRs render correctly**

Run: `ls adrs/`
Expected output:

```
0001-async-runtime.md
0002-error-model.md
0003-license-agpl-3.0.md
0004-naming-conventions.md
0005-workspace-layout.md
README.md
```

Open `adrs/README.md` in any markdown previewer (or just read it) and confirm all 5 links in the index point to existing files. Run:

```bash
for f in adrs/000*.md; do test -f "$f" && echo "OK: $f" || echo "MISSING: $f"; done
```

Expected: 5 lines of `OK: ...`.

- [ ] **Step 8: Commit**

```bash
git add adrs/
git commit -m "$(cat <<'EOF'
docs: add five Layer-0 ADRs

Documents the workspace-level decisions made during the Layer-0
brainstorm: async runtime (tokio), error model (thiserror per lib +
anyhow in binary), license (AGPL-3.0-only), naming (caliban-* libs +
caliban binary), workspace layout (crates/ for libs, binaries at root).

All five accepted. MADR-lite format: context / decision / consequences.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Root README

**Files:**
- Create: `README.md`

- [ ] **Step 1: Write `README.md`**

```markdown
# caliban

A from-scratch Rust agent harness — a replacement for Claude Code that puts
the operator in control of model routing, memory, skills, and prompt context.

> **Project status:** Layer 0 (workspace bootstrap). Private repo, designed
> to be open-sourced. The user-facing binary (`caliban --version`) exists
> as a stub; no real agent runtime yet.

## Why

- **Provider-agnostic.** No SDK lock-in. Talk to Anthropic, OpenAI,
  local Ollama, or anything else, swapping providers per task.
- **Operator control.** You decide what model handles what task, what
  context goes into the prompt, and where memory lives.
- **Data sovereignty.** Local-first by default. Designed to integrate
  with self-hosted homelab components.
- **Rust-fast.** The harness overhead should be negligible compared to
  model latency. The user's time-to-result is dominated by the model,
  not the runtime.

## License

caliban is licensed under [AGPL-3.0-only](LICENSE). In short: if you
modify caliban and either distribute the binary or run it as a network
service, you must release your changes under AGPL-3.0. Personal use is
unaffected. Read the [license ADR](adrs/0003-license-agpl-3.0.md) for
the reasoning.

## Building

Requires the toolchain pinned in `rust-toolchain.toml` (currently Rust
1.85.0). `rustup` will install it automatically on first `cargo`
invocation.

```bash
cargo build --workspace             # build everything
cargo test  --workspace             # run all tests
cargo run   --bin caliban -- --version    # smoke-test the binary
```

## Repository layout

```
caliban/             # the user-facing binary
crates/              # libraries
  caliban-core/      # foundational types (Layer 1 seed)
adrs/                # architecture decision records
docs/superpowers/    # design specs and implementation plans
.github/workflows/   # CI
```

## Adding a new crate

**Library:**
```bash
cargo new --lib crates/caliban-<name>
# then add "crates/caliban-<name>" to the workspace.members list in
# the root Cargo.toml
```

**Binary:**
```bash
cargo new caliban-<name>
# then add "caliban-<name>" to the workspace.members list
```

Both inherit the workspace's package metadata, dependencies, and lints
via `*.workspace = true`. See an existing crate's `Cargo.toml` for the
boilerplate.

## Architecture decisions

See [`adrs/`](adrs/). Notable Layer-0 decisions:
- [Async runtime: tokio](adrs/0001-async-runtime.md)
- [Error model: thiserror libs, anyhow binary](adrs/0002-error-model.md)
- [License: AGPL-3.0](adrs/0003-license-agpl-3.0.md)
- [Naming conventions](adrs/0004-naming-conventions.md)
- [Workspace layout](adrs/0005-workspace-layout.md)

## Design specs

The Layer-0 spec lives at
[`docs/superpowers/specs/2026-05-22-layer-0-bootstrap-design.md`](docs/superpowers/specs/2026-05-22-layer-0-bootstrap-design.md).
```

- [ ] **Step 2: Verify all README links resolve**

```bash
grep -oE '\]\([^)]+\)' README.md | sed 's/](\(.*\))/\1/' | while read p; do
  # skip http(s) links
  if [[ "$p" == http* ]]; then continue; fi
  if [ -e "$p" ]; then echo "OK: $p"; else echo "MISSING: $p"; fi
done
```

Expected: every non-HTTP link prints `OK: ...`. All linked ADRs, the LICENSE, and the spec doc should exist on disk by now.

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "$(cat <<'EOF'
docs: add project README

Project pitch, license summary, build instructions, repo layout, the
add-a-crate playbook, and links to the Layer-0 ADRs and design spec.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: CI workflow

**Files:**
- Create: `.github/workflows/ci.yml`

- [ ] **Step 1: Locally simulate the CI pipeline once before writing the workflow**

Run each of these in order from the repo root. **All must exit 0**:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo build --workspace --all-targets
cargo test --workspace --all-features
```

If any fails, fix it before continuing. The workflow file is just an automation of these commands; if they don't work locally, the CI run will fail identically.

- [ ] **Step 2: Create `.github/workflows/ci.yml`**

Run: `mkdir -p .github/workflows`

Then create `.github/workflows/ci.yml`:

```yaml
name: ci

on:
  push:
    branches: ["**"]
  pull_request:
    branches: ["**"]

concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: true

jobs:
  check:
    name: fmt · clippy · build · test
    runs-on: ubuntu-latest
    timeout-minutes: 20

    steps:
      - name: Checkout
        uses: actions/checkout@v4

      - name: Install Rust toolchain (from rust-toolchain.toml)
        uses: dtolnay/rust-toolchain@stable

      - name: Cache cargo registry & target
        uses: Swatinem/rust-cache@v2

      - name: cargo fmt --check
        run: cargo fmt --all -- --check

      - name: cargo clippy
        run: cargo clippy --workspace --all-targets --all-features -- -D warnings

      - name: cargo build
        run: cargo build --workspace --all-targets

      - name: cargo test
        run: cargo test --workspace --all-features
```

Notes:
- `concurrency` cancels in-progress runs when a new commit lands on the same ref. Saves CI minutes during rapid pushes.
- `timeout-minutes: 20` is a generous cap; a clean Layer-0 run should be <5 min.
- `dtolnay/rust-toolchain@stable` reads `rust-toolchain.toml` and installs the pinned version, not the literal `stable` channel.

- [ ] **Step 3: Verify the workflow file is valid YAML**

Run:

```bash
python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml')); print('OK')"
```

Expected: `OK`. (Any Python with PyYAML works; if PyYAML isn't installed, install it or use any other YAML linter — `yamllint`, `ruamel`, etc.)

- [ ] **Step 4: Verify each CI step works locally one more time**

Re-run the four commands from Step 1. If anything regressed since you wrote the workflow file, fix it now.

- [ ] **Step 5: Commit**

```bash
git add .github/
git commit -m "$(cat <<'EOF'
ci: add GitHub Actions workflow

Runs cargo fmt --check, cargo clippy -D warnings, cargo build, and
cargo test --workspace on every push and PR. Single job on
ubuntu-latest with rust-cache enabled. Toolchain pulled from
rust-toolchain.toml. No OS matrix or MSRV check yet — deferred to
later layers.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Acceptance verification

**Files:** none modified. This task is a checklist run.

- [ ] **Step 1: Fresh-clone simulation**

From a clean working tree (`git status` reports nothing to commit), run **in this order**:

```bash
cargo check --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo build --workspace --all-targets
cargo test --workspace --all-features
./target/debug/caliban --version
```

All must exit 0. The final command must print `caliban 0.0.0` to stdout.

- [ ] **Step 2: Verify `unsafe_code = "forbid"` works in caliban-core**

Append to `crates/caliban-core/src/lib.rs`:

```rust
fn _check_forbid() { unsafe { std::ptr::null::<u8>(); } }
```

Run: `cargo build -p caliban-core 2>&1 | head -5`
Expected: error mentioning `unsafe_code` or `usage of an `unsafe` block`.

Remove the function. Re-run `cargo build -p caliban-core` and confirm success.

- [ ] **Step 3: Verify the new-crate handoff path**

The Layer-0 spec promises that adding the provider crate (Layer 1, B) should be a one-shot `cargo new` + workspace edit. Test it without keeping the result:

```bash
cargo new --lib crates/caliban-provider
# Edit root Cargo.toml: add "crates/caliban-provider" to members
cargo build -p caliban-provider
# Expected: builds successfully
```

Then **revert** the addition: `git checkout Cargo.toml && rm -rf crates/caliban-provider`. Re-run `cargo check --workspace` to confirm clean state.

- [ ] **Step 4: Verify ADR file completeness**

```bash
ls adrs/
```

Expected output includes:
- `README.md`
- `0001-async-runtime.md`
- `0002-error-model.md`
- `0003-license-agpl-3.0.md`
- `0004-naming-conventions.md`
- `0005-workspace-layout.md`

For each ADR, confirm it contains the strings `**Status:** accepted` and `**Date:** 2026-05-22`:

```bash
for f in adrs/000*.md; do
  grep -q '\*\*Status:\*\* accepted' "$f" || echo "MISSING status in: $f"
  grep -q '\*\*Date:\*\* 2026-05-22' "$f"  || echo "MISSING date in: $f"
done
```

Expected: no output (no failures).

- [ ] **Step 5: Push to GitHub and verify the CI is green**

When the GitHub repo exists, push `main` and confirm:
- The Actions tab shows a green run.
- All four steps (fmt, clippy, build, test) pass.

If the GitHub remote doesn't exist yet, this step is **deferred** — note it in a follow-up issue and move on.

- [ ] **Step 6: Final smoke test of the negative case**

Deliberately break formatting in a scratch branch to verify CI fails as expected:

```bash
git checkout -b ci-negative-test
# Introduce a formatting violation, e.g., extra blank line in caliban/src/main.rs
echo "" >> caliban/src/main.rs
echo "" >> caliban/src/main.rs
git add caliban/src/main.rs
git commit -m "test: deliberately break formatting"
git push origin ci-negative-test
```

Watch the Actions run. Expected: red, failing at the `cargo fmt --check` step.

Then clean up:

```bash
git checkout main
git branch -D ci-negative-test
git push origin --delete ci-negative-test
```

Like Step 5, this is **deferred** until a GitHub remote exists.

- [ ] **Step 7: Done — Layer 0 is complete**

If every step above passed (or is correctly deferred to "push to GitHub when remote exists"), Layer 0 is complete. The handoff signal to Layer 1 / B (provider abstraction) is: `cargo new --lib crates/caliban-provider` + add to workspace members + start the B spec/plan cycle.

---

## Self-Review

**Spec coverage check:**
- Goals (workspace skeleton, ADRs, CI) → Tasks 1–6 ✓
- Repository structure → Tasks 1, 2, 3, 4, 5, 6 cover every path ✓
- Workspace `Cargo.toml` config → Task 1 ✓
- All 5 ADRs → Task 4 ✓
- CI pipeline → Task 6 ✓
- Acceptance criteria (build/lint/test/CI/repo hygiene/ADRs/handoff path) → Task 7 ✓
- Risks (forbid unsafe, AGPL constraints, strict clippy, toolchain pin) — addressed by the lint-override pattern in Task 2 step 7 and the negative-test in Task 7 step 6 ✓

**Placeholder scan:**
- No "TBD", "TODO" in steps. The `# repository = TODO` in Task 1's `Cargo.toml` snippet is an intentional, marked-as-such placeholder for the GitHub repo URL — the spec acknowledges this.
- All step code is concrete and complete.
- All test code is shown in full.

**Type/name consistency:**
- `caliban-core` (kebab in dirs/packages, `caliban_core` snake in module paths) — consistent throughout.
- `caliban` binary's `[[bin]] name = "caliban"` matches the integration test's `CARGO_BIN_EXE_caliban`. ✓
- ADR file names match the README index. ✓
- `--version` / `-V` flag handling consistent between Task 3 main.rs and Task 3 integration test. ✓
