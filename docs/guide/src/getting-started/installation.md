# Installation & Building

Caliban is distributed as source. You build it with Cargo and install the resulting binary yourself. There are no pre-built releases yet.

## Requirements

| Requirement | Details |
|---|---|
| Rust toolchain | `1.95.0`, pinned in `rust-toolchain.toml` |
| rustup | Installs the pinned toolchain automatically on first `cargo` invocation |
| Git | To clone the repository |

`rustup` detects `rust-toolchain.toml` and downloads the exact channel automatically — no manual `rustup install` step required.

## Clone

```bash
git clone https://github.com/caliban-ai/caliban.git
cd caliban
```

## Build

### Release binary

```bash
cargo build --release --bin caliban
```

The binary lands at `target/release/caliban`. Build time on a modern machine is a few minutes on a cold cache.

### Development build

```bash
cargo build --workspace      # all crates, debug symbols
cargo test  --workspace      # full test suite
```

## Put the binary on your PATH

```bash
# Option A — copy to a directory already on your PATH
cp target/release/caliban ~/.local/bin/caliban

# Option B — add target/release to PATH (in your shell profile)
export PATH="$PWD/target/release:$PATH"
```

## Smoke test

```bash
caliban --version
```

You should see a version string. If you get a "command not found" error, confirm `target/release/` is on your `PATH`.

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

Cloud transport features are not built in default CI runs. They are exercised by a weekly cron job and by manual dispatch of the `ci-cloud` workflow.

## Helper scripts

The `scripts/` directory contains one helper:

| Script | Purpose |
|---|---|
| `scripts/check.sh` | Mirrors the full PR CI suite locally: `cargo fmt --check`, `cargo clippy`, `cargo build`, `cargo test`. Accepts `--cloud` to additionally run the cloud-features build, and `--no-test` to skip the test step. |

Run `scripts/check.sh --help` for the full usage summary.

```admonish tip title="Headless / CI builds"
On headless Linux hosts, the default binary features include `clipboard` (the `arboard` crate). If your CI image lacks the X11/Wayland clipboard libraries, build with `--no-default-features` to avoid the link-time dependency.
```
