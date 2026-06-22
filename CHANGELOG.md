# Changelog

All notable changes to caliban are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
While the project is pre-1.0, the minor version is bumped for new features and
the patch version for fixes.

## [Unreleased]

## [0.4.0] - 2026-06-21

This release is a **security and protocol-correctness pass** on top of a completed
architecture-refactor sprint. It tightens permission resolution, hook egress, and
tool sandboxing across the board; aligns the headless result frame and terminal-stop
reporting with ADR 0025; hardens the model-router circuit breaker; and adds two TUI
input conveniences. **Behavior change:** several permission fixes are intentionally
stricter than v0.3.0 — actions that were previously allowed (broad MCP allows, `../`
workspace escapes, mode-weakening flags under lockdown) are now denied. Review the
Security section before upgrading if you depend on the looser behavior.

### Added

- **Fuzzy slash-menu typeahead** (#15): the slash-command menu now matches on
  fuzzy subsequences instead of prefix-only. (#146)
- **Backslash-continuation for multi-line input** (#101): end a line with `\` then
  Enter to continue input across lines. (#136)

### Changed

- **Plan mode gates on tool capability, not a name allowlist** (#162): read-only
  enforcement now keys off `Tool::is_read_only`, so plan mode correctly blocks any
  mutating tool rather than a hard-coded set. (#207)
- **One resettable wakeup per watched stream** (#117): replace the per-poll wakeup
  with a single resettable timer, cutting stream-polling overhead. (#139)

### Security

- **`deny:mcp__*` outranks server allows** (#213): global rules are partitioned by
  action so a deny rule can no longer be overridden by a more specific server allow. (#228)
- **Block `../` workspace escape** (#216): relative path traversal out of the
  workspace is rejected, and a reason sentinel can no longer flip a static Deny. (#231)
- **SSRF guard + scoped HTTP-hook allowlists** (#217): http-hook allowlists are
  scoped to user/managed, URLs are matched by component, and an SSRF guard blocks
  internal-address egress. (#232)
- **Preserve static Deny under acceptEdits** (#169): acceptEdits mode no longer
  weakens an explicit Deny rule. (#188)
- **Lockdown refuses mode-weakening flags** (#178): enforce-lockdown rejects CLI
  flags that would relax the active permission mode. (#191)
- **Workspace-scope relative file-edit patterns** (#177): relative edit patterns are
  resolved against the workspace root rather than matching loosely. (#189)
- **OS sandbox applied to background Bash** (#160): background Bash commands now run
  under the same OS sandbox as foreground ones. (#190)
- **Marketplace downloads use the hardened HTTP client** (#158): plugin marketplace
  fetches route through the SSRF-guarded client. (#187)
- **Checkpoint byte-cap + integrity hardening** (#220): enforce the checkpoint
  byte-cap, restore integrity checks, and prompt the index before the hook is wired. (#235)

### Fixed

- **Model router — circuit-breaker recovery** (#215, #183): fix recovery
  concurrency, the recovery state machine, and hedge bugs; wire recovery to use a
  single probe and require N successes before closing. (#230, #200)
- **Model router — route-resolved reasoning effort applied** (#173): the effort
  resolved by the route is now applied to outgoing calls. (#198)
- **Model router — tool-use capability parsing** (#172): parse the `tool_use`
  capability requirement as a string enum. (#196)
- **Model router — router-debug default model** (#144): derive the router-debug
  default model from CLI args. (#150)
- **Headless result frame stays terminal** (#218): the result frame is kept terminal
  and `stream-json` input now activates headless mode. (#233)
- **Headless protocol parity** (#184): budget no longer masks success, max-turns
  reporting reaches parity, and EventKind drift is corrected. (#208)
- **Headless text mode surfaces non-success stops** (#175): text output now reports
  non-success terminal stops instead of swallowing them. (#203)
- **`--json-schema` applied reliably** (#214, #174): the directive applies even when
  a system message exists, instructs the model, and ships three validator fixes. (#229, #204)
- **Generation-agnostic Anthropic rate-card globs** (#142): rate-card matching no
  longer pins to a specific model generation. (#148)
- **`type:"text"` on Anthropic system blocks** (#141): emit the explicit block type. (#147)
- **Provider stream-interruption retry** (#245): retry stream interruptions that
  occur before any content is produced. (#248)
- **Google 400 bodies no longer trip fault markers** (#221): classify Google 400
  bodies correctly and fix the Vertex `list_models` doc. (#238)
- **Sub-agent no-edit nudge resets on edits** (#244): the no-edit nudge resets on
  actual file edits, not on any side-effecting tool. (#246)
- **AgentTool prompt truncation on a char boundary** (#219): truncate the AgentTool
  Debug system prompt on a UTF-8 char boundary to avoid a panic. (#234)
- **Sub-agent signal races** (#115, #138): serialize Kill/Respawn on the registry
  lock and close the `Rm --force`/Respawn signal race. (#137, #145)
- **`agents logs` reads the worker transcript** (#143): read the worker's transcript
  file instead of the wrong source. (#149)
- **Hooks — filter/async/exit-2 handling** (#171): apply the `if` filter, honor
  `async`, and stop swallowing exit-2 denials. (#195)
- **Hooks — validation hardening** (#185): event↔kind validation, `${VAR}` header
  expansion, UpdatedInput validation, and a truncation panic fix. (#209)
- **Checkpoint byte-cap sweeper** (#180): implement the per-project blob byte-cap
  sweeper. (#205)
- **Checkpoint rewind correctness** (#181): rewind keeps the prompt's assistant turn
  and uses a deterministic cwd hash. (#206)
- **Agent-core — degenerate reasoning-only turns** (#249): nudge reasoning-only
  turns instead of ending the run empty. (#253)
- **Agent-core — MicroCompactor result ordering** (#170): a failed result no longer
  supersedes a good one. (#193)
- **Agent-core — ToolResultCap window clamp** (#182): clamp head/tail windows to
  avoid overlap. (#199)
- **Eval follow-ups** (#240, #241): whitespace-tolerant Edit and transient-5xx
  retry. (#242)
- **Config migrate detects legacy `permissions.toml`** (#176). (#202)
- **Perms CLI predictors mirror runtime** (#179): plus lossless ordered export. (#201)

Internal: completed the architecture-refactor sprint (#152–#168) — extracted
`RecoveryState` and pure helpers from the turn-loop, segregated the `Hooks` trait,
decomposed the plugins-manager and TUI startup modules, converged provider transport
plumbing and error classification, centralized frontmatter/path helpers, and migrated
`/usage`, `/context`, `/compact` onto the slash-command registry.

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

[Unreleased]: https://github.com/caliban-ai/caliban/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/caliban-ai/caliban/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/caliban-ai/caliban/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/caliban-ai/caliban/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/caliban-ai/caliban/releases/tag/v0.1.0
