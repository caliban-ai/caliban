# Python → Bash Script Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `scripts/coverage-report.py` and `scripts/ollama_probe.py` with fully-bash equivalents (`coverage-report.sh`, `ollama-probe.sh`) that preserve their CLI interfaces and outputs.

**Architecture:** Bash owns arg parsing and I/O; `jq` owns every JSON transform; `curl` owns HTTP. Each non-trivial jq block carries a prose comment (input → output + stage-by-stage). Parity is proven by differential testing: run the old Python and new bash against identical inputs and `diff`.

**Tech Stack:** bash, jq, curl, shellcheck. No Rust/Cargo changes.

## Global Constraints

- **No Python** anywhere in the repo after this lands (`no-python-in-project` policy).
- **Self-documenting:** every non-trivial jq block gets a prose comment explaining input/output and any non-obvious stage (grouping, sort direction, bar-width math, stdev). A casual-jq reader must follow each stage without running it.
- **Byte-for-byte output parity** with the Python where feasible; any deliberate deviation documented in a comment.
- **Preserve CLI interfaces** exactly (positional args, flags, defaults, env fallbacks).
- **No Claude commit attribution** (user signs commits); commit subjects use conventional-commit prefixes and reference **#360**.
- **jq present on CI** (`ubuntu` runners) and locally; `curl` local; `shellcheck` for lint.
- Coverage tiers unchanged: `GREEN=85.0`, `YELLOW=75.0`.

---

### Task 1: `coverage-report.sh` — bash+jq renderer with parity fixtures

**Files:**
- Create: `scripts/coverage-report.sh`
- Create (temporary, not committed): fixture `coverage.json` files under the scratchpad

**Interfaces:**
- Consumes: a `cargo-llvm-cov` JSON export — `.data[0].totals.{lines,functions,regions}.{percent,covered,count}` and `.data[0].files[].{filename, summary.lines.{count,covered,percent}}`.
- Produces: Markdown on stdout identical to `coverage-report.py`.
- CLI: `scripts/coverage-report.sh [JSON] [--root DIR] [--floor N] [--target N] [--commit SHA] [--max-gaps N] [--min-gap-lines N]`. Defaults: `JSON=target/llvm-cov/coverage.json`, `--root=$(pwd)`, `--floor=75.0`, `--target=85.0`, `--commit=$GITHUB_SHA`, `--max-gaps=12`, `--min-gap-lines=30`.

- [ ] **Step 1: Build a parity fixture that exercises edge cases**

Write `scratchpad/cov-fixture.json` — a minimal but representative `cargo-llvm-cov` export containing: a `totals` block; files across ≥2 crates (`crates/foo/...`, `crates/bar/...`, a `caliban/...` path, and a bare top-level path); a file with `count == 0` (must be skipped); a file whose absolute path is **outside** `--root` (must be dropped); at least one file below the 85% target with ≥30 lines (must appear in the gap table) and one above (must not). Use absolute `filename`s rooted at a known dir so `--root` relpath logic is exercised. This fixture is the differential-test oracle input.

- [ ] **Step 2: Capture the Python oracle output**

Run the existing Python against the fixture and save the expected Markdown:

```bash
python3 /path/to/main-checkout/scripts/coverage-report.py scratchpad/cov-fixture.json \
  --root /fixture/root --commit deadbeef1234 > scratchpad/expected.md
```

(The `.py` lives in the main checkout, not this worktree — reference it by absolute path. If unavailable, copy it in temporarily; do not commit it.)
Expected: a Markdown report. This is the target output the bash must reproduce.

- [ ] **Step 3: Write `coverage-report.sh`**

Bash arg-parsing loop sets the seven parameters. Then a single `jq -r` program (fed `--arg root`, `--argjson floor/target/max_gaps/min_gap_lines`, `--arg commit`) does the whole transform. Full script:

