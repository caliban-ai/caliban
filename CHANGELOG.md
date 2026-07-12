# Changelog

All notable changes to caliban are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
While the project is pre-1.0, the minor version is bumped for new features and
the patch version for fixes.

## [Unreleased]

## [0.6.0] - 2026-07-12

This release makes caliban **observable** and closes a broad **security-hardening
sweep**. OpenTelemetry support graduates from feature-gated stubs to a working
OTLP pipeline: `gen_ai.*` spans per model request and tool call, following the
OTel GenAI semantic conventions (ADR 0053), plus an OTLP metrics pipeline. It
also lands QA sweep epic #399 — 28 fixes, a dozen of which close **security
fail-opens shipped in 0.5.0**, where a permission or sandbox control was
configured but silently inert. **Review before upgrading:** several
permissions/sandbox paths now fail *closed* — a malformed permissions config, an
unusable sandbox, or an unparseable domain ACL is now an error rather than a
silent no-op, so a config that "worked" in 0.5.0 may now refuse to start.

**The sandbox is not a secrets boundary in this release.** Sandboxed commands
still inherit the full parent environment, so provider credentials
(`ANTHROPIC_API_KEY`, `GH_TOKEN`, …) are readable by any command caliban runs,
and the default fence leaves network egress open — a sandboxed command can
exfiltrate them. Do not rely on the sandbox to contain a command you would not
trust with your API keys. Tracked in #405.

### Added

