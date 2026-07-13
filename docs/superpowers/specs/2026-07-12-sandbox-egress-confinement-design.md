# Sandbox confinement: close egress by default

- **Date:** 2026-07-12
- **Tickets:** #406 (primary), #399 (epic). Demotes #405 to a follow-up.
- **Status:** designed, not implemented
- **Target release:** 0.7.0 (breaking)

## Problem

caliban's OS sandbox is an opt-in **write fence**. Under `--workspace` (or
`--restrict-paths`), Bash commands are wrapped so writes land only in the
workspace and temp dirs. Everything else is conceded:

- `workspace_fence_policy()` sets `allow_read: ["/"]` — the whole host is
  readable, including `~/.ssh`, `~/.aws/credentials`, `~/.config/gh/hosts.yml`,
  `~/.netrc`, caliban's own config, and the MCP OAuth token store
  (`$XDG_DATA_HOME/caliban/mcp-tokens.json`).
- It sets `allow_all_outbound: true` — egress is fully open.
- The child inherits caliban's entire environment, including
  `ANTHROPIC_API_KEY`, `CALIBAN_*` tokens, and `OTEL_EXPORTER_OTLP_HEADERS`.

File permissions buy nothing here: the sandboxed child runs as **the same uid**,
so mode `0600` on the token store is irrelevant. Reads plus egress means any
Bash command the model runs can ship your credentials to an arbitrary host.

The posture reads as broader confinement than it delivers, and a user who types
`--workspace` is precisely the user who believes they are protected.

## Threat model

The adversary is **not the user**. It is **untrusted content** — a repo file, a
fetched web page, an MCP tool result — that steers the agent into running a
command it should not run. The sandbox exists to bound the blast radius of a
prompt-injection-driven command.