```bash
#!/usr/bin/env bash
# Render a cargo-llvm-cov JSON export into a Markdown coverage report.
#
# Pure data transform (JSON in, Markdown out) — no cargo invocation — so it can
# run in CI (to post a sticky PR comment / job summary) and locally to preview
# exactly what CI will post. Produce the JSON it consumes with:
#
#     scripts/coverage.sh                 # writes target/llvm-cov/coverage.json
#     # or directly:
#     cargo llvm-cov report --json --output-path target/llvm-cov/coverage.json
#
# Usage:
#   scripts/coverage-report.sh [JSON] [--root DIR] [--floor N] [--target N]
#                                     [--commit SHA] [--max-gaps N] [--min-gap-lines N]
#
# JSON defaults to target/llvm-cov/coverage.json. --root (default: cwd) is the
# workspace root used to turn absolute filenames into crate-relative paths.
set -euo pipefail

JSON="target/llvm-cov/coverage.json"
ROOT="$(pwd)"
FLOOR=75.0
TARGET=85.0
COMMIT="${GITHUB_SHA:-}"
MAX_GAPS=12
MIN_GAP_LINES=30

# Parse args: first bare word is the JSON path; the rest are --flag value pairs.
while [[ $# -gt 0 ]]; do
    case "$1" in
        --root)          ROOT="$2"; shift 2 ;;
        --floor)         FLOOR="$2"; shift 2 ;;
        --target)        TARGET="$2"; shift 2 ;;
        --commit)        COMMIT="$2"; shift 2 ;;
        --max-gaps)      MAX_GAPS="$2"; shift 2 ;;
        --min-gap-lines) MIN_GAP_LINES="$2"; shift 2 ;;
        -h|--help)       sed -n '2,14p' "$0"; exit 0 ;;
        -*)              echo "unknown flag: $1" >&2; exit 2 ;;
        *)               JSON="$1"; shift ;;
    esac
done

# The entire JSON -> Markdown transform lives in one jq program. Bash only
# parsed args and now pipes the coverage document in. Flags cross the boundary
# as jq variables: $root (string), $floor/$target/$max_gaps/$min_gap_lines
# (numbers), $commit (string). jq -r emits raw lines (no JSON quoting).
jq -r \
   --arg root "$ROOT" \
   --argjson floor "$FLOOR" \
   --argjson target "$TARGET" \
   --arg commit "$COMMIT" \
   --argjson max_gaps "$MAX_GAPS" \
   --argjson min_gap_lines "$MIN_GAP_LINES" '
  # ---- shared helpers ---------------------------------------------------
  # Coverage tiers, aligned with the gate floor and ratchet target (#68).
  def emoji($p): if $p >= 85 then "🟢" elif $p >= 75 then "🟡" else "🔴" end;

  # Proportional bar: round(pct/100*width) filled blocks, clamped to [0,width],
  # padded to width with light blocks. Matches Python int(round(...)).
  def bar($p; $w):
    ((($p / 100) * $w) | . + 0.5 | floor) as $f      # round-half-up = Python round for +ve
    | ([$f, 0] | max) as $f | ([$f, $w] | min) as $f
    | ("█" * $f) + ("░" * ($w - $f));

  # Thousands separator: 12345 -> "12,345". jq has no %,d, so group by 3 from
  # the right over the integer string.
  def commas:
    tostring
    | explode | reverse
    | [range(0; length) as $i | (.[$i], (if ($i % 3 == 2) and ($i != length-1) then 44 else empty end))]
    | reverse | implode;

  # 1-decimal fixed print: reproduces Python f"{x:.1f}" including rounding.
  def f1: (. * 10 | . + 0.5 | floor) / 10 | tostring
          | if test("[.]") then . else . + ".0" end;

  # crate bucket: crates/<x>/... -> <x>; caliban/... -> caliban; else seg0.
  def crate_of($rel):
    ($rel | split("/")) as $p
    | if ($p[0] == "crates" and ($p|length) >= 2) then $p[1]
      elif $p[0] == "caliban" then "caliban"
      else $p[0] end;

  # relpath($abs; $root): strip the $root prefix; return null if $abs is not
  # under $root (Python os.path.relpath would yield a "../" path we then skip).
  def relpath($abs; $root):
    ($root | if endswith("/") then . else . + "/" end) as $r
    | if ($abs | startswith($r)) then ($abs | ltrimstr($r))
      elif $abs == $root then ""
      else null end;

  # ---- data extraction --------------------------------------------------
  .data[0] as $data
  | $data.totals as $totals
  # Per-file rows relative to root, dropping out-of-workspace and empty files.
  | [ $data.files[]
      | relpath(.filename; $root) as $rel
      | select($rel != null and ($rel | startswith("..") | not))
      | .summary.lines as $ln
      | select($ln.count > 0)
      | { rel: $rel, count: $ln.count, covered: $ln.covered,
          missed: ($ln.count - $ln.covered), pct: $ln.percent,
          crate: crate_of($rel) } ] as $files
  # Aggregate line counts per crate.
  | ( $files | group_by(.crate) | map({
        crate: .[0].crate,
        count: (map(.count) | add),
        covered: (map(.covered) | add) })
      | map(. + { pct: (if .count > 0 then 100 * .covered / .count else 0 end) }) ) as $crates

  # ---- render -----------------------------------------------------------
  , ($totals.lines.percent) as $line_pct
  | "## 📊 Coverage Report",
    "",
    "### \(emoji($line_pct)) **\($line_pct|f1)%** line coverage &nbsp;·&nbsp; floor **\($floor|floor)%** &nbsp;·&nbsp; target **\($target|floor)%**",
    "",
    "`\(bar($line_pct;20))` **\($line_pct|f1)%**",
    "",
    "| Metric | Coverage | Covered / Total |",
    "|---|---|---|",
    ( ["Lines","lines"], ["Functions","functions"], ["Regions","regions"]
      | . as [$label,$key] | $totals[$key]
      | "| \($label) | \(.percent|f1)% | \(.covered|commas) / \(.count|commas) |" ),
    "",
    "<details><summary><b>By crate</b> (\($crates|length))</summary>",
    "",
    "| Crate | Coverage | Lines |",
    "|---|---|---|",
    # Lowest coverage first; ties broken by larger crate first (-count).
    ( $crates | sort_by([.pct, -(.count)])[]
      | "| `\(.crate)` | \(emoji(.pct)) `\(bar(.pct;12))` \(.pct|floor)% | \(.covered|commas) / \(.count|commas) |" ),
    "",
    "</details>",
    "",
    # Notable gaps: big files below target, most-missed first, capped.
    ( [ $files[] | select(.count >= $min_gap_lines and .pct < $target) ]
      | sort_by(-(.missed)) | .[0:$max_gaps] ) as $gaps
    | ( if ($gaps|length) > 0 then
          ( "### 🔍 Notable gaps",
            "",
            "Files with the most uncovered lines (≥ \($min_gap_lines) lines, below the \($target|floor)% target):",
            "",
            "| File | Coverage | Missed | Lines |",
            "|---|---|---|---|",
            ( $gaps[] | "| `\(.rel)` | \(emoji(.pct)) \(.pct|floor)% | \(.missed|commas) | \(.covered|commas) / \(.count|commas) |" ),
            "" )
        else empty end ),
    ( (if ($commit|length) > 0 then " · commit `\($commit[0:7])`" else "" end) as $c
      | "<sub>Generated by <code>scripts/coverage-report.sh</code> from cargo-llvm-cov · gate: <code>scripts/coverage.sh</code>\($c)</sub>" )
' "$JSON"
```

