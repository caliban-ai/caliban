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
