# MCP HTTP transport + `mcp.toml` config-path consistency

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make HTTP / SSE / OAuth MCP servers configured in `mcp.toml` actually load in production, and make every user-facing reference to the user-scope `mcp.toml` path agree with the discovery rules on each OS.

**Spec / ADR:** [`docs/superpowers/specs/2026-05-24-mcp-v2-design.md`](../specs/2026-05-24-mcp-v2-design.md), [`docs/adr/0023-mcp-v2-transports-and-oauth.md`](../../../docs/adr/0023-mcp-v2-transports-and-oauth.md).

---

## Current-state diagnosis (three real bugs)

1. **`mcp.toml` is never loaded in production.** `caliban/src/startup.rs::start_mcp` only calls `settings_snapshot.mcp_config()`. The legacy loader `caliban_mcp_client::load_config` is bridged through `caliban_settings::compat::maybe_load_legacy_mcp`, but `rg 'maybe_load_legacy_mcp'` confirms zero production call sites — the shim was only invoked from tests.
2. **`Settings::mcp_config()` hard-codes stdio.** `crates/caliban-settings/src/settings.rs` `Settings::mcp_config()` constructs `caliban_mcp_client::ServerConfig` with `transport: TransportKind::Stdio`, `url: None`, `headers: empty`, `oauth: OauthMode::Off`. HTTP/SSE/OAuth/permissions config from `mcp.toml` is silently dropped during the round-trip even if the compat shim *did* run.
3. **`McpServerSetting` schema is stdio-only.** `crates/caliban-settings/src/settings.rs` `McpServerSetting` carries `command/args/env/cwd/disabled` only. So `settings.json`'s `mcpServers` block can't express HTTP either, and the compat shim has no fields to copy HTTP info into.

Plus: the `/mcp` overlay help text in `caliban/src/tui/overlay.rs::mcp_lines` hardcodes `~/.config/caliban/mcp.toml` — misleading on macOS, where `dirs::config_dir()` resolves to `~/Library/Application Support`.

---

## Design decisions

### Field naming

We accept **both** `type` and `transport` as the spelling of the transport selector for `McpServerSetting` (via `serde(alias = "transport")` on a Rust field literally named `r#type`). Rationale: `~/.claude.json` and Claude Desktop spell it `type`; the existing TOML schema in `caliban-mcp-client::config::RawServerConfig` spells it `transport`. Accepting both lets settings.json copy-paste between caliban and Claude Desktop without surprise.

### User-scope `mcp.toml` discovery order

For the legacy loader `caliban_mcp_client::load_config`:

1. If `$XDG_CONFIG_HOME/caliban/mcp.toml` exists, use it (the "XDG path"). Done.
2. Else, if `dirs::config_dir().join("caliban/mcp.toml")` exists, use it (the "platform-native path"). On Linux this is the same as the XDG path; on macOS it's `~/Library/Application Support/caliban/mcp.toml`; on Windows it's `%APPDATA%\caliban\mcp.toml`.
3. Project entries still override user entries by name, unchanged.

**Decision:** "first-found wins" at the user tier — we do not merge XDG into Application Support. This avoids surprising silent merges when a user has stale data in both locations. The chosen path is logged at debug level. Document the rule on `discovery_paths`.

### Hooks / permissions compat shim

While we're inside `load_settings`, we also call `maybe_load_legacy_permissions` and `maybe_load_legacy_hooks` so legacy `.toml` files for those features behave the same as legacy `mcp.toml`. This matches the documented "one-release compat window" already implied by the shim's doc-comment.

---

## File-by-file change list

```
docs/superpowers/plans/2026-05-26-mcp-http-and-config-path.md   CREATE (this doc)

crates/caliban-settings/src/settings.rs                          MODIFY
  - McpServerSetting: add r#type, url, headers, oauth, permissions
  - Settings::mcp_config(): map r#type+url+headers+oauth+permissions
    onto caliban_mcp_client::ServerConfig

crates/caliban-settings/src/compat.rs                            MODIFY
  - maybe_load_legacy_mcp: round-trip transport/url/headers/oauth/permissions
  - Add HTTP round-trip test

crates/caliban-settings/src/loader.rs                            MODIFY
  - After scope merge, call maybe_load_legacy_{mcp,permissions,hooks}
  - Add an integration-flavored test

crates/caliban-mcp-client/src/config.rs                          MODIFY
  - discovery_paths returns (Vec<PathBuf>, PathBuf), de-duped, XDG-first
  - load_config: iterate user-scope candidates, first-found wins
  - New unit tests for ordering / dedupe / XDG override

caliban/src/tui/overlay.rs                                       MODIFY
  - mcp_lines: render resolved user-config path(s); add HTTP example

caliban/src/tui/slash/config.rs                                  MODIFY (one-line)
  - Replace hard-coded ~/.config/caliban/ in hooks message
```

---

## Test plan

- `crates/caliban-mcp-client/src/config.rs` (unit):
  - `discovery_paths_linux_dedupes_when_xdg_equals_native`
  - `discovery_paths_returns_xdg_and_native_when_distinct`
  - `discovery_paths_xdg_override_honored`
- `crates/caliban-settings/src/compat.rs` (unit):
  - `legacy_mcp_http_round_trip_preserves_url_headers_oauth_permissions`
- `crates/caliban-settings/src/loader.rs` (unit):
  - `load_settings_invokes_compat_shim_when_unified_absent`
- `crates/caliban-settings/tests/integration.rs`:
  - `http_mcp_loaded_end_to_end_via_load_settings_and_mcp_config`

Workspace-wide:

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

All three must pass before commit.

---

## Manual verification

1. `cargo build --bin caliban` — clean build, no new warnings.
2. Ensure `~/Library/Application Support/caliban/mcp.toml` (on macOS) or `~/.config/caliban/mcp.toml` (Linux) contains an HTTP entry:
   ```toml
   [server.silverbullet]
   type = "http"
   url  = "https://mcp.silverbullet.hexadecimate.net/mcp"
   ```
3. Launch `cargo run --bin caliban`.
4. In the TUI, type `/mcp`. Expect: a row reading `● silverbullet  http   connected — N tools`.
5. `/help`-style verification: the empty-state of `/mcp` now mentions *both* the XDG and platform-native paths (or just the unified one on Linux).

---

## Out of scope

- Schema changes to `settings.json`'s top-level `mcpServers` JSONSchema (Phase D).
- Migrating ADR 0017 / older spec docs to the new dual-path discovery (those are historical records).
- Any UI for editing the chosen user-scope path inside caliban.
