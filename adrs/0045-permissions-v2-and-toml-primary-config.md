# ADR 0045 · Permissions v2 — TOML-primary config + richer rule schema

- **Status:** accepted
- **Date:** 2026-05-31
- **Supersedes (partial):** ADR 0026 (settings layering) — refines write format and per-rule schema.

## Context

caliban shipped v1 permissions (ADR 0020), permission modes
(ADR 0029), and layered settings (ADR 0026) with JSON as the
canonical write format. Operator feedback and a security/UX review
surfaced four classes of problems: (1) the TUI Ask modal's "always
allow / always deny" never persisted, breaking the ADR 0020 promise;
(2) the JSON `permissions.{allow,ask,deny}` form lost source order
and comments; (3) JSON is the wrong primary format for a Rust
project where operators expect TOML and want hand-edited config that
ports between machines; (4) there was no full management surface
(CLI or in-TUI editor) for rules.

## Decision

1. **Restore TOML as caliban's canonical config write format** at
   every scope; JSON is accepted on read as a legacy/import path
   (with a WARN). All caliban-owned writes — modal, `/permissions`
   editor, `caliban perms` CLI — emit TOML.
2. **Replace the three-bucket `permissions.{allow,ask,deny}` form
   with an ordered `[[permissions.rules]]` array** of objects
   carrying `pattern`, `action`, optional `comment`, optional
   `reason` (deny-only, seen by the model), and reserved
   `expires_at`. First match wins. The three-bucket form still
   loads (legacy compat) but normalizes into the ordered array on
   load.
3. **Extend pattern grammar**: globstar `**`, path normalization
   for file-edit tools, `Bash:~glob` anywhere-match, dotted-key MCP
   arg accessors.
4. **Modal writeback (P1)**: y / n opens a sub-prompt with
   narrow-default suggestions, a scope picker, and an optional
   comment/reason. Atomic flock-protected TOML append.
5. **Active management surface**: `/permissions` overlay grows full
   editor capabilities; `caliban perms` CLI provides headless
   `list / test / explain / add / remove / import / export / audit / lint`.
6. **Hardening**: `permissions.enforce` lockdown knob, append-only
   JSONL decision log under `$XDG_STATE_HOME` with size-based
   rotation, always-visible bypass-latch chip with `ctrl+shift+b`
   drop keybind.

## Consequences

- **Positive**: matches Rust ecosystem norms; comments and source-order
  survive; the modal's promise is finally honored; operators have a
  complete management story (TUI + CLI); enforce + audit log close
  long-standing security gaps.
- **Negative**: doubles the schema surface during the compat window
  (legacy JSON + TOML buckets + v2 ordered rules coexist on read);
  the matcher gets a denser grammar (more to document).
- **Compat window**: legacy reads continue for two minor releases;
  writes deprecate immediately. After three minor releases only the
  canonical TOML schema loads.

## Revisit if

- Operators report concrete cases where the `~glob` or dotted-key
  grammars are insufficient — next step would be a richer expression
  language or a classifier-graded gate (already deferred via
  ADR 0029 auto-mode).
- The bypass-latch chip + drop keybind UX proves footgunny — could
  promote the drop to a confirmation dialog.
