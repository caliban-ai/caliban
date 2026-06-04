# CI Patterns

This page puts the headless flags together into complete, copyable recipes for GitHub Actions and other CI environments. Before reading further, familiarise yourself with [Print Mode](./print-mode.md), [The stream-json Protocol](./stream-json.md), and [Headless & Audit](../permissions/headless-and-audit.md).

## Key flags for CI

| Flag | Purpose |
|------|---------|
| `--bare` | Skip hooks, skills, plugins, MCP, auto-memory, CLAUDE.md. Deterministic — output depends only on what you pass. |
| `--max-budget-usd <USD>` | Hard spend cap; exit 137 if exceeded. Prevents runaway costs in long jobs. |
| `--permission-mode acceptEdits` | Allow file edits without prompting; still denies shell commands the rules don't cover. |
| `--allow <PAT>` | Add an Allow rule at top priority for this invocation only (repeatable). |
| `--no-save` | Don't write the session to disk — keeps CI agents stateless. |
| `--output-format stream-json` | Full NDJSON output for structured parsing. |
| `--output-format json` | Single JSON result object — simpler for scripts that only need the answer and exit code. |

Exit codes are the primary success signal. See [Print Mode — Exit codes](./print-mode.md#exit-codes) for the full table. `$? == 0` means success; `$? == 137` means the budget cap fired.

## Recipe 1 — Simple text answer in GitHub Actions

Suitable for jobs that just need a freeform answer and check the exit code.

```yaml
# .github/workflows/caliban-check.yml
name: caliban check

on: [push]

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Run caliban review
        env:
          ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}
        run: |
          caliban \
            --bare \
            --max-budget-usd 0.50 \
            --permission-mode acceptEdits \
            --no-save \
            -p "Review the diff for obvious bugs and print a one-sentence verdict."
```

The job fails if caliban exits non-zero (runtime error, budget exceeded, etc.).

## Recipe 2 — Structured output with `jq` parsing

Use `--output-format json` and `--json-schema` when you need machine-readable output — for example, a gate that checks whether a review verdict is "pass" or "fail".

```bash
#!/usr/bin/env bash
# ci/review-gate.sh
set -euo pipefail

RESULT=$(caliban \
  --output-format json \
  --json-schema '{"type":"object","required":["verdict","reason"]}' \
  --bare \
  --max-budget-usd 1.00 \
  --permission-mode acceptEdits \
  --allow "Bash:git diff*" \
  --allow "Read" \
  --no-save \
  -p "Review the staged changes. Reply ONLY with JSON: {\"verdict\": \"pass\"|\"fail\", \"reason\": \"<one sentence>\"}")

echo "Raw result: $RESULT"
VERDICT=$(echo "$RESULT" | jq -r '.structured_output.verdict')

if [ "$VERDICT" = "pass" ]; then
  echo "Review passed: $(echo "$RESULT" | jq -r '.structured_output.reason')"
  exit 0
else
  echo "Review failed: $(echo "$RESULT" | jq -r '.structured_output.reason')"
  exit 1
fi
```

```admonish tip title="Exit code vs. structured output"
Check `$?` first: if caliban exits non-zero before emitting a result frame (bad flags, budget blown before any turn, etc.) `jq` will fail on empty input. A pattern like `RESULT=$(caliban … || true)` followed by a `$?` check is more robust.
```

## Recipe 3 — Multi-turn stream-json pipeline

For jobs that drive several agent turns or need to observe tool calls in real time, parse the NDJSON stream line by line.

```bash
#!/usr/bin/env bash
# ci/stream-pipeline.sh
set -euo pipefail

TASKS=$(cat <<'EOF'
{"type":"user","content":"Run the test suite and report any failures."}
{"type":"user","content":"If any tests failed, suggest a fix."}
EOF
)

LAST_RESULT=""
TOOL_CALLS=0

while IFS= read -r line; do
  [ -z "$line" ] && continue
  TYPE=$(echo "$line" | jq -r '.type')
  case "$TYPE" in
    system)
      SUBTYPE=$(echo "$line" | jq -r '.subtype')
      if [ "$SUBTYPE" = "init" ]; then
        echo "[init] model=$(echo "$line" | jq -r '.model')"
      fi
      ;;
    tool_use)
      TOOL_CALLS=$((TOOL_CALLS + 1))
      echo "[tool] $(echo "$line" | jq -r '.name')"
      ;;
    result)
      LAST_RESULT="$line"
      SUBTYPE=$(echo "$line" | jq -r '.subtype')
      COST=$(echo "$line" | jq -r '.total_cost_usd')
      echo "[result] subtype=$SUBTYPE cost=\$$COST tool_calls=$TOOL_CALLS"
      ;;
  esac
done < <(echo "$TASKS" | caliban \
    --output-format stream-json \
    --input-format stream-json \
    --bare \
    --max-budget-usd 2.00 \
    --permission-mode acceptEdits \
    --allow "Bash:cargo test*" \
    --no-save)

# Final check
EXIT_CODE=$?
SUBTYPE=$(echo "$LAST_RESULT" | jq -r '.subtype')
if [ "$EXIT_CODE" -ne 0 ] || [ "$SUBTYPE" != "success" ]; then
  echo "Run did not succeed (exit=$EXIT_CODE subtype=$SUBTYPE)"
  exit 1
fi
```

## Permissions in headless mode

By default, caliban inherits all user and project permission rules in headless mode, just as in interactive sessions. For CI you typically want tighter control:

- `--permission-mode acceptEdits` — auto-allows file edits; still asks (or denies) for shell commands not covered by a rule.
- `--allow "Bash:git *"` — add a top-priority Allow rule for specific shell patterns.
- `--deny "Bash:rm -rf*"` — add a top-priority Deny rule.
- `--bare` — skip all settings-derived rules (only built-in defaults apply).

For a full discussion of permission modes and how they interact with headless runs, see [Headless & Audit](../permissions/headless-and-audit.md).

```admonish warning title="Never use --allow-dangerously-skip-permissions in CI"
`bypassPermissions` mode disables all permission gating. In a CI context this means an adversarially-crafted prompt or tool output could instruct caliban to delete files, exfiltrate secrets, or make network calls. Use `acceptEdits` instead and add explicit `--allow` rules for the shell patterns your job actually needs.
```

## Parsing exit codes in shell

```bash
caliban --bare --max-budget-usd 0.20 -p "summarize the diff" || {
  CODE=$?
  case $CODE in
    75)  echo "Max turns exceeded";;
    137) echo "Budget cap hit";;
    124) echo "Cancelled";;
    *)   echo "Error: exit $CODE";;
  esac
  exit $CODE
}
```

## Related pages

- [Print Mode](./print-mode.md)
- [The stream-json Protocol](./stream-json.md)
- [Structured Output](./structured-output.md)
- [Headless & Audit](../permissions/headless-and-audit.md)
