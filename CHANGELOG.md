# Changelog

All notable changes to caliban are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
While the project is pre-1.0, the minor version is bumped for new features and
the patch version for fixes.

## [Unreleased]

## [0.3.0] - 2026-06-17

This release pairs **extensibility** — config-defined hooks that now fire at
runtime, a SessionStart context-injection surface, and a proactive skill nudge —
with a **reliability pass on the background sub-agent supervisor**, hardening the
EventHub and worker channels against panics, unbounded memory, blocked readers,
and idle-timeout overshoot. It also adds a user-facing extended-thinking toggle
and tightens the Google provider's fault classification.

### Added

- **Config-defined hooks execute at runtime** (#121): `[[hooks.*]]` handlers in
  settings are now built into executing `command`/`http` handlers and composed
  into the agent's hook chain (previously they were parsed but never fired). This
  completes end-to-end external `[[hooks.SessionStart]]` `additionalContext`
  injection via the #106 surface. `disable_all_hooks` is honored;
  `allow_managed_hooks_only` conservatively fires none until handler scope
  provenance lands (#124); `mcp`/`prompt`/`agent` kinds are skipped with a warning.
- **SessionStart context-injection hook surface** (#106): `session_start` hooks
  can return a `SessionStartOutcome` whose `additional_context` is spliced into
  the system prompt before turn 1 (via a `<session-context>` block), letting skill
  packs / plugins ship their own activation preambles. The #56 built-in skills
  nudge remains an independent fallback. Ships a reusable `additionalContext`
  parser; runtime execution of config-defined hooks is tracked in #121. (#122)
- **Proactive skill-invocation nudge** (#56): the system prompt now lists loaded
  skills and instructs the model to invoke a matching skill before improvising,
  gated by `tools.skill_guidance`. (#105)
- **User-facing extended-thinking toggle** (#100): control extended thinking
  independently of the effort level. (#110)
- **Shared cross-cutting area labels** (#109): a shared "area core" label set
  spanning the caliban-ai repos. (#123)

### Changed

- **Immediate slash commands** (#13): 19 eligible slash commands now execute
  immediately instead of requiring a confirmation step. (#104)

### Fixed

- **Inbound frame reader no longer blocks on a full worker channel** (#118): the
  reader no longer stalls when the worker channel is full. (#134)
- **Worker idle timeout uses a deadline** (#119): switch to a deadline to remove
  idle-timeout overshoot. (#133)
- **EventHub history is bounded** (#116): cap EventHub history to bound worker
  memory. (#132)
- **Inherited workers honor parent allow/deny rules** (#114): propagate the
  parent runtime's allow/deny rules to inherited workers. (#131)
- **Poisoned EventHub history lock recovers** (#113): recover the lock instead of
  panicking. (#130)
- **Google provider fault classification** (#111): classify 400 context-overflow
  and in-band SSE faults. (#129)
- **`/memory delete` gated behind `--force`** (#112): destructive memory deletion
  now requires explicit confirmation. (#120)
- **Skipped skills surfaced** (#107): skills that fail to load are now reported
  instead of being silently dropped. (#108)

Docs: adopted the `docs/adr/` convention to match prospero + gonzalo (#125), and
centralized GitHub Pages — ADR ingestion, shared theme, rustdoc (#128).

## [0.2.0] - 2026-06-13

This release centers on **interactive background sub-agents** and the supervisor
machinery behind them: you can now spawn workers, attach to a live sub-agent's
transcript, send it input, and have it idle while awaiting more — all under an
inherited permission policy. It also adds observability flags, smarter Ollama
context detection, and a more robust streaming/permissions layer.

### Added

- **Interactive background sub-agents** (#81): sub-agents can now run in an
  `InputProvider` mode that idles awaiting input and resumes interactively,
  backed by a bidirectional per-agent socket (`SocketInputProvider`), a
  `worker → daemon` status channel with `AgentStatus::Idle`, and a `--interactive`
  spawn path. Interactive workers bound their idle time when no client is
  attached. (#87, #89, #90, #91, #92)
- **`caliban agents attach`** (#79): stream a running sub-agent's transcript
  live, with a send path to feed the attached agent input. (#82)
- **Worker permission gating** (#75): a spawned worker applies its
  `tool_allowlist` and a default permission gate; background sub-agents inherit
  the parent's permission policy via `inherit_hooks`. (#84, #85)
- **`--verbose` flag** (#27): emit full headless tool I/O for observability.
- **`--debug-file` flag** (#26): redirect the debug log to a chosen file.
- **Ollama context-window detection** (#60): detect a model's real context
  window via `/api/ps` + `/api/show` instead of assuming a default. (#64)
- **Full-fidelity event stream** (#78): `TurnEvent` now derives
  `Serialize`/`Deserialize`, enabling complete event-stream capture and replay.
  (#80)

### Changed

- **Permissions "Ask" modal UX overhaul** (#58): reworked the Ask modal and
  fixed a double-prompt so a decision is requested once. (#61)

### Fixed

- **Daemon-spawned workers honor a provider** (#93): the daemon now threads the
  requested provider through `SpawnSpec.provider` instead of defaulting. (#96)
- **Workers actually launch on Spawn** (#71): the supervisor launches a real
  worker on `Spawn`, fixing agents stuck in the `Spawning` state. (#74)
- **`agents rm --force` cleanup** (#76, #77): `--force` now signals the worker
  and the per-agent socket is cleaned up on exit. (#83)
- **Mid-stream failures classified correctly** (#63): body/decode failures that
  occur mid-stream are now classified as `StreamInterrupted` rather than a
  generic error.
- **Live permission rules** (#55): "Always allow" / "Always deny" rules now take
  effect immediately, in-session. (#57)

### Internal

- CI: resumable, rate-limit-aware crates.io publisher (#59) and a
  minimum-coverage gate with coverage tracking (#67).
- Testing: covered CLI subcommands and ratcheted the coverage floor 75 → 80 → 85
  (#68, #72, #73); de-flaked the `strict_routing` and `hooks_shell` races.
- Project: Kanban foundation — Kubernetes-style labels and board automation
  (#53).
- Docs: ADR 0047 for interactive background sub-agents (#86) and ADR hygiene
  pass (#98).

## [0.1.0] - 2026-06-06

Initial public release.

[Unreleased]: https://github.com/caliban-ai/caliban/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/caliban-ai/caliban/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/caliban-ai/caliban/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/caliban-ai/caliban/releases/tag/v0.1.0
