# ADR 0059 · ACP over the network + per-session permission posture

- **Status:** accepted
- **Date:** 2026-09-18
- **Source:** [#673](https://github.com/caliban-ai/caliban/issues/673); driver [prospero#217](https://github.com/caliban-ai/prospero/issues/217). **Amends [0055](0055-driveable-server-surface.md)** (driveable server surface); builds on [0051](0051-caliband-network-transport.md) (caliband TLS/TCP transport) and [0045](0045-permissions-v2-and-toml-primary-config.md) (Permissions v2). References #527 (drive-surface bearer gate, shipped) and #528 (permission-elicitation bridge, shipped).

## Context

[ADR 0055](0055-driveable-server-surface.md) shipped the ACP adapter (`caliban acp serve`, `caliban/src/serve/acp.rs`) as **stdio-only**, with an auth model of "stdio is loopback-inherent." That is the right posture for a human driving caliban from a local editor, but it does not reach the deployment that now needs ACP most: in Kubernetes, prospero (the control plane) reaches agents over caliband's TLS/TCP control plane (`:8443`) and per-agent worker listeners (`__agent-worker --listen 0.0.0.0:7100`, ADR 0051), *not* over stdio. Nothing today puts ACP on that network path.

prospero#217 (raised to `priority/important-soon`) makes ACP prospero's preferred long-term drive path — it carries permission decisions and tool results that the NDJSON wire handles poorly — and adds an operator requirement: **a human chooses the permission posture per session**, in one of two modes:

- **Supervised.** Tool calls that hit an `Ask` rule pause and are approved or denied by a human in prospero via `session/request_permission` (already implemented in the adapter, #528, `acp.rs:468-491`). This must work over the network path, not only stdio.
- **Unattended.** The session runs without stalling on prompts — it executes under a permission profile that does not ask (a bypass or auto-allow profile), so a long task never blocks on a human.

Two facts constrain the design. First, ADR 0055 already anticipated networked surfaces: its Auth section requires a **bearer token for any non-loopback surface** (reusing 0051's scheme — shipped as the drive-surface gate #527), and its Permissions section requires that **"no surface may bypass the permission model or widen a run's authority beyond its profile."** Second, the current in-cluster reality gives agents *neither* posture: the `__agent-worker` default gate hardcodes `NonInteractiveAskHandler { auto_allow: false }` (`worker.rs:763`), so every `Bash`/`Write`/`Edit` is denied non-interactively and nothing reaches a human; `CALIBAN_AUTO_ALLOW` / `CALIBAN_NO_PERMISSIONS` / `CALIBAN_DEFAULT_PERMISSION_MODE` are not read by the worker, and the only lever it honors is the opaque `SpawnSpec.inherited_hooks_config` (`worker.rs:750`), which prospero does not send.

Three transport options were weighed (prospero#217):

1. **caliband hosts ACP sessions** — a new caliband-side ACP endpoint (TLS + `CALIBAN_DAEMON_TOKEN`) multiplexing to workers. One central endpoint, but new multiplexing machinery.
2. **The per-agent worker speaks ACP on its existing listener** — ACP as an alternative protocol to NDJSON on the worker's TLS+token listener, reusing the drive core.
3. **A generic stdio↔socket bridge** in front of `caliban acp serve` — simplest, but bypasses caliband's supervision, TLS and token model.

## Decision

**1. Transport — the per-agent worker speaks ACP on its existing network listener (Option 2).** The worker offers ACP as an alternative protocol on the same TLS + bearer-token listener it already exposes for the NDJSON session plane (ADR 0051; #319/#320), reusing the ADR 0055 drive core. prospero already dials the per-agent port, so ACP rides the path it already secures — behind the shipped drive-surface bearer gate (#527) — with no new caliband endpoint and no new supervision/TLS/token machinery. We reject Option 3 (it drops supervision, TLS and the token model — exactly the guarantees 0055's auth section requires of a non-loopback surface). Option 1 remains viable but is deferred: it adds a central endpoint and session multiplexing we do not yet need. This **amends ADR 0055**, whose ACP adapter was scoped stdio-only; the stdio `caliban acp serve` path is retained unchanged for local editor drive-in.

**2. Auth — the networked ACP surface reuses the drive-surface gate.** The ACP-over-network path requires TLS + a bearer token exactly as the NDJSON session plane does (0051 + #527). This is not a new auth model; it is 0055's own "non-loopback surface requires a bearer token" applied to ACP. The loopback stdio path keeps its loopback-inherent posture.

**3. Per-session permission posture — the profile is the lever, never the adapter.** A session carries an explicit permission posture, chosen per spawn:

- **Supervised** → the run drives through Permissions v2 (0045); an `Ask` is surfaced over the ACP `session/request_permission` bridge (#528) on the network path, and the human's decision is delivered back as an input.
- **Unattended** → the session runs under an explicit bypass/auto-allow *profile* (Permissions v2 `bypassPermissions` mode, or auto-allow of `Ask` rules).

This **refines ADR 0055's** "no surface may bypass the permission model": the adapter still never bypasses anything — it faithfully drives whatever the session's profile says. Unattended is a property of the *session's chosen profile*, not of the transport. Choosing a bypass profile is a privileged act, gated by scope (**`admin` under prospero's ADR 0010** — `docs/adr/0010-inbound-api-authentication.md` in `caliban-ai/prospero`, the `read` < `operate` < `admin` token-scope model) and enforced **upstream** (prospero / the operator's Workspace policy, caliban-operator#80), never by the caliban worker itself; the default posture is **supervised/deny** (fail-closed), so an unconfigured spawn is never silently unattended.

**4. Expression + the worker honors it.** The posture is expressed on `SpawnSpec` (a typed field, e.g. `permission_posture`, superseding the opaque `inherited_hooks_config` bridge for this purpose) and, for k8s, on the `CalibanTask` CR (a caliban-operator change, tracked on that side). The `__agent-worker` gate is rebuilt to construct its `AskHandler`/permission profile from that field — replacing the hardcoded `NonInteractiveAskHandler { auto_allow: false }` — so in-cluster agents get the posture the operator chose (this fixes the current "every tool denied" blocker for the NDJSON path too, not just ACP).

**5. `allow_always` permission option.** The ACP `session/request_permission` gains an `allow_always` option (today only `allow_once`/`reject_once`, `acp.rs:485-486`) — the natural middle ground between one-shot supervision and a fully unattended profile; an `allow_always` decision persists a session-scoped allow rule.

**Scope.** This ADR records the transport choice, the auth reuse, and the permission-posture model, and spawns the implementation child tickets below. The ACP adapter's **data-parity gaps** (tool-call input, `TurnEnd`/`RunEnd` accounting, `loadSession`/resume) are transport-independent and tracked separately in **#674**; `allow_always` is listed there too and is folded into this posture work.

**Child tickets to spawn (on #673):**

1. **ACP on the worker's network listener** — the worker offers ACP as an alternative protocol on its TLS+token listener, reusing the drive core, behind the #527 gate. (Option 2 transport.)
2. **`SpawnSpec.permission_posture` + the worker honors it** — a typed posture field; rebuild the `__agent-worker` gate to construct the profile from it (supervised / unattended-bypass / auto-allow), default supervised/deny; retire the opaque `inherited_hooks_config` path for posture. Fixes the current hardcoded-deny blocker for NDJSON and ACP alike.
3. The adapter data-parity gaps + `allow_always` are **#674**; the `CalibanTask` CR posture field is a **caliban-operator** ticket (flagged to that side).

## Consequences

- **Positive.** prospero can drive in-cluster agents over ACP on the path it already secures, with both permission postures, without a new caliban endpoint or a second auth model. The worker-gate rebuild (child #2) un-blocks in-cluster agents *today* — they currently deny every mutating tool regardless of transport. The bypass posture is authorized and fail-closed by default, honoring 0055's "no surface bypasses the model" by making the profile — not the transport — the only lever. The stdio ACP path is unchanged for local editors.
- **Negative.** The worker now speaks two protocols (NDJSON + ACP) on one listener — protocol negotiation and a second codec to keep stable. A new privileged `SpawnSpec` field (bypass profile) is a security-sensitive surface: it must be scope-gated (admin) and audited, or it becomes a way to silently run agents unsupervised. Retiring `inherited_hooks_config` for posture is a migration touch-point. The `CalibanTask` CR change is cross-repo (caliban-operator) and must land in step for the k8s path to work end-to-end.
- **Revisit if.** A central ACP endpoint becomes worth its cost (many short sessions, or a need to drive agents that have no dedicated port) — reopen Option 1 (caliband-hosted multiplexing). Or if the two-protocol listener accretes divergent logic rather than staying thin codecs over the one drive core — the 0055 core abstraction is leaking; refactor before adding a third protocol.
