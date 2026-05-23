# Layer 0 · Workspace & ADRs · Design

- **Date:** 2026-05-22
- **Status:** Draft (pending implementation plan)
- **Sub-project of:** caliban Rust agent harness
- **Next sub-project:** Layer 1 / B — provider abstraction

## Goals

Establish the workspace skeleton and architectural decisions that every subsequent crate will inherit. Layer 0 ends with a workspace that compiles, lints, formats, and tests cleanly in CI, plus five accepted ADRs documenting the cross-cutting decisions. The handoff to Layer 1 (B, provider abstraction) is: drop a new crate into `crates/`, add it to the workspace members list, write code.

## Non-goals

- No real provider, agent loop, tools, MCP client, memory, router, or UX code. Those are Layer 1+.
- No message-schema ADR — deferred to B, where the concrete provider trait will drive the design.
- No release infrastructure (`cargo-release`, changelog automation, version policy). Added when we publish or tag.
- No pre-commit hooks. Per-developer choice. CI is the source of truth.
- No `cargo doc` deployment or docs hosting.
- No coverage reporting, security scanning, or matrix builds in CI. Added when there's a concrete reason.

## Repository structure

```
caliban/
├── .github/workflows/ci.yml         # fmt · clippy · build · test
├── .gitignore                       # target/, .superpowers/, *.swp, .DS_Store
├── adrs/
│   ├── README.md                    # ADR index + status legend
│   ├── 0001-async-runtime.md        # tokio
│   ├── 0002-error-model.md          # thiserror per lib, anyhow in binary
│   ├── 0003-license-agpl-3.0.md     # AGPL-3.0-only
│   ├── 0004-naming-conventions.md   # caliban-* libs, caliban binary
│   └── 0005-workspace-layout.md     # crates/ for libs, binaries at root
├── crates/
│   └── caliban-core/                # placeholder library, trait crate seed
│       ├── Cargo.toml
│       └── src/lib.rs               # forbid(unsafe_code) + smoke test
├── caliban/                         # user-facing binary
│   ├── Cargo.toml                   # [[bin]] name = "caliban"
│   └── src/main.rs                  # --version, exit 0
├── docs/
│   └── superpowers/specs/
│       └── 2026-05-22-layer-0-bootstrap-design.md   # this file
├── Cargo.lock                       # committed
├── Cargo.toml                       # [workspace] root + workspace.dependencies + workspace.lints
├── LICENSE                          # AGPL-3.0 text
├── README.md                        # description, build, links to ADRs and spec
├── rust-toolchain.toml              # pinned stable, e.g., channel = "1.85.0"
└── rustfmt.toml                     # edition = "2024"
```

### Rationale for each path choice

- **`crates/caliban-core/`** — workspace requires ≥1 member to compile; `caliban-core` will become the trait-crate seed where shared types and the `Provider` trait eventually land (B). Layer 0 ships it empty except for a smoke test.
- **`caliban/` at root, not under `crates/`** — binaries live at the workspace root; libraries under `crates/`. Future binaries (`caliban-tui`, `caliban-orchestrator`) join `caliban/` at root.
- **`adrs/` at root, not `docs/adrs/`** — ADRs are first-class Layer 0 deliverables; top-level placement makes them impossible to miss.
- **`docs/superpowers/specs/`** — follows the superpowers brainstorming convention. Specs design what we'll build; ADRs record what we decided. Different artifact, different home.
- **No `tests/` at root** — integration tests live alongside their crate (`crates/<name>/tests/`). Workspace-level integration tests deferred until cross-crate behavior exists.
- **No `examples/`** — no library APIs to demo yet.

## Workspace `Cargo.toml`

```toml
[workspace]
resolver = "2"
members = [
    "crates/caliban-core",
    "caliban",
]

[workspace.package]
edition = "2024"
license = "AGPL-3.0-only"
authors = ["John Ford <john.ford2002@gmail.com>"]
rust-version = "1.85"
# repository = TODO: set once the GitHub repo exists

[workspace.dependencies]
tokio    = { version = "1", features = ["full"] }
thiserror = "1"
anyhow   = "1"

[workspace.lints.rust]
unsafe_code        = "forbid"
missing_docs       = "warn"
unreachable_pub    = "warn"
rust_2018_idioms   = "warn"

[workspace.lints.clippy]
all      = { level = "deny", priority = -1 }
pedantic = { level = "deny", priority = -1 }
cargo    = "warn"
module_name_repetitions = "allow"
must_use_candidate      = "allow"
```

Each member crate sets `[lints] workspace = true` in its own `Cargo.toml` to inherit.

## ADRs (full text shipped under `adrs/`)

All five ADRs use a MADR-lite format: **Status / Context / Decision / Consequences**. Status is `accepted` at Layer 0 ship time.

### 0001 · Async runtime → `tokio`

caliban's foundation is heavily I/O-bound. Rust's async story is fragmented across runtimes; mixing produces incompatible futures. Standardize on `tokio` (multi-threaded scheduler) across every crate. Workspace pins the dep; members declare `tokio.workspace = true` with their feature subset. Locks us out of `smol`/`embassy` (acceptable — no embedded targets). Direct compatibility with `reqwest`, `tower`, `hyper`, `axum`, `tonic`, major MCP transports.

