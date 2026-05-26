# ADR 0044 · `rmcp` 1.7 version pin

- **Status:** accepted
- **Date:** 2026-05-26

## Context

`caliban-mcp-client` depends on `rmcp` — the Model Context Protocol
Rust SDK. The workspace `Cargo.toml` pins it at `1.7.x`:

```toml
rmcp = { version = "1.7", features = [...] }
```

This is a tighter pin than the typical Rust convention of "compatible
with the listed version" (`^1.7` allows any 1.x.y where x ≥ 7). The
choice was made when adopting `rmcp` and never recorded. The
2026-05-25 ADR conformance audit (Finding 7) flagged the gap.

## Decision

Pin `rmcp` at the `1.7.x` minor.

Bumps to a new minor (1.8, 1.9, etc.) are landed in a single dedicated
PR after:

1. Reading the upstream changelog for breaking changes affecting our
   MCP transport, OAuth, elicitation, or resource surface (ADRs 0017,
   0023).
2. Verifying our integration tests still pass against the bumped
   version.
3. Spot-checking the canonical reference MCP servers (a stdio server,
   an HTTP+OAuth server) end-to-end.

Patch bumps within `1.7.x` (1.7.0 → 1.7.1) are auto-resolved by Cargo
and do not require a dedicated PR.

## Consequences

- **Insulation from breaking changes** in MCP transport or server
  APIs between rmcp minor releases. Our surface
  (`crates/caliban-mcp-client/src/{client,transport,oauth,elicitation,resource}.rs`)
  is large enough that an unexpected upstream minor could mean a
  multi-day debug session.
- **Manual maintenance cost.** Each minor bump requires changelog
  review + integration test pass + a dedicated PR. Estimate: 1-3 hours
  per bump.
- **Predictable runtime behavior** for users running pinned binaries
  against established MCP servers. The wire protocol is stable across
  the 1.x line by upstream convention, but rmcp's API surface has
  reshaped between minors in the past.
- **Risk: lagging behind upstream** means missing protocol-level
  enhancements (e.g., new transport modalities, new elicitation
  features) until we explicitly bump. Mitigation: a quarterly
  changelog check is on the project cadence.
- **Risk: security updates** in a future minor (e.g., a fix in OAuth
  validation) require an immediate bump rather than auto-pulling.
  Mitigation: subscribe to the rmcp release notes / RustSec advisories.

## Revisit if

- rmcp reaches 2.0 — at which point the pin needs to move regardless,
  and the changelog review is mandatory.
- A security advisory affecting our usage of rmcp surfaces — bump
  immediately to the patched minor, write the dedicated PR
  retrospectively.
- The maintenance cost of staying current outweighs the insulation
  benefit (e.g., if upstream stabilizes such that minors stop
  reshaping the API).

## References

- `rmcp` crate: https://crates.io/crates/rmcp
- ADR 0017 (MCP stdio v1) and ADR 0023 (MCP v2 — transports, OAuth,
  elicitation, resources) — the surfaces that consume rmcp.
- Workspace pin: root `Cargo.toml` (`rmcp = { version = "1.7", ... }`).
- 2026-05-25 ADR conformance audit, Finding 7.
