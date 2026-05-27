# QA report — `caliban` release binary, v1 readiness

> **Status (2026-05-27):** every finding below is closed on branch
> `worktree-qa-v1-binary`. See the PR for the fix commit. The report
> is kept as-is for traceability — each finding still cites the
> file:line that motivated the fix.

- **Probe date:** 2026-05-26
- **Branch:** `worktree-qa-v1-binary` (worktree off `main` at `469f9d9`)
- **Binary:** `target/release/caliban`, 16 MB, built clean (`cargo build --release` exit 0)
- **Probe method:** read the CLI source (`caliban/src/{args,main,startup,headless,diagnostics,plugin_cli,subcommands}.rs`),
  ADRs (0025, 0026, 0029, 0030, 0037, 0040, 0042), the README, the
  parity-gap matrix, and the 2026-05-25 ADR conformance + lmstudio
  probe write-ups; then exercised the binary across `--help`,
  `--version`, every subcommand, every documented error path, headless
  + single-prompt + TUI dispatch, ADR-referenced flags, and a set of
  CLI hygiene checks (broken pipe, `NO_COLOR`, invalid flags, missing
  values, leading-dash values, conflict detection).

## TL;DR

The binary is functionally a long way past v1 in scope — every Tier 1
parity row except a few long-tail surfaces is wired, headless mode
honors the ADR 0025 exit-code table, the supervisor daemon spawns and
shuts down cleanly, and the plugin/router/agents subcommands all work.

But there are four ship-blockers and ~17 smaller issues whose common
thread is **drift between the docs people read** (README, `--help` text,
runtime hints, ADRs) **and the code that actually runs.** None of the
blockers are subtle; the worst is a hard semver problem (`--version`
returns `0.0.0`), and the most consequential is the single-prompt CLI
silently exiting 0 on provider/hook/cancellation errors (an explicitly
deferred follow-up from the last lmstudio cleanup pass — still open).

## Findings, by severity

Each finding cites the file the change lives in. ADR conformance issues
quote the relevant ADR section.

### 🚨 BLOCKER — must fix before tagging v1

#### B1. `caliban --version` returns `caliban 0.0.0`
`Cargo.toml` lacks a `version` in `[workspace.package]`; `caliban/Cargo.toml:3`
hardcodes `version = "0.0.0"`. Set a real semver before tagging
(`0.1.0` if you want to leave headroom; `1.0.0` for a true 1.0). While
in there, fill `repository = TODO: set once the GitHub repo exists`
(`Cargo.toml:36`) — that comment will ship verbatim in `cargo metadata`
otherwise.

#### B2. README still references the old project name `iron-orrery`
`README.md:143`, inside the TUI mockup status bar:
`│ ~/dev/personal/iron-orrery · openai gpt-4o · session: research │`.
Per the memory note "previous names `iron-orrery`, `orrery-*`, and
`iroy` are obsolete — do not use." Replace with `~/dev/personal/caliban`
(or strip to just `caliban`).

#### B3. Single-prompt CLI driver exits 0 on every non-`EndOfTurn` stop
Verified live: `--max-turns 0 … "test"` prints
`[caliban: max-turns (0) reached]` and exits **0** (headless exits 130).
Provider error: prints `[caliban: provider error: …]` and exits **0**
(headless exits 1). Same for `HookDenied`, `CompactionFailed`,
`Cancelled`, `Refusal`, etc.

