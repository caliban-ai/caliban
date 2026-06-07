# Releasing caliban to crates.io

caliban publishes the binary plus its internal library crates to crates.io from
the **`caliban-ai/caliban`** repository only. Publishing is guarded three ways
(see `.github/workflows/publish.yml`): a repo `if`, a `CARGO_REGISTRY_TOKEN`
secret that exists only for this repo, and a tag↔version check. The actual
upload runs through **`scripts/publish.sh`**, which is resumable and
rate-limit-aware (see below).

## The crates.io new-crate rate limit

crates.io throttles the creation of **brand-new crate names** much harder than
new *versions* of existing crates: a burst of **5 new crates**, then **~1 new
crate per 10 minutes** (https://crates.io/docs/rate-limits). The first release
of a multi-crate workspace therefore cannot go out in one shot — a plain
`cargo publish --workspace` uploads ~5 crates and then fails with HTTP 429.

This only bites on the **first** publish of each crate name. Once all crates
exist, future releases publish new *versions*, which are not meaningfully
limited.

To request a higher limit (so a future batch of new crates can go out at once),
email **help@crates.io** with the account and crate list; they routinely grant
it for legitimate multi-crate projects.

## `scripts/publish.sh`

The publisher handles the limit and partial failures:

- publishes only crates **not yet on crates.io** at the workspace version, so it
  is **idempotent and resumable** — safe to Ctrl-C and re-run;
- publishes in **dependency order**, one crate at a time;
- uses **`--no-verify`** (CI's `package-check` already verified packaging on the
  release commit, so there is no recompile at publish time);
- on a 429, parses crates.io's "try again after" time and sleeps until then;
- honors **`MAX_SLEEP_SECS`**: locally it sleeps through the windows for free; in
  CI it is set to `0` so the runner never idles (and bills) on a 429.

## One-time setup

1. Create a crates.io API token (scopes: `publish-new` + `publish-update`) and
   add it as the **organization** secret `CARGO_REGISTRY_TOKEN` at
   https://github.com/organizations/caliban-ai/settings/secrets/actions, scoped
   to **selected repositories** (just `caliban-ai/caliban` and any future
   publishers) — never "all repositories." A repo-level secret of the same name
   also works and takes precedence; pick one home.
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

## First publish (many new crates) — run locally

Because of the rate limit, the very first publish is best run from your machine,
where the ~10-minute waits cost nothing (a GitHub runner would bill the idle
time). Authenticate, then run the resumable publisher:

```sh
cargo login            # paste a publish-new token; stored in ~/.cargo/credentials.toml
scripts/publish.sh     # ~25 crates → ~2–3 h, paced; resumable if interrupted
```

It skips anything already live and grinds through the rest. Re-run it any time
to resume. **Rotate the token afterward** if it was ever exposed (e.g. pasted
where it could be logged).

## Subsequent releases (version bumps)

These publish new *versions* of existing crates and are not rate-limited, so the
workflow handles them automatically:

1. Bump `version` in the root `Cargo.toml` `[workspace.package]` to `X.Y.Z`.
2. Commit and push to `origin` (`caliban-ai/caliban`).
3. Tag and push:

   ```sh
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```

4. The `publish` workflow validates the guards and runs `scripts/publish.sh`
   (with `MAX_SLEEP_SECS=0`), publishing each crate in dependency order with no
   recompile.

If a release ever introduces **new** crate names and there are more than ~5 of
them, the workflow will publish the burst and stop (it won't idle-bill on the
429) — finish the rest locally with `scripts/publish.sh`.

## If a publish fails partway

crates.io releases are immutable, so already-published crates cannot be
re-uploaded at the same version. Recovery is simply to **re-run
`scripts/publish.sh`** — it skips everything already live and continues with the
rest. (If you must do it by hand: `cargo publish -p <crate> --no-verify` for each
remaining crate, in dependency order.)
