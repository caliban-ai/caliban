# Print Mode

Print mode is caliban's non-interactive entry point. Instead of launching the TUI, it drives the agent to completion and writes results to stdout — making caliban scriptable from a shell, a CI job, or any program that can invoke a subprocess.

## Activating print mode

| Method | Example |
|--------|---------|
| `-p` / `--print` flag | `caliban -p "summarize this repo"` |
| `--output-format` flag | `caliban --output-format json "fix the bug"` |
| Auto-headless | caliban detects a piped stdout or non-TTY stdin and enters print mode automatically |

Auto-headless fires when a prompt is given **and** stdout is piped **or** stdin is not a TTY. Pass `--no-auto-print` to suppress this inference and keep the TUI even in piped contexts.

## Choosing an output format

```bash
--output-format text|json|stream-json
```

| Format | Output |
|--------|--------|
| `text` | The assistant's final reply, streamed to stdout as plain text. Default. |
| `json` | A single JSON object identical to the `result` frame in `stream-json`. Useful for `jq` consumers that only need the final answer and cost totals. |
| `stream-json` | Newline-delimited JSON (NDJSON). One frame per event — `system/init` first, per-turn tool and message frames, `result` last. The full automation contract; see [The stream-json Protocol](./stream-json.md). |

## Supplying input

By default caliban reads the prompt from the positional argument or `--prompt`. To pipe multi-line input, pass `-` as the prompt value and write to stdin:

```bash
git diff HEAD | caliban -p - "review these changes"
```

For multi-turn scripted sessions use `--input-format stream-json` to send NDJSON `user` frames on stdin instead. When this flag is active, a non-`-` inline prompt is rejected at startup (exit 64) to prevent accidentally bypassing the frame parser. See [The stream-json Protocol](./stream-json.md) for details.

## Budget guard

```bash
--max-budget-usd <USD>
```

Caps the cumulative spend for a run. Cost is tracked against the vendored rate card in `caliban-telemetry`. When the budget is exceeded after a turn completes, caliban emits a `result` frame with `subtype: "budget_exceeded"` and exits **137**. Unknown `(provider, model)` pairs contribute `$0.00` and emit a single warning — the run is not blocked.

## Deterministic runs with `--bare`

```bash
caliban --bare -p "count lines of code"
```

`--bare` skips hooks, skills, plugins, MCP server discovery, auto-memory, and `CLAUDE.md` walk-up. The agent runs with only its built-in tools and the flags you supply. Use it when you need a fully reproducible run that ignores user and project settings.

```admonish tip title="bare vs. --no-auto-print"
`--bare` controls *what the agent loads*; `--no-auto-print` controls *whether headless mode fires automatically*. They are independent.
```

## Exit codes

| Code | Condition |
|------|-----------|
| 0 | Success |
| 1 | Generic runtime error (provider error, hook denial, tool crash) |
| 2 | Schema validation failed (`--json-schema`) |
| 64 | Bad flags / malformed stream-json input (`EX_USAGE`) |
| 66 | `--resume <name>` not found, or empty stream-json stdin (`EX_NOINPUT`) |
| 75 | `--max-turns` exceeded (`EX_TEMPFAIL`) |
| 78 | Config error — settings parse failure, stdin > 10 MB (`EX_CONFIG`) |
| 124 | Cancelled (Ctrl-C / SIGTERM from the agent loop) |
| 130 | Real SIGINT — second Ctrl-C reaching the harness |
| 137 | `--max-budget-usd` exceeded |

CI scripts can distinguish budget exhaustion from genuine failures without parsing stdout: `$?` carries the signal.

## Session persistence

Print-mode runs honour `--session <NAME>`, `--continue` (`-c`), and `--resume` the same way as interactive sessions. Pass `--no-save` to skip writing the session back to disk after the run.

## Related pages

- [The stream-json Protocol](./stream-json.md) — detailed frame reference
- [Structured Output](./structured-output.md) — `--json-schema` for schema-conformant replies
- [CI Patterns](./ci.md) — complete recipes for GitHub Actions and other pipelines
