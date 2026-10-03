# ADR 0061 · Shared `caliban-config-types` leaf crate inverts settings' upward dependency

- **Status:** accepted
- **Date:** 2026-10-03
- **Builds on:** [0026](0026-settings-layering.md) (the unified settings layer)
- **Author:** john.ford2002@gmail.com

## Context

`caliban-settings` is the configuration substrate — it loads and merges the
settings hierarchy ([ADR 0026](0026-settings-layering.md)) and hands typed
config to the rest of the workspace. But it depended **upward** on two of its own
consumers, `caliban-agent-core` and `caliban-mcp-client`, so it could name the
types it stores and projects:

- `Settings` **embeds** consumer types directly in its data model —
  `caliban_mcp_client::{ManualOauthConfig, ServerPermissions}` as struct fields,
  `caliban_agent_core::PermissionMode` as a field.
- It **projects** consumer types out of methods — `mcp_config() ->
  caliban_mcp_client::McpConfig`, `permission_rules() -> Vec<caliban_agent_core::Rule>`,
  `hook_config() -> caliban_agent_core::HooksConfig`, and several
  `apply_*(&mut caliban_agent_core::AgentConfig)` methods.
- Its `compat.rs`, `import.rs`, and `provenance.rs` modules **consume** those
  crates' loaders and matchers.

The module doc was candid that the shape existed "to avoid a cyclic dep." That
workaround inverts the layering: the configuration substrate knows the shape of
permissions, hooks, and MCP, so every consumer change reaches back into
settings. This is the structural reason [#498](https://github.com/caliban-ai/caliban/issues/498)
("rules invisible to live-reload") has no clean fix — the reload path and the
rule projection sit on opposite sides of a dependency pointing the wrong way.

Epic [#539](https://github.com/caliban-ai/caliban/issues/539) (part of the
architecture epic [#546](https://github.com/caliban-ai/caliban/issues/546))
weighed two cycle-free options:

- **Option 1 — consumer-side extension traits.** Each consumer defines its own
  `TryFrom<&Settings>` for its projection; settings returns raw config. Smaller
  diff, no new crate.
- **Option 2 — a shared leaf config-types crate.** A leaf crate holds the
  projection types; both settings and consumers depend on it. Better when the
  projections are genuinely shared by more than one consumer.

A survey of the actual code settled the choice. The consumer types are not merely
*projected* by a handful of methods — they are **embedded in the Settings data
model** and consumed by whole modules (`compat`, `import`, `provenance`). That
is, `caliban-settings` is itself a consumer of these types, so they are shared
between settings and their owning crate — exactly the condition Option 2 names.

## Decision

Adopt **Option 2**. Introduce a leaf crate, **`caliban-config-types`**, that
holds the genuinely-shared configuration **data types** and depends on no
consumer crate. Both `caliban-settings` and its consumers depend on it
*downward*.

- The shared **data types** move to `caliban-config-types`: the MCP config types
  (`McpConfig`, `ServerConfig`, `ServerPermissions`, `ManualOauthConfig`,
  `OauthMode`, `TransportKind`) and, next, the permission/hook types (`Rule`,
  `Action`, `PermissionMode`, `HooksConfig`) with their pure, dependency-free
  helpers.
- The owning consumer crates **re-export** the moved types, so their downstream
  consumers are unaffected (behavior-preserving).
- Runtime types that are *not* leaf data stay in their crate. In particular
  `caliban_agent_core::AgentConfig` remains in `caliban-agent-core`; the
  `apply_*(&mut AgentConfig)` projections move there (as functions reading
  `&Settings`), a small hybrid with the extension-trait idea of Option 1. The
  edge is fully inverted either way, because `caliban-settings` keeps no
  dependency on `caliban-agent-core`.

The work lands as four sub-PRs in dependency order: stand up the crate + MCP
types (#708, this ADR), move permission/hook types (#709), repoint settings'
MCP references and drop the `caliban-mcp-client` edge (#710), and move the
`AgentConfig` projections and drop the `caliban-agent-core` edge (#711).

## Consequences

- **The layering points the right way.** `caliban-settings` depends only on
  leaf crates (`caliban-common`, `caliban-config-types`). Consumers depend on
  settings and on config-types, never the reverse. The cycle the old shape
  dodged is gone by construction, not by workaround.
- **#498 becomes fixable.** Live reload and rule projection no longer sit across
  an inverted edge, so the reload path can own rule re-projection cleanly.
- **One more published crate** (25 → 26 internal library crates). These are
  internal plumbing for the binary with no API-stability promise, consistent
  with the existing crate set, so the cost is a manifest entry and a version pin,
  not a public-surface commitment.
- **A small hybrid.** `AgentConfig` projections live in `caliban-agent-core`
  rather than in the leaf crate, because `AgentConfig` is a runtime type, not
  leaf config data. This keeps the leaf crate free of runtime concerns at the
  cost of the projections not all living in one place; the dependency direction
  is correct regardless.
- **A future CI guard** (tracked separately) can enforce the restored layering so
  the upward edge cannot silently return.
