# ADR 0032 · OS-level sandbox

- **Status:** proposed
- **Date:** 2026-05-24
- **Author:** john.ford2002@gmail.com
- **Spec:** `docs/superpowers/specs/2026-05-24-os-sandbox-design.md`
- **Depends on:** existing `caliban-tools-builtin::BashTool`
  (`crates/caliban-tools-builtin/src/bash.rs`).

## Context

Permission rules (ADR 0020) gate which commands the agent *asks* about
before running. They do nothing once a command is approved. An agent
that's been told `Bash(*)` is `allow` can rewrite the home directory
or exfiltrate via curl with no friction. Claude Code mitigates this
with an OS-level sandbox — Seatbelt on macOS, bubblewrap on Linux —
that restricts the child process itself. With the sandbox enabled,
operators can drop the per-command Ask entirely (`autoAllowBashIfSandboxed`)
because the sandbox is the protection.

Matrix row A "OS-level sandbox" is 🔴 and flagged as a big lift /
security-critical. This ADR records the decision to ship it as a
shim layer over the existing Bash plumbing.

## Decision

### Two backends, one config surface

- **macOS:** `sandbox-exec` with a generated `.sb` (TinyScheme dialect)
  profile written to `$XDG_RUNTIME_DIR/caliban/sandbox/<sessid>.sb`.
  Profile is computed once per session from settings.
- **Linux (and WSL):** `bwrap` with `--bind`/`--ro-bind`/`--tmpfs`
  flags plus optional `--unshare-net` and `--unshare-user`. Argv is
  computed once per session.
- **Windows native:** not supported in v1. Refuses to enable; documents
  Job Objects + AppContainer as the v2 path.

A single `[sandbox]` settings block drives both backends. Operators
configure intent (allow-write paths, allowed domains, etc.); the
backend translates intent into its native policy language.

### A new crate `caliban-sandbox` provides a shim, not a rewrite

`caliban-sandbox` exposes `SandboxedShim::wrap_command(cmd,
command_str)` which either returns `cmd` unchanged (sandbox disabled,
or command on the unsandboxed allow-list) or wraps it in a new
`tokio::process::Command` whose program is `sandbox-exec` / `bwrap`
and whose tail is the original command. `BashTool::invoke` calls
`wrap_command` after building its base `Command`; everything else —
stdout/stderr capture, PID-group cleanup, cancellation, timeouts —
stays identical.

This keeps the change tightly scoped: the sandbox is a layer, not a
fork of Bash.

### `auto_allow_bash_if_sandboxed` short-circuits the Ask modal

Setting `sandbox.enabled: true` *and*
`sandbox.auto_allow_bash_if_sandboxed: true` makes the permission
classifier short-circuit `Bash(*)` to `allow` before the Ask modal
would fire. Rule grammar isn't modified — the short-circuit sits
alongside plan-mode-bypass in the permission pipeline.
`allow_unsandboxed_commands` entries (commands that genuinely need
unrestricted access) are *not* auto-allowed; they keep going through
the normal rules because they're running unsandboxed.

The auto-allow knob defaults to `false`; both settings must be set
deliberately.

### Network egress is sandbox + proxy, not sandbox alone

Neither Seatbelt nor `bwrap` enforces per-hostname egress reliably on
its own. The supported patterns are:

- `allowed_domains = []`: deny all egress (`--unshare-net` /
  Seatbelt no `network-outbound`).
- `http_proxy_port = N`: deny all egress *except* `127.0.0.1:N`;
  the operator runs a domain-aware HTTP proxy at that port.
- Both unset, `allowed_domains` non-empty on Linux: a warning is
  logged; the sandbox is less restrictive than the operator
  probably intended. A v1.1 follow-up ships an in-tree minimal
  proxy that consumes `allowed_domains` natively.

macOS Seatbelt supports literal `(remote tcp "host:port")` allow
rules and is correspondingly stricter.

### Filesystem ACLs are explicit allow + deny + masks

Bubblewrap masks denied paths with `--tmpfs` (an empty in-memory
directory shadows the real one). Seatbelt uses
`(deny file-write* (subpath …))`. Globs aren't supported in the ACL —
operators add explicit roots. `${WORKSPACE}`, `${HOME}`, and the XDG
vars are expanded at session start.

### Detection runs at startup; `fail_if_unavailable` is the gate

`SandboxedShim::new` detects the backend, version-checks `bwrap`
(>= 0.5), and verifies path. When `fail_if_unavailable: true` and the
backend is missing or too old, caliban refuses to start instead of
running unsandboxed.

`enable_weaker_nested_sandbox: true` is the escape hatch for
dev containers (already inside a user namespace; `--unshare-user`
would fail). It drops the offending flags on Linux and is a no-op on
macOS.

## Consequences

- **Positive:** Closes matrix row A "OS-level sandbox" with a
  minimally-invasive shim. Reuses the existing PID-group cleanup
  logic (the wrapper inherits the child's group). Unlocks the
  `auto_allow_bash_if_sandboxed` UX — Bash becomes a one-keystroke
  tool when the sandbox is properly configured. Two backends and one
  config surface means operators move between macOS dev and Linux CI
  without rewriting policy.
- **Negative:** Seatbelt is deprecated by Apple (no replacement
  ship-date). Bubblewrap requires an external binary (`bwrap >= 0.5`)
  that isn't installed by default on every distro. Per-hostname
  network rules need a proxy to enforce reliably; we don't ship one
  in v1. Windows isn't supported (deferred). The policy languages are
  fiddly and undocumented (Seatbelt) or terse (`bwrap` argv), so
  debugging operator misconfiguration takes care.
- **Revisit if:** Apple removes `sandbox-exec` (move to Endpoint
  Security Framework backend). A standard hostname-aware sandbox
  layer emerges (e.g. systemd-resolved per-process filtering). Demand
  appears for a Windows backend (Job Objects + AppContainer is the
  v2 path). Container-based sandboxing becomes the prevailing pattern
  on Linux (revisit with a Podman / Firejail backend option).
