# MCP client v1 (scaffold) — Implementation Plan

> Executed inline. This PR lands the **config scaffold + manager API surface**
> only. The actual `rmcp` integration (spawn child, JSON-RPC handshake, tool
> wrapping) lands in a follow-up PR — pulling in `rmcp` 1.7 and the rest of
> the dep chain is its own integration job and would dwarf this PR.

**Goal (this PR):** A `caliban-mcp-client` crate that parses `mcp.toml`,
validates server names + env, and exposes an `McpClientManager::start` that's
ready to wire actual spawn/tool registration once the rmcp dependency lands.

**Architecture (delivered):**
- `crates/caliban-mcp-client/` workspace member.
- `config.rs` — `ServerConfig`, `McpConfig`, `discovery_paths`, `load_config` (user file → project file merge, project wholesale-overrides user), `is_valid_server_name` (`^[a-z0-9_-]{1,32}$`).
- `error.rs` — `McpError` (`Io`, `ConfigParse`, `InvalidServerName`, `MissingEnv`, `InlineInterpolation`, `Spawn`).
- `manager.rs` — `McpClientManager` with `start(cfg) -> Result<Self>`, `register_into(&mut ToolRegistry)`, `enabled_count()`, `skipped_disabled()`, `shutdown()`. v1 is a no-op for spawn + register; logs a warn that explains "wiring lands later" when enabled servers are configured.

**Wiring delivered:**
- Binary loads config + builds manager during startup; failures are warn-and-skip (never abort startup).
- `--no-mcp` (env `CALIBAN_NO_MCP`) skips both config load and manager.

**Tests (5 unit):**
- parses_minimal_server, parses_full_server, valid_name_rule, project_overrides_user_wholesale, disabled_field_round_trip.

**Spec:** `docs/superpowers/specs/2026-05-23-mcp-client-design.md`
**ADR:** `docs/adr/0017-mcp-client-architecture.md`

**Deferred to follow-up PR (call it v1.1):**
- `rmcp` 1.7 dependency.
- Child-process spawn + `initialize` handshake with 5s timeout.
- `list_tools` discovery + per-server `McpTool` registration.
- `McpTool::invoke` with cancellation race.
- Content translation (text / image / resource / audio).
- Integration tests via in-tree `rmcp`-based test server binary.
- `/mcp` overlay update with live server status.