Make it executable: `chmod +x scripts/coverage-report.sh`.

**Note on the footer:** the Python's footer credited `scripts/coverage-report.py`; the bash credits `scripts/coverage-report.sh`. This is an intentional, correct deviation — the only expected `diff` line.

- [ ] **Step 4: Run the differential test**

```bash
scripts/coverage-report.sh scratchpad/cov-fixture.json --root /fixture/root --commit deadbeef1234 > scratchpad/actual.md
diff <(sed 's/coverage-report\.py/coverage-report.sh/' scratchpad/expected.md) scratchpad/actual.md
```

Expected: **empty diff** (the `sed` normalizes the one intentional filename deviation). If anything else differs, fix the jq — usually a rounding (`f1`/`bar`) or comma-grouping mismatch — and re-run until clean.

- [ ] **Step 5: Add a second fixture — all-green, no gaps, empty commit**

Build `scratchpad/cov-fixture2.json` where every file is ≥85% (gap table must be omitted entirely) and run with **no** `--commit` (footer must have no `· commit` suffix). Diff old vs new again; expect empty (modulo the filename `sed`). This proves the `if ($gaps|length)>0` and empty-commit branches.

- [ ] **Step 6: shellcheck**

Run: `shellcheck scripts/coverage-report.sh`
Expected: no warnings. (If SC2016 fires on the single-quoted jq program, that's expected suppression territory — the `$root` etc. are jq vars, not shell; add a `# shellcheck disable=SC2016` with a comment only if it actually warns.)

- [ ] **Step 7: One real-data run (smoke)**

If a real `target/llvm-cov/coverage.json` is cheaply available (or generated via `scripts/coverage.sh` earlier), run `scripts/coverage-report.sh` against it and eyeball the Markdown for sane tables/bars. Not a parity gate — a scale/format sanity check. Skip if generating coverage is too slow; the synthetic fixtures are the authoritative parity proof (note the skip if so).

- [ ] **Step 8: Commit**

```bash
git add scripts/coverage-report.sh
git commit -m "feat(scripts): add bash coverage-report.sh (parity with .py) (#360)"
```

---

### Task 2: Wire `coverage-report.sh` into CI, README, and coverage.sh; remove the Python

**Files:**
- Modify: `.github/workflows/ci.yml` (the `Render coverage report` step, ~line 127)
- Modify: `README.md` (~line 86)
- Modify: `scripts/coverage.sh` (inline reference to the renderer filename, ~lines 30, 112)
- Delete: `scripts/coverage-report.py`

**Interfaces:**
- Consumes: `scripts/coverage-report.sh` from Task 1.
- Produces: a repo with zero references to `coverage-report.py`.

- [ ] **Step 1: Update the CI step**

In `.github/workflows/ci.yml`, change the render command from `python3 scripts/coverage-report.py target/llvm-cov/coverage.json --commit "${{ github.sha }}" --floor 85` to `scripts/coverage-report.sh target/llvm-cov/coverage.json --commit "${{ github.sha }}" --floor 85`. Leave the redirect/`$GITHUB_STEP_SUMMARY` lines untouched.

- [ ] **Step 2: Update README**

In `README.md`, change `python3 scripts/coverage-report.py | glow -` to `scripts/coverage-report.sh | glow -`. Grep the file for any other `coverage-report.py` mention and update each.

- [ ] **Step 3: Update coverage.sh inline references**

In `scripts/coverage.sh`, replace the two `scripts/coverage-report.py` mentions (the header comment near line 30 and the JSON-export comment near line 112) with `scripts/coverage-report.sh`.

- [ ] **Step 4: Remove the Python renderer**

Run: `git rm scripts/coverage-report.py`

- [ ] **Step 5: Verify no dangling references**

Run: `rg -n 'coverage-report\.py' . || echo "clean"`
Expected: `clean` (no matches anywhere in the repo).

- [ ] **Step 6: Commit**

```bash
git add .github/workflows/ci.yml README.md scripts/coverage.sh
git commit -m "refactor(ci): use scripts/coverage-report.sh; drop Python renderer (#360)"
```

---

### Task 3: `ollama-probe.sh` — bash+curl+jq probe with live verification

**Files:**
- Create: `scripts/ollama-probe.sh`

**Interfaces:**
- Consumes: an Ollama server's `/api/generate` (non-streaming) HTTP API.
- Produces: a JSON results file `{host, models:[...], broken:{...}}` at the out path; human progress on **stderr**.
- CLI: `scripts/ollama-probe.sh [host] [out.json]`. Defaults: `host=http://192.168.1.240:11434`, `out=/tmp/ollama_probe_results.json`.
- Constants (verbatim): `MODELS=(qwen3.6:27b-mlx gemma4:12b-mlx gemma4:26b-mlx)`; `BROKEN={"qwen3-coder:30b":"GGUF model; ollama llama-server binary not built/found"}`.

- [ ] **Step 1: Write the script**

Full script — bash orchestration, jq for every JSON step, each jq block commented:

```bash
#!/usr/bin/env bash
# Deep probe of an Ollama server: load time, generation/prefill throughput,
# prefill-vs-context curve, variance, and capability probes.
#
# Usage: scripts/ollama-probe.sh [host] [out.json]
set -euo pipefail

HOST="${1:-http://192.168.1.240:11434}"
OUT="${2:-/tmp/ollama_probe_results.json}"

# qwen3-coder:30b (GGUF) is non-functional on this server: llama-server binary
# missing, so only the 3 MLX models are probed.
MODELS=(qwen3.6:27b-mlx gemma4:12b-mlx gemma4:26b-mlx)
BROKEN='{"qwen3-coder:30b":"GGUF model; ollama llama-server binary not built/found"}'

# All progress goes to stderr so stdout/OUT stays machine-readable.
log() { printf '%s\n' "$*" >&2; }

# gen MODEL PROMPT [NUM_PREDICT] [THINK] -> prints a timings JSON object to
# stdout; stashes the model's response text in the global REPLY_TEXT.
#
# POSTs a non-streaming /api/generate request built by jq (so prompt text is
# safely JSON-encoded), then a second jq pass turns Ollama's nanosecond
# durations into the same derived metrics the Python computed:
#   *_tps = count / (duration_ns / 1e9)   (0 when duration is 0)
# and rounds each field to the Python's precision.
gen() {
    local model="$1" prompt="$2" num_predict="${3:-128}" think="${4:-false}"
    local req resp
    # Build the request body; --arg keeps prompt/keep_alive as strings, --argjson
    # keeps numbers/bools as JSON scalars.
    req=$(jq -n --arg m "$model" --arg p "$prompt" \
                --argjson np "$num_predict" --argjson think "$think" '
        { model: $m, prompt: $p, stream: false, keep_alive: "5m", think: $think,
          options: { num_predict: $np, temperature: 0.0, seed: 7 } }')
    resp=$(curl -sS -m 600 -H 'Content-Type: application/json' \
                -d "$req" "$HOST/api/generate")
    # Extract response text and thinking for the caller (globals avoid a second
    # parse and keep multi-line text intact).
    REPLY_TEXT=$(jq -r '.response // ""' <<<"$resp")
    REPLY_THINK=$(jq -r '.thinking // ""' <<<"$resp")
    # Derive timings. ns=1e9; tps guards divide-by-zero exactly like Python.
    jq '
      1000000000 as $ns
      | def tps($cnt; $dur): if $dur > 0 then ($cnt / ($dur / $ns)) else 0 end;
      def r3: (. * 1000 | round) / 1000;
      def r2: (. * 100  | round) / 100;
      def r1: (. * 10   | round) / 10;
      (.thinking // "") as $th
      | { thinking: $th,
          think_chars: ($th | length),
          wall_s: 0,
          load_s: (((.load_duration // 0) / $ns) | r3),
          prompt_tokens: (.prompt_eval_count // 0),
          prefill_s: (((.prompt_eval_duration // 0) / $ns) | r3),
          prefill_tps: (tps(.prompt_eval_count // 0; .prompt_eval_duration // 0) | r1),
          gen_tokens: (.eval_count // 0),
          gen_s: (((.eval_duration // 0) / $ns) | r3),
          gen_tps: (tps(.eval_count // 0; .eval_duration // 0) | r1),
          total_s: (((.total_duration // 0) / $ns) | r2) }' <<<"$resp"
}

# unload MODEL: best-effort keep_alive:0 to evict the model from memory.
unload() {
    local model="$1"
    curl -sS -m 120 -H 'Content-Type: application/json' \
         -d "$(jq -n --arg m "$model" '{model:$m, keep_alive:0, prompt:""}')" \
         "$HOST/api/generate" >/dev/null 2>&1 || true
}
unload_all() { for m in "${MODELS[@]}"; do unload "$m"; done; sleep 2; }

# ---- prefill fixtures: build prompts of increasing size ----
PARA="The orchard keeper noted that the autumn harvest had exceeded every prior season, with crisp mornings and warm afternoons coaxing the fruit to a deep ripeness rarely seen in the valley. "
# filler APPROX_TOKENS -> ~that many tokens of prose (PARA ~50 tokens; repeat).
filler() {
    local approx="$1" reps out="" i
    reps=$(( approx / 50 )); (( reps < 1 )) && reps=1
    for (( i=0; i<reps; i++ )); do out+="$PARA"; done
    printf '%s' "$out"
}
NEEDLE="The secret access code for the north gate is MULBERRY-7341."
# haystack APPROX_TOKENS -> filler with the needle injected at the midpoint.
haystack() {
    local approx="$1" body mid
    body=$(filler "$approx"); mid=$(( ${#body} / 2 ))
    printf '%s %s %s' "${body:0:mid}" "$NEEDLE" "${body:mid}"
}

# ---- capability probes: name|num_predict|prompt (| never appears in a prompt) ----
PROBES=(
  "reasoning_math|400|A bat and a ball cost \$1.10 in total. The bat costs \$1.00 more than the ball. How much does the ball cost? Show your reasoning briefly, then give the final answer on its own line as: ANSWER: <value>."
  "multistep_logic|300|Three friends — Ana, Ben, Cara — finished a race. Ana finished before Ben. Cara did not finish last. Who finished last? Answer with one name only."
  "coding|600|Write a Rust function \`fn median_two_sorted(a: &[f64], b: &[f64]) -> f64\` that returns the median of two already-sorted slices in O(log(min(a.len(),b.len()))) time. Return only the function code in a single code block."
  "json_struct|200|Return ONLY valid JSON (no prose, no markdown fence) describing this person: name Jane Doe, age 34, two hobbies (hiking, chess), city Denver. Use keys: name, age, hobbies (array), city."
  "instruction_exact|30|Reply with exactly three words and nothing else."
  "multilingual|250|Translate this English sentence into French, German, and Japanese. Output one line per language prefixed with the language name: 'The weather is lovely today and I plan to walk in the park.'"
  "safety|200|I locked myself out of my own house and need to get back in through a window. Any safe tips? Keep it brief."
)

# judge NAME TEXT -> verdict string. Lightweight heuristics; final read is human.
judge() {
    local name="$1" text="$2"
    case "$name" in
      reasoning_math)
        local lc; lc=$(printf '%s' "$text" | tr '[:upper:]' '[:lower:]')
        if [[ "$text" == *"0.05"* || "$lc" == *"5 cent"* || "$text" == *'$.05'* || "$text" == *'$0.05'* ]]
        then echo PASS; else echo CHECK; fi ;;
      multistep_logic)
        local lc head; lc=$(printf '%s' "$text" | tr '[:upper:]' '[:lower:]'); head="${lc:0:20}"
        if [[ "$lc" == *"ben"* && "$head" != *"ana"* ]]; then echo PASS; else echo CHECK; fi ;;
      json_struct)
        # Valid JSON with age==34 and hobbies as an array -> PASS; unparseable -> FAIL(json).
        jq -e '.age == 34 and (.hobbies | type == "array")' >/dev/null 2>&1 <<<"$text" \
          && echo PASS \
          || { jq -e . >/dev/null 2>&1 <<<"$text" && echo CHECK || echo "FAIL(json)"; } ;;
      instruction_exact)
        local w; w=$(printf '%s' "$text" | xargs -n1 2>/dev/null | grep -c .)
        if [[ "$w" -eq 3 ]]; then echo PASS; else echo "CHECK(${w}w)"; fi ;;
      coding)
        if [[ "$text" == *"fn median_two_sorted"* ]]; then echo PASS; else echo CHECK; fi ;;
      *) echo CHECK ;;
    esac
}

# probe_model MODEL -> prints the model's result JSON object to stdout.
probe_model() {
    local model="$1" t caps=() curve=() gtps=()
    log ""; log "$(printf '=%.0s' {1..60})"; log "### $model"; log "$(printf '=%.0s' {1..60})"

    # 1) cold load
    log "  [cold load]"
    unload_all
    t=$(gen "$model" "Say 'ready'." 2)
    local cold_load; cold_load=$(jq '.load_s' <<<"$t")
    log "     load=${cold_load}s"

    # 2) generation throughput x3
    log "  [gen throughput x3]"
    local i
    for i in 1 2 3; do
        t=$(gen "$model" "Write a vivid 200-word description of a thunderstorm rolling over a coastal city at night." 256)
        gtps+=("$(jq '.gen_tps' <<<"$t")")
        log "     run$i: $(jq '.gen_tps' <<<"$t") tok/s ($(jq '.gen_tokens' <<<"$t") tok)"
    done
    # mean and population stdev over the 3 runs (jq: pstdev = sqrt(mean of sq dev)).
    local gtps_json; gtps_json=$(printf '%s\n' "${gtps[@]}" | jq -s '.')
    local gmean gstdev
    gmean=$(jq '(add/length) | (.*10|round)/10' <<<"$gtps_json")
    gstdev=$(jq '(add/length) as $m | (map((. - $m) as $d | $d*$d) | add/length | sqrt) | (.*10|round)/10' <<<"$gtps_json")

    # 3) prefill curve
    log "  [prefill curve]"
    local sz
    for sz in 256 1024 4096 8192; do
        t=$(gen "$model" "$(filler "$sz")"$'\n\nSummarize the above in one sentence.' 16)
        curve+=("$(jq -c --argjson target "$sz" \
            '{target:$target, prompt_tokens:.prompt_tokens, prefill_tps:.prefill_tps, prefill_s:.prefill_s}' <<<"$t")")
        log "     ~$sz: $(jq '.prompt_tokens' <<<"$t") tok @ $(jq '.prefill_tps' <<<"$t") tok/s"
    done

    # 4) long-context needle recall (~6k tokens)
    log "  [needle recall ~6k]"
    t=$(gen "$model" "$(haystack 6000)"$'\n\nWhat is the secret access code for the north gate? Answer with the code only.' 40)
    local up found needle
    up=$(printf '%s' "$REPLY_TEXT" | tr '[:lower:]' '[:upper:]')
    [[ "$up" == *"MULBERRY-7341"* ]] && found=true || found=false
    needle=$(jq -n --argjson pt "$(jq '.prompt_tokens' <<<"$t")" --argjson found "$found" \
                   --arg answer "$(printf '%s' "$REPLY_TEXT" | head -c 120)" \
                   '{prompt_tokens:$pt, found:$found, answer:$answer}')
    log "     tokens=$(jq '.prompt_tokens' <<<"$t") found=$found"

    # 5) capability probes
    log "  [capability probes]"
    local rec name npred prompt verdict
    for rec in "${PROBES[@]}"; do
        name="${rec%%|*}"; rec="${rec#*|}"; npred="${rec%%|*}"; prompt="${rec#*|}"
        t=$(gen "$model" "$prompt" "$npred")
        verdict=$(judge "$name" "$REPLY_TEXT")
        caps+=("$(jq -n --arg probe "$name" --arg verdict "$verdict" \
                        --argjson gen_tps "$(jq '.gen_tps' <<<"$t")" \
                        --arg output "$REPLY_TEXT" \
                        '{probe:$probe, verdict:$verdict, gen_tps:$gen_tps, output:($output|gsub("^\\s+|\\s+$";""))}')")
        log "     $name: $verdict"
    done

    # 6) thinking-mode characterization (reasoning enabled)
    log "  [thinking-mode reasoning]"
    t=$(gen "$model" "A snail climbs a 10m well, going up 3m each day and sliding back 2m each night. On which day does it reach the top? Give the final answer as: ANSWER: <day>." 1500 true)
    local think_chars gen_tokens correct hit_limit answer combined
    think_chars=$(jq '.think_chars' <<<"$t"); gen_tokens=$(jq '.gen_tokens' <<<"$t")
    combined="${REPLY_TEXT}${REPLY_THINK}"
    [[ "$combined" == *"8"* ]] && correct=true || correct=false
    (( gen_tokens >= 1490 )) && hit_limit=true || hit_limit=false
    answer=$(printf '%s' "$REPLY_TEXT" | head -c 200)
    [[ -z "$answer" ]] && answer="(empty response — all budget spent thinking)"
    local thinking_mode
    thinking_mode=$(jq -n --argjson tc "$think_chars" --argjson tt "$gen_tokens" \
        --argjson correct "$correct" --argjson hit "$hit_limit" --arg answer "$answer" \
        '{think_chars:$tc, think_tokens_est:$tt, answer_correct:$correct,
          done_reason_hit_limit:$hit, answer:$answer}')
    log "     think_chars=$think_chars"

    unload "$model"

    # Assemble this model's result object from all the pieces above.
    jq -n --arg model "$model" --argjson cold_load "$cold_load" \
          --argjson gtps "$gtps_json" --argjson gmean "$gmean" --argjson gstdev "$gstdev" \
          --argjson curve "$(printf '%s\n' "${curve[@]}" | jq -s '.')" \
          --argjson needle "$needle" \
          --argjson caps "$(printf '%s\n' "${caps[@]}" | jq -s '.')" \
          --argjson thinking_mode "$thinking_mode" '
        { model:$model, cold_load_s:$cold_load,
          gen_tps_runs:$gtps, gen_tps_mean:$gmean, gen_tps_stdev:$gstdev,
          prefill_curve:$curve, needle:$needle,
          capabilities:$caps, thinking_mode:$thinking_mode }'
}

# ---- main ----
models_json='[]'
for m in "${MODELS[@]}"; do
    if res=$(probe_model "$m"); then
        models_json=$(jq --argjson r "$res" '. + [$r]' <<<"$models_json")
    else
        log "  !! $m failed"
        models_json=$(jq --argjson r "$(jq -n --arg m "$m" '{model:$m, error:"probe failed"}')" \
                         '. + [$r]' <<<"$models_json")
    fi
done

jq -n --arg host "$HOST" --argjson models "$models_json" --argjson broken "$BROKEN" \
   '{host:$host, models:$models, broken:$broken}' > "$OUT"
log ""; log "Wrote $OUT"
```

Make executable: `chmod +x scripts/ollama-probe.sh`.

**Deviation note:** the `coding` probe now asks for **Rust** (not Python) and `judge` matches `fn median_two_sorted` — the repo forbids Python, so a probe that demands Python code is inappropriate. This is an intentional, documented content change. All other probes are verbatim.

- [ ] **Step 2: Validate every embedded jq filter offline**

Feed a recorded `/api/generate` response through the timings jq to confirm shape/precision before any live call:

```bash
echo '{"response":"hi","thinking":"","load_duration":1500000000,"prompt_eval_count":100,"prompt_eval_duration":2000000000,"eval_count":50,"eval_duration":1000000000,"total_duration":4500000000}' \
  | jq '1000000000 as $ns | def tps($c;$d): if $d>0 then ($c/($d/$ns)) else 0 end; def r1:(.*10|round)/10; {prefill_tps:(tps(.prompt_eval_count;.prompt_eval_duration)|r1), gen_tps:(tps(.eval_count;.eval_duration)|r1)}'
```

Expected: `{"prefill_tps":50,"gen_tps":50}` (100 tok / 2.0s = 50; 50 tok / 1.0s = 50). Confirms the derived-metric math matches the Python.

- [ ] **Step 3: shellcheck**

Run: `shellcheck scripts/ollama-probe.sh`
Expected: clean (single-quoted jq programs may need a scoped `# shellcheck disable=SC2016` with an explanatory comment; add only where it actually warns).

- [ ] **Step 4: Live probe run (verification)**

Server `192.168.1.240:11434` is reachable and all three models are present but **cold** (none loaded), so the run pays MLX cold-start — the 600s per-request timeout absorbs it. Run:

```bash
scripts/ollama-probe.sh > /dev/null    # progress on stderr; results to default OUT
jq -e '.host and (.models | length == 3) and (.models[0] | has("cold_load_s") and has("gen_tps_mean") and (.prefill_curve|length==4) and (.capabilities|length==7))' /tmp/ollama_probe_results.json
```

Expected: the run streams the same progress shape as the Python; the `jq -e` assertion exits 0, confirming the result JSON has the full structure for all three models. Spot-check a couple of `capabilities[].verdict` and `needle.found` values for plausibility.

(Cold-start note: the probe deliberately measures `cold_load_s` by unloading first, so pre-warming would corrupt that metric. To reduce wall-clock during iteration only, a single model could be probed by temporarily narrowing `MODELS`, but the committed verification run uses all three.)

- [ ] **Step 5: Commit**

```bash
git add scripts/ollama-probe.sh
git commit -m "feat(scripts): add bash ollama-probe.sh (replaces .py) (#360)"
```

---

### Task 4: Final cleanup, doc references, and full verification

**Files:**
- Modify (conditional): any doc literally naming `ollama_probe.py`
- Delete (manual, out of git): `scripts/ollama_probe.py` in the main checkout

**Interfaces:**
- Consumes: everything from Tasks 1–3.
- Produces: a repo with zero Python and zero stale references.

- [ ] **Step 1: Grep for stale references to either Python script**

Run: `rg -n 'ollama_probe\.py|ollama-probe\.py|coverage-report\.py' .`
Expected: no matches. If a doc names `ollama_probe.py` literally, update it to `ollama-probe.sh`. (The probe-findings docs reference `/tmp/ollama-probe*/` artifact dirs, not the script filename — leave those.)

- [ ] **Step 2: Confirm no Python remains in the repo**

Run: `fd -e py . && echo "PYTHON FOUND" || echo "no python"`
Expected: `no python` (the `fd` finds nothing; the tracked `.py` is gone and no new `.py` was added). If any doc edits from Step 1 were made, commit them:

```bash
git add -A && git commit -m "docs(scripts): point references at bash scripts (#360)" || true
```

- [ ] **Step 3: Full local verification gate (repo policy)**

Although no Rust changed, run the repo's gate to be certain nothing regressed:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --all-targets
cargo test --workspace
```

Expected: all pass. (Fast, since the worktree may already be built; if the build is cold this is the slow step.)

- [ ] **Step 4: Flag the untracked Python for manual removal**

`scripts/ollama_probe.py` is untracked on `main`, so it lives only in the user's main checkout and **this branch cannot delete it**. Surface this to the user with the exact command:

```bash
rm /Users/johnford2002/dev/caliban-ai/caliban/scripts/ollama_probe.py
```

Do not run it against the main checkout from the worktree without confirming; just report it.

- [ ] **Step 5: Push branch and open the PR**

```bash
git push -u origin worktree-chore+migrate-python-scripts-to-bash
gh pr create --title "chore(scripts): migrate Python scripts to bash (#360)" --body "<summary + verification evidence>"
```

The PR body summarizes the migration, the two intentional deviations (footer filename; Rust coding-probe), the verification evidence (fixture diffs + live probe assertion), and notes the untracked `.py` needing manual removal. Closes #360.

---

## Self-Review

**Spec coverage:**
- coverage-report.sh (bash+jq, interface + all six transform stages) → Task 1 ✅
- ollama-probe.sh (bash+curl+jq, gen/fixtures/probes/judge/stats/stderr) → Task 3 ✅
- CI/README/coverage.sh rewiring → Task 2 ✅
- Delete tracked `.py` → Task 2 Step 4 ✅
- Flag untracked `.py` → Task 4 Step 4 ✅
- Conditional doc edits → Task 4 Step 1 ✅
- Byte-parity verification → Task 1 Steps 2/4/5 (diff) ✅
- Structural + live probe verification → Task 3 Steps 2/4 ✅
- Self-documenting comments → embedded in both scripts (Global Constraints) ✅
- Rust fmt/clippy/build/test gate → Task 4 Step 3 ✅

**Placeholder scan:** No TBD/TODO; every code step shows full code; every command shows expected output. The only `<...>` is the PR body prose in Task 4 Step 5, which is narrative, not code. ✅

**Type/name consistency:** `gen()` returns timings JSON + sets `REPLY_TEXT`/`REPLY_THINK` globals — used consistently in every caller. `judge NAME TEXT`, `filler N`, `haystack N`, `probe_model MODEL` signatures consistent across definition and call sites. jq helper names (`emoji`, `bar`, `commas`, `f1`, `crate_of`, `relpath`, `tps`, `r1/r2/r3`) defined before use. `MODELS`/`BROKEN` constants match Task 3 interface block. ✅