### 0002 · Error model → `thiserror` for libs, `anyhow` for the binary

Libraries benefit from precise error enums (consumers match on variants); binaries benefit from ergonomic context propagation. Every `caliban-*` library defines its own `Error` enum via `thiserror` and exposes `pub type Result<T> = std::result::Result<T, Error>;`. Cross-crate errors convert at boundaries with `#[from]` or explicit `From` impls. The `caliban` binary uses `anyhow` for `main()` and top-level command handlers. No shared "uber error" crate.

### 0003 · License → `AGPL-3.0-only`

Project is private now but designed to be open-sourced. Author rejects permissive defaults; goal is enforced community contribution from downstream users and SaaS operators. Every `Cargo.toml` declares `license = "AGPL-3.0-only"`. `LICENSE` at root contains the full text. README states implications. caliban crates won't compose into permissive Rust projects on crates.io — this is intentional.

### 0004 · Naming → `caliban-*` libraries, `caliban` binary

Crate names on crates.io are global; publishability needs to be possible without rename. Library crates use the `caliban-` prefix (`caliban-core`, `caliban-provider`, …). The binary's package is `caliban` with `[[bin]] name = "caliban"`. Directory names mirror package names. Internal module paths drop the prefix (`caliban_provider::ProviderClient`, not `CalibanProvider`). The clippy `module_name_repetitions` lint is allowed at the workspace level to support this convention.

### 0005 · Workspace layout → `crates/` for libs, binaries at root

Workspace will grow to ~11 crates across 4 layers. A flat layout clutters the root; a fully `crates/`-d layout hides entry points among utilities. Library crates live in `crates/caliban-<name>/`. Binary crates live at the workspace root. Workspace `Cargo.toml` lists members explicitly (no globs). New library: `cargo new --lib crates/caliban-<name>` + add to members. New binary: `cargo new caliban-<name>` at root + add to members.

## CI pipeline

Single workflow `.github/workflows/ci.yml`, one job (`check`), `ubuntu-latest`, toolchain from `rust-toolchain.toml`. Triggers: every push to any branch, every PR.

Steps, in order, each failing fast:

1. `actions/checkout@v4`
2. `dtolnay/rust-toolchain@stable` (reads `rust-toolchain.toml`)
3. `Swatinem/rust-cache@v2`
4. `cargo fmt --all -- --check`
5. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
6. `cargo build --workspace --all-targets`
7. `cargo test --workspace --all-features`

Deferred to later layers: OS matrix, MSRV check, beta/nightly tracking, coverage, security scanning, release artifacts, publish steps.

## Acceptance criteria

Layer 0 is done when all of the following hold on a fresh clone of `main`:

**Build & runtime**
- `cargo check --workspace` exits 0.
- `cargo build --workspace --all-targets` succeeds; produces `target/debug/caliban`.
- `./target/debug/caliban --version` prints `caliban <version>` (parsed from `Cargo.toml`) and exits 0.

**Linting & formatting**
- `cargo fmt --all -- --check` exits 0.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` exits 0.
- Workspace `Cargo.toml` contains the `[workspace.lints]` table above.
- Attempting `unsafe {}` anywhere in the workspace fails to compile (confirms `unsafe_code = "forbid"` is active).

**Tests**
- `cargo test --workspace` exits 0.
- At least one smoke test exists in `caliban-core` purely to prove the test harness is wired up.

**CI**
- First push to `main` produces a green Actions run.
- Deliberately breaking formatting in a branch produces a red Actions run failing at the `cargo fmt` step.

**Repo hygiene**
- `LICENSE` at root contains the full AGPL-3.0 text.
- `README.md` at root contains: project description, license statement, build instructions, link to this spec doc, link to the ADRs.
- `.gitignore` contains at minimum `target/`, `.superpowers/`, `*.swp`, `.DS_Store`.
- `rust-toolchain.toml` pins a specific stable Rust version.
- `rustfmt.toml` exists with `edition = "2024"`.

**ADRs**
- All five ADRs exist under `adrs/`, status `accepted`, dated 2026-05-22.
- `adrs/README.md` indexes them with one-line summaries and the status legend (`accepted` / `superseded` / `proposed` / `rejected`).

**Handoff path to Layer 1 / B**
- A developer wanting to start the provider crate runs `cargo new --lib crates/caliban-provider`, adds `"crates/caliban-provider"` to workspace members, and `cargo build` succeeds. This path is documented in `README.md`.

## Open questions

None for Layer 0. The message-schema decision (deferred to B) is explicitly out of scope.

## Risks

- **`unsafe_code = "forbid"` may block a future need.** Mitigation: any crate that needs `unsafe` overrides the lint locally with `#![allow(unsafe_code)]` and an inline justification comment. The forbid keeps unsafe rare and audited rather than absent.
- **AGPL-3.0 may limit dependency choices.** Mitigation: AGPL is compatible with most permissive licenses for consumption (MIT/Apache-2.0 deps are fine inside AGPL software). The constraint runs in the other direction — permissive consumers can't embed us.
- **Strict clippy may slow development.** Mitigation: the two noisiest pedantic lints are pre-allowed; individual lints can be `#[allow]`-ed locally with justification.
- **Toolchain pin may drift from `latest stable`.** Mitigation: bump explicitly during routine maintenance; CI catches breakage immediately.