This was never written down. Every sandbox ticket in the 2026-07 QA sweep
(#402, #403, #404, #405, #406, #407) traces back to its absence.

## What the field does

Surveyed 2026-07 (Codex CLI, Claude Code, Cursor, Gemini CLI, Goose, aider,
OpenHands, Devin):

- **Nobody read-jails.** Unanimous. Claude Code documents plainly that its
  default "still allows reading credential files such as `~/.aws/credentials`
  and `~/.ssh/`." Codex cannot restrict reads at all.
- **The serious harnesses close egress.** Codex CLI blocks network by default in
  `workspace-write` mode. Claude Code blocks it behind an allowlist + external
  proxy, with no domains pre-allowed, whenever its sandbox is on. Gemini CLI,
  Goose, and aider leave egress open — but Goose and aider ship *no sandbox at
  all*, so they promise nothing.
- **Env scrubbing is rarer.** Codex filters any var whose name contains
  `KEY`/`SECRET`/`TOKEN` by default; Claude Code offers opt-in deny/mask modes.
  Inheriting the parent env is otherwise the norm.

Anthropic's published rationale is the load-bearing argument: filesystem and
network isolation are only meaningful **together** — *"Without network
isolation, a compromised agent could exfiltrate sensitive files like SSH keys."*
Open reads are defensible **only because** egress is closed.

caliban currently concedes reads, egress, and env simultaneously. It is the only
surveyed harness that ships a sandbox while conceding all three.

## Decision

**Keep reads open. Close egress.** Match the field on reads (a read jail is
neither industry practice nor worth the breakage) and match Codex/Claude Code on
network.

**Stated guarantee, after this change:** under `--workspace`, a Bash command may
read your disk but cannot phone home; writes are confined to workspace + temp.
It is explicitly **not** a read jail and **not** a secrets boundary against an
attacker who already has egress by another route. That coupling — open reads are
safe *only* while egress is shut — is the core claim, and it goes in the ADR.

## Design

### 1. Close egress in the fence policy

`workspace_fence_policy()` (`caliban/src/startup/compose.rs:493-498`) flips
`allow_all_outbound: true` → `false`.

Both backends already implement deny-egress correctly; the path has simply never
been selected in production:

- **bwrap** emits `--unshare-net` when neither a proxy port nor
  `allow_all_outbound`/`allow_local_binding` is set.
- **Seatbelt** emits no `network-outbound` allow rule, and the generated profile
  is deny-by-default.

caliban's own provider HTTP calls are **unaffected** — they run in the parent
process, never through the shim. Only Bash tool commands lose egress. The shim
wraps nothing else: MCP stdio servers and sub-agent workers spawn on separate
paths (verified — `wrap_command` has exactly one non-test call site).

### 2. Fix the macOS loopback asymmetry

The two backends are **not** equivalent under deny-egress, and shipping without
this would give macOS a materially more broken sandbox than Linux:

| | Behavior with egress denied |
|---|---|
| **Linux / bwrap** | `--unshare-net` creates an isolated netns **with loopback up**. A command that binds `127.0.0.1:8080` and connects to it still works; it just cannot reach the host or the internet. |
| **macOS / Seatbelt** | No network rule denies **all** network, *including loopback*. Any test server, dev server, or suite that binds localhost **breaks**. |

Fix: add a local-only branch to the Seatbelt generator, driven by
`allow_local_binding`:

```lisp
;; Network: loopback only (allow_local_binding).
(allow network* (local ip "*:*") (remote ip "localhost:*"))
```

and set `allow_local_binding: true` in `workspace_fence_policy()`. Loopback
inside the sandbox is a low-exfil-risk convenience that keeps ordinary test
suites working on both platforms.

Note the existing bwrap condition (`!allow_all_outbound && !allow_local_binding`
→ `--unshare-net`) is correct as written for this: with `allow_local_binding:
true` bwrap would *not* unshare the net namespace, which is wrong — it would
leave real egress open. The bwrap branch must be changed so that
`allow_local_binding` alone still yields `--unshare-net` (isolated netns *is*
the loopback-only posture on Linux); only `allow_all_outbound` or a proxy port
should keep the host namespace.

### 3. Wire the config surface (hard prerequisite)

The `[sandbox]` TOML table is **dead code**: `config.rs` documents it, but
nothing in production deserializes a `Policy`. The only policy ever constructed
is the hardcoded `workspace_fence_policy()`. Every knob — `allow_read`,
`deny_read`, `allowed_domains`, `http_proxy_port`, `allow_unsandboxed_commands`,
`auto_allow_bash_if_sandboxed` — is unreachable by users today.

Closing egress with no escape hatch would strand anyone who needs `git fetch` in
a sandboxed command. So this ticket must also:

- **Load the `[sandbox]` table** into `Policy`, overlaying the fence defaults.
- **Add `--sandbox-network=deny|allow`** (default `deny` when the fence is
  active), as the one-flag escape hatch.

This also gives the #402/#403/#407 fixes a reachable surface for the first time.

`auto_allow_bash_if_sandboxed` is separately dead — `should_auto_allow_bash()`
has zero call sites. Wiring it is explicitly **out of scope**; it should be
either wired deliberately or deleted, tracked separately.

### 4. Error UX

Codex's single most-reported gotcha is `npm install` hanging with no
explanation. Do not reproduce it.

When a sandboxed command fails **and** egress was denied by policy, append a
hint to the tool result:

> network egress is blocked by the `--workspace` sandbox; re-run with
> `--sandbox-network=allow`, or configure a proxy under `[sandbox]`.

This converts the predictable top support complaint into a self-service fix.
Emit a one-line startup notice on the first sandboxed run for the same reason.

### 5. Out of scope (follow-ups)

- **#405 env scrubbing.** With egress closed this becomes honest
  defense-in-depth rather than theater, and it is what Codex ships by default.
  Recommended shape when picked up: filter vars matching `*KEY*`/`*SECRET*`/
  `*TOKEN*` in `wrap_with_program` (one choke point, both backends), plus
  `deny_read` masks on `~/.ssh`, `~/.aws`, `~/.config/gh`, `~/.netrc`, and
  `mcp-tokens.json` using the mask machinery #407 fixed. Keep #405 open,
  re-scoped to this.
- **Per-domain allowlists** via the loopback proxy (the Claude Code model).
  `http_proxy_port`/`socks_proxy_port` already exist in the policy and are
  honored by both backends; nothing ships a proxy. Future work.
- Read confinement. Explicitly rejected — not industry practice, high breakage,
  low marginal value once egress is shut.

## Migration — this is breaking

`--workspace` today means "fence writes." After this it also means "no network."
Sandboxed `git fetch`, `cargo build` against crates.io, `npm install`, `gh`, and
`curl` will fail unless the user opts out. This includes caliban's own
sprint/dogfood loops, which run under `--workspace`.

Ship in **0.7.0** with a prominent BREAKING changelog entry and the startup
notice above. Not a patch release.

## Testing

Per backend (`bwrap` on Linux, `sandbox-exec` on macOS):

1. Egress is denied by default under the fence — an outbound TCP connect /
   `curl` from a sandboxed command fails.
2. **Loopback still works on both platforms** — bind `127.0.0.1:<port>` and
   connect to it inside the sandbox. This is the regression that the macOS
   asymmetry would otherwise introduce.
3. `--sandbox-network=allow` restores egress.
4. `[sandbox]` TOML overlays the fence defaults (the surface is live).
5. The write fence still confines writes (no regression).
6. `workspace_fence_policy()` does not set `allow_all_outbound` — a unit test
   pinning the default posture so it cannot silently regress.

## Consequences

- A `--workspace` user gets a guarantee worth the name: a hijacked command can
  read the disk but cannot ship it anywhere.
- Sandboxed commands needing the network require an explicit opt-out, which is
  the same tradeoff Codex made and the same complaint Codex fields.
- The sandbox becomes user-configurable for the first time.
- caliban stops being the only surveyed harness that ships a sandbox while
  conceding reads, egress, and environment simultaneously.