This is explicitly the deferral called out at the top of
`docs/2026-05-25-lmstudio-probe-findings.md` ("the single-prompt CLI
still exits `0` for non-`EndOfTurn` stop conditions … not blocking this
branch"). It IS blocking v1 — any script, hook, CI runner, or shell
chain (`caliban "fix it" && deploy`) cannot tell a successful run from
a failed one. The visibility fix (commit `b274e1d`) added the stderr
surface lines but left the exit code at 0.

Fix lives in `caliban/src/startup.rs`: `run_and_render` should return
the `StopCondition` to `main`, and `run_single_prompt` should map it to
the same `sysexits.h` codes `caliban/src/headless/mod.rs::exit_code_for`
uses (130 / 137 / 124 / 1 / 2 etc.). The mapping logic is already in
`stopped_for_surface_line` (`startup.rs:340-358`); just lift it.

#### B4. `caliban plugin` subcommand invisible to `--help`
The dispatcher at `caliban/src/main.rs:45-49` short-circuits on
`argv[1] == "plugin"` *before* clap parses anything, so the subcommand
never appears in:

```
$ caliban --help
…
Commands:
  router   …
  agents   …
  daemon   …
  attach   …  (etc — `plugin` missing)
```

`plugin` IS implemented (`plugin_cli.rs`, ~345 LOC, full
`install`/`list`/`info`/`enable`/`disable`/`remove`/`update` surface),
but operators have no path from `caliban --help` to learn it exists.
Either promote it to a real `clap::Subcommand` variant (preferred — gets
free `--help`, completion, and conflict detection) or, at minimum, mention
it in the binary's `about` string and add a stub `Plugin` variant whose
help redirects to `caliban plugin --help`.

### 🔴 HIGH — fix before v1 or accept as known caveat

#### H1. `caliban plugin enable/disable` is still the env-var stub
`plugin_cli.rs:314-344` prints "set `CALIBAN_ENABLED_PLUGINS=foo` to
enable plugin 'foo'" and `(settings.json keys land with ADR 0026; until
then env-var only)`. ADR 0026 has been `accepted` for two days and the
unified `caliban-settings` is wired in `caliban/src/main.rs:94`. The
v1-stub message is stale and the command does not do what its name
implies. Should patch `Settings.plugins.enabled` in the **project** or
**user** scope.

#### H2. Invalid `--settings <FILE_OR_JSON>` silently falls back to defaults
`caliban/src/main.rs:94-104` wraps `load_layered_settings` in `match …
Err(e) => { tracing::warn!(…); Settings::default() }`. So
`caliban --settings '{not_json' "test"` logs a warning and continues
with empty settings. ADR 0025's exit-code table reserves 78
(`EX_CONFIGURATION_ERROR`) precisely for "settings parse failure". The
overlay parse error needs to abort startup with exit 78.

#### H3. `--setting-sources "bogus,scope"` silently accepted
No validator on the CSV; unknown tokens are dropped on the floor. Misleads
the operator into thinking they pinned a scope when they didn't. Add
a validator that errors on tokens not in
`{managed, user, project, local}` (same exit 78).

#### H4. `--workspace /missing/dir` not validated at startup
`caliban/src/main.rs:86-89` calls `WorkspaceRoot::new(p.clone())`
unconditionally; the missing-directory check defers to first tool call.
Validate at parse: `std::fs::metadata(&p).is_dir()`; if false, error
with exit 64 (`EX_USAGE`) and a "directory does not exist or is not
readable" message.

#### H5. `--resume <missing>` behaves differently in headless vs single-prompt
- **Headless:** `[caliban] no session named 'X' to resume` → exit **66**.
  Correct per `headless/mod.rs`.
- **Single-prompt:** silently treats it as a *new* session named `X`
  and proceeds. The two paths should resolve identically; the headless
  semantics is the correct one (operators wrote `--resume`, not `--session`).

Fix path: `startup::resolve_session` should distinguish "I asked for an
existing session, it was missing" from "create me a fresh one." The
session loader already has this signal — `caliban/src/headless/session_loader.rs`
returns the typed `ResumeNotFound` error; the single-prompt path
ignores it.

#### H6. `agents attach <id>` and top-level `attach <id>` leak Rust `Debug` formatting
Output for `caliban agents attach nonexistent-id`:
```
caliban: unexpected reply: Error { error: NotFound { id: "nonexistent-id" } }
```
That's `format!("{:?}", reply)` reaching the user. Sister commands
(`kill`, `stop`, `rm`) normalize to `caliban: daemon: agent not found:
nonexistent-id` — the `attach` path forgot the `Display` mapping. Fix
in `caliban/src/agents_cli.rs` (the attach handler).

#### H7. Three different error spellings for "agent not found"
Sampled by giving each verb a bogus ID:

| Verb | Output |
|---|---|
| `kill` / `stop` / `rm` | `caliban: daemon: agent not found: nonexistent-id` |
| `attach` | `caliban: unexpected reply: Error { error: NotFound { id: "nonexistent-id" } }` |
| `logs` | `caliban: agent nonexistent-id not found` |

Pick one canonical phrasing (`caliban: agent not found: <id>` is the
cleanest) and apply it across every supervisor verb.

#### H8. Multiple stale "until ADR X lands" doc strings shipped in `--help` and runtime hints
Each of these is either out-of-date code (the feature shipped and the
warning is wrong) or out-of-date docs (the feature was renamed and
the help string didn't update):

| Surface | String | Reality |
|---|---|---|
| `--max-budget-usd` help (`args.rs:78`) | "Placeholder enforcement until ADR 0033 wires real cost" | ADR 0033 is ✅ shipped; `caliban-telemetry::pricing` is real |
| `--max-budget-usd` runtime warn (`headless/budget.rs` area) | `[caliban] --max-budget-usd is in placeholder mode: every request contributes 0.0 USD until ADR 0033 wires real pricing` | Same — should fire only for unknown (provider, model) pairs |
| `--fallback-model` help (`args.rs:114`) | "Router v2 wires this end-to-end; v1 records and surfaces it in init frames" | ADR 0038 ✅; router v2 *is* wired |
| `--permission-prompt-tool` help (`args.rs:120`) | "Parsed for forward-compat; MCP elicitation lands with Phase C (ADR 0023)" | Phase C ✅ per matrix |
| `--no-hooks` help (`args.rs:272-275`) | "Mirrors the `disable_all_hooks` field in `hooks.toml`" | Legacy filename; settings.json owns this now |
| `--no-mcp` help (`args.rs:212`) | "skip loading `mcp.toml`" | Same — settings.json owns this; mcp.toml is the compat shim |
| `plugin enable/disable` hint | "(settings.json keys land with ADR 0026; until then env-var only)" | ADR 0026 ✅ |
| `events.rs:226` doc comment on `total_cost_usd` | "0.0 until OTel/cost lands per ADR 0033" | ADR 0033 ✅ |

These are all 5-minute single-line patches; they ship as part of the
user's first impression of the binary.

#### H9. No auto-headless when stdout is piped, no `--no-auto-print`
ADR 0025's Decision section: *"Auto-headless when stdin is non-TTY or
stdout is piped, unless `--no-auto-print` is explicit. Explicit
`--print` always wins."*

Reality (`caliban/src/main.rs:286`):
```rust
let headless_active = args.print.is_some() || args.output_format.is_some();
```
There is no `--no-auto-print` flag at all (verified by grep), and stdout
being piped doesn't trigger headless. `caliban "do X" | tee log.txt`
currently runs the *interactive* single-prompt driver and writes
ANSI-escaped tool announcements through the pipe.

#### H10. `caliban config` subcommand does not exist
ADR 0026's text repeatedly references `caliban config migrate` and
`caliban config print`. Neither is implemented (no `Config` variant in
`CalibanCommand`). For v1 this can ship as a documented gap, but the
ADR should be amended either way so future contributors don't waste
time looking for the command.

### 🟡 MEDIUM — polish

#### M1. Error messages expose internal field names and double-print
- `--max-tokens 0` → `Error: agent misconfigured: Agent::max_tokens
  must be > 0`. The user typed `--max-tokens`; the error should say
  `--max-tokens` not the internal struct field.
- Missing API key → `Error: ANTHROPIC_API_KEY missing\n\nCaused by:\n
  missing config field: ANTHROPIC_API_KEY` — the "Caused by" line is
  a verbatim rephrase of the top line. Add `.context()` more carefully
  in `caliban-provider-anthropic::config::DirectConfig::from_env`.
- Missing API key also gives no remediation hint. Compare to `gh` or
  `aws`: "set `ANTHROPIC_API_KEY` env, configure
  `settings.json:provider.anthropic.api_key`, or wire `apiKeyHelper`."

#### M2. `NO_COLOR` is not honored
No reference to `NO_COLOR` anywhere in `caliban/src/`. The
`run_and_render` single-prompt driver emits `\x1b[2m...\x1b[0m` for
thinking deltas unconditionally (`startup.rs:240`); tool emoji `\u{1f527}`
likewise. https://no-color.org/ has been a de-facto CLI standard for
several years. Cheap fix: gate ANSI escapes on `std::env::var("NO_COLOR").is_err() && std::io::stderr().is_terminal()`.

#### M3. `--temperature 999` silently accepted
No `value_parser` clamp; the bogus value rides through to the provider
where it'll error mid-stream. Clamp to `[0.0, 2.0]` (or whatever your
maximum supported provider takes) at parse time.

#### M4. `--temperature -1` parse failure isn't user-friendly
clap rejects it as "unexpected argument '-1'", suggesting `-- -1`. If
temperatures are always non-negative this is fine; the `value_parser`
clamp from M3 would make the error message clearer.

#### M5. Anyhow `Caused by:` chains are verbose
Multiple places print a 3-line "Error: X / Caused by: 0: Y / 1: Z"
block when Y and Z are close paraphrases of each other (the
`--config` TOML parse error in particular prints the same parse error
twice). Audit `.context()` calls and prefer one good top-line message
over a chain.

#### M6. `caliban daemon status` auto-spawns the daemon
Running `caliban daemon status` cold reports `uptime_secs=1` — the
query itself started the daemon. Either rename to `daemon start +
status`, or have `status` print "daemon not running" instead of
implicitly starting one. Side effects on a "status" verb violate POLA.

#### M7. `-p ""` (empty `--print` value) does not error
`--print` has `num_args = 0..=1, default_missing_value = ""` (`args.rs:61`).
`caliban -p ""` with no stdin proceeds with an empty user message to
the provider. Either treat empty as "no prompt → consult stdin/positional",
or error with "prompt cannot be empty" at exit 64.

#### M8. `--workspace` worktree path leaks into init frame `cwd`
When run inside `.claude/worktrees/qa-v1-binary`, the init frame's
`cwd` field is the worktree absolute path, not the workspace root the
user thinks they're operating against. Probably fine, but worth a
README note. ADR 0025 doesn't pin a specific semantic.

#### M9. `caliban router debug` requires no caliban.toml to produce a useful diagnostic
Today: `caliban router debug` → `Error: no caliban.toml found (router
unconfigured)` exit 1. The point of a debug subcommand is "show me
what the router would do" — when there is no router, the message
should explain the synthetic-router fallback (and maybe still print
"default chain: anthropic/claude-3-5-sonnet"), not just error out.

### 🟢 POLISH — cosmetic

#### P1. `--help` doesn't visually group headless-only vs always-active flags
The flag list runs to 60+ entries. Headless-only flags
(`--output-format`, `--input-format`, `--include-partial-messages`,
`--include-hook-events`, `--replay-user-messages`, `--max-budget-usd`,
`--json-schema`, `--bare`) are interspersed with always-active flags.
clap's `help_heading` attribute lets you group them under a
`Headless options:` block — cheap, much friendlier.

#### P2. `--debug` help omits the `CALIBAN_DEBUG` env equivalent
README documents `CALIBAN_DEBUG=1`; `args.rs:184` does not. Most flags
that accept an env var declare it via clap's `env = "…"`; `--debug`
should too. (`startup::init_debug_tracing` checks the env var
directly.)

#### P3. Init frame's `plugins` field is hardcoded `Vec::new()`
`events.rs:263` always emits `plugins: Vec::new()` regardless of what
the plugin manager actually loaded. The matrix says ADR 0030 is ✅,
but the init frame's `plugins` array never gets populated.

#### P4. `caliban doctor` doesn't see global skills
"0 skill(s) loaded" despite `~/.claude/skills/` containing several.
`diagnostics::check_skills` calls `caliban_skills::default_roots(workspace)`
which appears to only walk workspace-local roots. Either expand the
check, or rename the hint to "0 workspace-local skill(s) loaded".

#### P5. Init frame uses mixed `camelCase` (`settingSources`) + `snake_case` (everything else)
This is intentional per `events.rs:7-10` ("Fields the ADR explicitly
names in camelCase … stay camelCase") for Claude Code parity. Worth
documenting in the headless mode README section so downstream JSON
consumers aren't surprised.

#### P6. `--bg` short-form has no log line
`caliban --bg "task"` prints `backgrounded as <id> (socket: …)` and
exits — fine. But if the supervisor daemon auto-spawn fails halfway,
the user has no way to tell. Consider emitting an info line in
verbose / debug mode.

#### P7. README says `~/.local/share/caliban/sessions/<name>.json`, doctor says `~/Library/Application Support/caliban/sessions`
Both are correct on their respective platforms (XDG vs macOS), but
the README's path is Linux-specific without saying so. Add the per-OS
note.

## What looks healthy

For balance — the binary IS in good shape on:

- **Headless mode exit codes:** all of 64 (parse), 66 (no input), 78
  (config), 124 (cancel), 130 (max-turns), 137 (budget) verified
  matching the ADR 0025 table.
- **Stream-json frame structure:** init + content + result frames all
  present, well-formed, terminate cleanly on every error path tested,
  one `system/init` per run (lmstudio Finding 8 regression test
  still holds).
- **clap validation surface:** invalid flag values (`--output-format yaml`,
  `--permission-mode bogusMode`), conflicting flags (`--system` + `--no-system`),
  and missing required values all error at exit 2 with the expected
  clap UX.
- **Permission-mode startup gate:** `bypassPermissions` without
  `--allow-dangerously-skip-permissions` aborts cleanly (CLI and env);
  unknown mode names error clearly. ADR 0029 conformance verified.
- **Supervisor/agents flow:** `caliban --bg "task"` spawns, `caliban
  agents list` shows it, `caliban agents kill <id>` + `rm <id> --force`
  cleans it, `caliban daemon stop` shuts down. End-to-end works.
- **`caliban doctor`** runs in <1s, hits every documented check, exits
  0 cleanly when no failures.

## Recommended v1 fix order

1. **B1 + B2** — semver + README rename. Both are one-line edits and
   immediately make the release tagging meaningful.
2. **B3** — single-prompt exit-code mapping. Reuses logic already in
   `startup::stopped_for_surface_line` and `headless::exit_code_for`.
   Probably <50 lines of glue. Highest leverage for CI/scripting users.
3. **B4** — promote `plugin` to a clap subcommand or at minimum add
   a hint to `--help`. Discoverability today is broken.
4. **H8** — sweep the stale "until ADR X lands" doc strings. Each is
   trivial and they collectively shape the user's first 60 seconds of
   help-reading.
5. **H1, H6, H7** — the agent-CLI error hygiene cluster. Same
   sub-system, can be one PR.
6. **H2 + H3 + H4** — input validation cluster. Eight-line patches that
   convert silent-degrade into clean error+exit-78 / 64.
7. Everything in M / P can wait for v1.1 unless something is blocking
   you personally.

Verified-during-probe details (commands, exit codes, env, flag combos)
are interleaved above; ask for a transcript if you want to regenerate
the run book.