- **OTel GenAI tracing over OTLP** (#375, ADR 0053): the OTLP span exporter is
  real rather than a feature-gated stub (#383), and caliban emits a `gen_ai`
  chat-generation span per model request (#378, #384) and an `execute_tool` span
  with `gen_ai.tool.*` attributes per tool call (#386).
- **Optional prompt/completion capture on spans** (#380): `gen_ai` input and
  output messages are recorded on spans when `OTEL_LOG_USER_PROMPTS` is set —
  off by default, since they contain user content. (#387)
- **OTLP metrics pipeline** (#427): a real `MeterProvider` with a periodic
  reader honoring `OTEL_METRIC_EXPORT_INTERVAL`, with `MetricEmitter::record`
  wired into live instruments, so emits reach the collector. `session.count`
  flows today; cost/token/active-time emits are tracked in #467 and OTLP mTLS in
  #465. (#468)
- **Bounded tool `result_text` on the agent stream** (#391): tool results are
  surfaced on the stream with a size bound. (#394)
- **Opt-in tool-dispatch timing** (#28): stream-json `tool_result` frames can
  carry a `t_ms` dispatch duration. (#398)
- **Per-turn thinking cap** (#62): a per-turn thinking-character cap bounds
  runaway extended-thinking spirals. (#396)
- **Fail-closed token + TLS on the `caliband` TCP session plane** (#288): the
  networked session plane requires a bearer token and TLS rather than accepting
  unauthenticated plaintext. (#395)

### Changed

- **Permissions and sandbox controls fail closed** (#410, #403, #404): an
  unparseable permissions config surfaces its parse error instead of silently
  dropping rules, v2 rule sets concatenate rather than overwrite, and an
  unusable sandbox or malformed domain ACL is an error rather than a silent
  no-op. (#438, #440, #437)
- **Run span covers polling** (#385): `gen_ai` spans nest under the run span
  rather than escaping it. (#389)

### Security

Twelve of these close controls that were configured but **inert** in 0.5.0 — the
policy was accepted and then not enforced. **Known gap:** the sandboxed child
environment is still unscrubbed (#405) — see the note above. These fixes harden
the *write* and *egress* fences; they do not make the sandbox a secrets boundary.

- **Sandbox domain ACLs no longer no-op or invert egress** (#403): a malformed
  or unsupported domain ACL silently allowed all egress, and in one path
  inverted allow/deny. Now fail-closed. (#440)
- **`allow_unsandboxed_commands` cannot be bypassed by chaining** (#402): a
  chained command (`allowed && evil`) escaped the sandbox via the allowlisted
  prefix. (#434)
- **bwrap deny-masks apply to files and actually block writes** (#407): masks
  covered directories only and did not prevent writes. (#450)
- **Workspace-fence TOCTOU closed on the write path** (#415): writes now go
  through a confined path, so the fence cannot be raced between check and open.
  (#459)
- **`/rewind` refuses to write through a planted symlink** (#413): checkpoint
  restore no longer follows a symlink out of the workspace. (#448)
- **`caliband` control plane and transport fail closed** (#400, #401): the
  control-plane listener no longer serves unauthenticated clients, and the
  transport is hardened against pre-auth DoS and token-comparison timing leaks.
  (#433, #436)
- **MCP OAuth hardening** (#431, #430): fixes a callback DoS and a cross-process
  token-store race, and enforces https on every OAuth discovery hop and bearer
  attach. (#456, #443)
- **Legacy `hooks.toml` fenced from HTTP-hook allowlists** (#409): a legacy hooks
  file can no longer inject entries into the user-managed HTTP-hook allowlist.
  (#435)
- **No false macOS per-host egress rule; fence root canonicalized** (#408): the
  Seatbelt profile no longer emits a per-host egress rule it cannot enforce, and
  the fence root is canonicalized before comparison. (#462)
- **Bound `gen_ai.*.messages` span attribute size** (#428): prompt capture cannot
  emit unbounded attributes. (#451)
- **Bump `crossbeam-epoch` to 0.9.20** (RUSTSEC-2026-0204). (#457)

### Fixed

- **Large Bash output can no longer deadlock** (#416): stdout/stderr are drained
  past the output cap, so a command producing more than the cap no longer hangs.
  (#439)
- **Checkpoint durability** (#412): transactional restore, correct eviction
  ordering, and an atomic index write. (#444)
- **Deferred session-write failures surface; debounce is bounded** (#414): a
  failed deferred write is reported rather than swallowed. (#461)
- **Extended-thinking signatures survive streaming** (#419): thinking-block
  signatures are preserved through the stream, so thinking blocks remain valid on
  replay. (#442)
- **Streaming robustness** (#424): usage deserialization, a `line_buf` cap,
  tool-id handling, and truncation. (#455)
- **Anthropic prompt-cache tokens land in usage** (#423): cache-read/write token
  counts from `message_start` are carried into the usage totals. (#446)
- **Compaction shrinks oversized `tool_use`/thinking blocks** (#421): caps are
  taken from the active model rather than a fixed constant. (#449)
- **No duplicate assistant message on surrender; empty content blocks dropped**
  (#422). (#452)
- **No-edit nudge cannot preempt a terminal stop reason** (#420). (#447)
- **Stable parallel-tool conflict key; new files respect umask** (#417). (#453)
- **`/config` attributes each key to its true source scope** (#411): keys are no
  longer misattributed to the wrong config file. (#463)
- **Pre-flight fatals route through the output encoder** (#429): a fatal before
  the stream opens is emitted in the requested output format rather than as bare
  text. (#464)
- **OTLP gRPC auth headers are applied** (#426): headers configured for the gRPC
  exporter are attached via tonic metadata, and the headers helper is applied at
  startup. mTLS client certs remain unwired (#465). (#466)
- **MCP robustness** (#432): utf8-safe template handling, bounded elicitation,
  and truncated error bodies. (#454)
- **Warm Ollama turns skip the static `/api/show` probe** (#425): the capability
  probe no longer re-runs on every turn. (#458)

Docs: adopt OTel GenAI semantic conventions, semconv-only (ADR 0053, #376, #382);
reconcile ADR 0033 headers-helper drift to env-only reality (#381, #388); publish
`CHANGELOG.md` to the Pages site (#373).

## [0.5.0] - 2026-07-05

This release turns caliban from a single-process CLI into the base of a
**distributed, self-hostable agent supervisor**. `caliband` gains a networked
control plane, workspace-scoped multi-repo supervision, and multi-arch container
images; caliban learns to consume gonzalo's code-graph over MCP; and
config/data relocate to XDG-first locations. It also lands a broad reliability,
OAuth, and security-hardening pass. **Behavior changes to review before
upgrading:** config/data/cache/state now live in XDG locations (previously the
platform GUI dirs on macOS), and `--workspace` now fences file writes by
default. The `caliband` network transport is **beta** (hardening tracked in
#319/#320).

### Added

- **Workspace-scoped `caliband`** (#281): the supervisor now manages a workspace
  spanning multiple sources, with per-source worktree isolation wired end to
  end. (#325)
- **`caliband` network transport** (#280) — *beta*: the daemon can serve its
  control plane over NDJSON on TCP with rustls TLS + a bearer token
  (`--listen`/`CALIBAN_DAEMON_LISTEN`), so a remote client (e.g. prospero) can
  drive it across the network rather than only a local Unix socket. (#321)
- **Consume the gonzalo code-graph MCP server** (#308): a config entry wires
  gonzalo's `search`/`node`/`callers`/`callees`/`impact`/`explore` tools into the
  agent, over stdio or HTTP. (#310)
- **Dynamic Ollama model discovery** (#316): the Ollama provider builds its model
  list and capabilities from the runtime API rather than a static table. (#322)
- **OAuth Dynamic Client Registration (RFC 7591)** (#313): `oauth="auto"` MCP
  servers self-register a client when the provider supports DCR. (#315)
- **Multi-arch container image** (#279): `caliban` + `caliband` ship as a single
  linux/amd64 + linux/arm64 image on GHCR. (#298)
- **Build commit in `--version`** (#303): binaries built from a git checkout now
  report the commit they were built from — `caliban --version` prints e.g.
  `caliban 0.5.0 (<sha>, <date>)`, appending `-dirty` for an uncommitted tree.
  Builds without git metadata (release tarballs, crates.io installs) fall back to
  the bare semver. (#305)

### Changed

- **XDG-first config/data/cache/state on all platforms** (#295): caliban now
  stores its config, data, cache, and state under XDG locations everywhere — on
  macOS this moves them out of `~/Library/Application Support` (ADR 0050).
  Existing files in the old locations are not migrated automatically. (#297)
- **`--workspace` fences file writes by default** (#237): under `--workspace`,
  file writes are restricted to the workspace unless you pass
  `--no-restrict-paths`. (#273)
- **Streaming-timeout policy raised above the transport layer** (#330): connect,
  first-byte, and total-exemption timeouts are applied uniformly above the
  provider transport, so a hanging `stream()` call is bounded like a silent
  stream. (#364)
- **Boolean CLI flags accept an optional `=BOOL` value** (#223): flags like
  `--foo` now also accept `--foo=true` / `--foo=false` consistently. (#293)

### Security

- **Clear all cargo-audit advisories + add a CI advisory gate** (#258): resolved
  all 6 outstanding `cargo-audit` findings and added a CI gate that fails on new
  advisories. (#260)
- **OAuth discovery enforces https + issuer match** (#339): MCP OAuth discovery
  rejects non-https endpoints and mismatched issuers. (#357)
- **OAuth token store writes are atomic + `0600`** (#341): the OAuth token
  `FileStore` writes through a temp file with `0600` permissions. (#355)
- **Collapse `..` before the workspace fence check** (#327): path traversal is
  normalized before the workspace boundary is enforced. (#346)
- **Fence Bash writes via the OS sandbox under `--workspace`** (#328):
  background and foreground Bash writes are confined by the OS sandbox when a
  workspace fence is active. (#348)

### Fixed

- **Real compactor wired for `/compact` + autocompact** (#292): `/compact` and
  automatic compaction now run an actual compaction pass. (#294)
- **Compactor strategy correctness** (#329): fixes orphaned blocks, summarizer
  input, usage accounting, and the in-window tail. (#349)
- **Prefill-aware stream watchdog + total-timeout exemption** (#263, #254): a
  slow first chunk within the prefill budget no longer trips the idle watchdog,
  and the stream path is exempt from the total timeout. (#269)
- **Sandbox degrades gracefully when user namespaces are denied** (#345):
  `caliband` now probes for an actual user namespace and runs unsandboxed (with a
  warning) instead of failing every tool call on runtimes that install `bwrap`
  but forbid unprivileged userns. (#371)
- **`write_atomic` preserves special bits + writes through symlinks** (#335). (#350)
- **`Write`/`Edit` produce `0644` (or preserve mode)** (#224): edited files no
  longer inherit the tempfile's `0600`. (#291)
- **Result frame = final message + additive CC-contract fields** (#222): the
  headless result frame carries the final message and the additive
  Claude-Code-contract fields. (#276)
- **`duration_ms` is whole-session in multi-frame stream-json** (#331). (#351)
- **Per-tool dispatch records + ignore/globset spam filtered** (#256). (#257)
- **MCP OAuth: cache before discovery, persist DCR `client_secret`** (#333). (#352)
- **Wire the MCP OAuth auto/manual flow into the connection path** (#300). (#304)
- **Complete settings-path MCP config plumbing** (#309, #311): env expansion and
  the OAuth callback port are honored for settings-file MCP servers. (#312)
- **Windows OAuth browser opener no longer truncates the auth URL** (#338). (#356)
- **Settings `${VAR}` expander preserves non-ASCII text** (#340). (#354)
- **`paths.rs` XDG env-edge hardening** (#336): relative fallbacks and
  non-absolute env values are handled. (#353)
- **`--version -dirty` ignores untracked files** (#306). (#307)

Testing: hermetic gonzalo code-graph contract test via an in-tree mock MCP
server, with the live round-trip honestly `#[ignore]`d (#344, #367); positive
prefill-grace assertions (#334, #368); closed the `no_bare_platform_dirs` guard
coverage holes (#337, #369).

Docs: introduced `docs/evaluation/` and reorganized the probe + competitor docs
(#361, #362); corrected the `container.md` sandbox-fallback caveat (#345, #370).

Internal: migrated helper scripts from Python to bash (#360, #365); tracked
`rebuild.sh` and gitignored the Python bytecode cache (#359); container images
now build each arch natively on GitHub arm64 runners, dropping QEMU (#302).

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

[Unreleased]: https://github.com/caliban-ai/caliban/compare/v0.6.0...HEAD
[0.6.0]: https://github.com/caliban-ai/caliban/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/caliban-ai/caliban/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/caliban-ai/caliban/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/caliban-ai/caliban/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/caliban-ai/caliban/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/caliban-ai/caliban/releases/tag/v0.1.0
