# Installation & Building

Caliban is published to [crates.io](https://crates.io/crates/caliban), so the
quickest way to get the binary is `cargo install caliban`. Building from a git
checkout stays fully supported and is the path to use when you want to hack on
caliban itself, or need a build with non-default feature flags.

There are no pre-built **binary** downloads yet — the GitHub releases carry no
attached artifacts, so every install compiles from source, whether Cargo fetches
that source from crates.io or you clone it yourself. A published
[container image](https://github.com/caliban-ai/caliban/blob/main/docs/container.md)
(`ghcr.io/caliban-ai/caliban`) is the third option if you would rather not build
at all.

## Requirements

| Requirement | Details |
|---|---|
| Rust toolchain | `1.95` or newer (the crate's `rust-version`) |
| rustup | Recommended; installs and manages the toolchain |
| Git | Only needed for the from-source path |

For a git checkout, the exact channel is pinned in `rust-toolchain.toml`
(currently `1.95.0`) and `rustup` downloads it automatically on first `cargo`
invocation — no manual `rustup install` step required. Installing from crates.io
does **not** pick up that pin (the published crate does not ship
`rust-toolchain.toml`), so your default toolchain must already be `1.95` or
newer.

## Install from crates.io

```bash
cargo install caliban --locked
```

Cargo builds the binary and drops it in `~/.cargo/bin/caliban`, which `rustup`
already puts on your `PATH`. The build takes a few minutes on a cold cache.

`--locked` is recommended: the published crate ships its `Cargo.lock`, so this
builds against exactly the dependency versions the release was tested with.
Drop it if you deliberately want newer semver-compatible dependencies.

To upgrade later, re-run the same command — caliban has no built-in
self-update command.

```admonish note title="Installing `caliband` too"
`cargo install caliban` installs only the `caliban` binary. Background-fleet
features additionally need the `caliband` daemon, which ships in a sibling
crate:

    cargo install caliban-supervisor --bin caliband --locked

See [The Background Fleet](../subagents/background-fleet.md) for what it does.
```

## Build from source

Use this path for development, for building a specific commit, or for enabling
the optional cloud transports below.

### Clone

```bash
git clone https://github.com/caliban-ai/caliban.git
cd caliban
```

### Release binary

```bash
cargo build --release --bin caliban
```

The binary lands at `target/release/caliban`. Build time on a modern machine is
a few minutes on a cold cache.

### Development build

```bash
cargo build --workspace      # all crates, debug symbols
cargo test  --workspace      # full test suite
```

### Put the binary on your PATH

A source build does not install anything — unlike `cargo install`, you place the
binary yourself:

```bash
# Option A — copy to a directory already on your PATH
cp target/release/caliban ~/.local/bin/caliban

# Option B — add target/release to PATH (in your shell profile)
export PATH="$PWD/target/release:$PATH"

# Option C — let cargo install it from the checkout
cargo install --path caliban --locked
```

## Smoke test

```bash
caliban --version
```

You should see a version string. If you get a "command not found" error, confirm
the install directory (`~/.cargo/bin` for `cargo install`, or `target/release/`
for a source build) is on your `PATH`.

For a build made from a git checkout, the version also carries the commit it
was built from, so you can pin exactly which point in history a binary
corresponds to:

```text
caliban 0.7.0 (a1b2c3d, 2026-07-15)
```

The parentheses hold the short commit SHA and that commit's date; a build with
uncommitted changes appends `-dirty` (e.g. `a1b2c3d-dirty`). Builds made
without git metadata (release tarballs, `cargo install` from crates.io) report
just the bare semver — `caliban 0.7.0`.

## Optional: cloud transport feature flags

By default, caliban connects to providers over their public HTTPS APIs. Cloud-managed transports (AWS Bedrock, Google Vertex AI, Azure OpenAI) require optional Cargo feature flags. The exact flag names per crate are:

| Transport | Feature flag |
|---|---|
| Anthropic via AWS Bedrock | `caliban-provider-anthropic/bedrock` |
| Anthropic via Google Vertex AI | `caliban-provider-anthropic/vertex` |
| OpenAI via Azure | `caliban-provider-openai/azure` |
| Gemini via Google Vertex AI | `caliban-provider-google/vertex` |

To build a binary with multiple cloud transports enabled at once:

```bash
cargo build --release --bin caliban \
  --features caliban-provider-anthropic/bedrock,caliban-provider-anthropic/vertex,\
caliban-provider-openai/azure,caliban-provider-google/vertex
```

The same `--features` list works with `cargo install caliban`, so you do not
need a checkout just to enable a cloud transport.

Cloud transport features are not built in default CI runs. They are exercised by a weekly cron job and by manual dispatch of the `ci-cloud` workflow.

## Helper scripts

The `scripts/` directory contains these helpers:

| Script | Purpose |
|---|---|
| `scripts/check.sh` | Mirrors the full PR CI suite locally: `cargo fmt --check`, `cargo clippy`, `cargo build`, `cargo test`. Accepts `--cloud` to additionally run the cloud-features build, and `--no-test` to skip the test step. |
| `scripts/coverage.sh` | Measures workspace line coverage with `cargo-llvm-cov` and fails below the `COVERAGE_MIN` floor — the same gate CI enforces. Accepts `--html`/`--open` to render an HTML report and `--no-fail` to report without gating. Writes `lcov.info` + `coverage.json` under `target/llvm-cov/`. |
| `scripts/coverage-report.sh` | Renders `target/llvm-cov/coverage.json` into the Markdown coverage report CI posts as a sticky PR comment (overall stats, per-crate breakdown, notable gaps). Run after `coverage.sh` to preview it locally. |

Run `scripts/check.sh --help` or `scripts/coverage.sh --help` for the full usage summary.

```admonish tip title="Headless / CI builds"
On headless Linux hosts, the default binary features include `clipboard` (the `arboard` crate). If your CI image lacks the X11/Wayland clipboard libraries, build with `--no-default-features` to avoid the link-time dependency — the flag works on both `cargo install caliban` and `cargo build`.
```
