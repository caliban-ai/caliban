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

# Parse args: the first bare word is the JSON path; the rest are --flag value
# pairs mirroring coverage-report's original argparse interface.
while [[ $# -gt 0 ]]; do
    case "$1" in
        --root)          ROOT="$2"; shift 2 ;;
        --floor)         FLOOR="$2"; shift 2 ;;
        --target)        TARGET="$2"; shift 2 ;;
        --commit)        COMMIT="$2"; shift 2 ;;
        --max-gaps)      MAX_GAPS="$2"; shift 2 ;;
        --min-gap-lines) MIN_GAP_LINES="$2"; shift 2 ;;
        -h|--help)       sed -n '2,18p' "$0"; exit 0 ;;
        -*)              echo "unknown flag: $1" >&2; exit 2 ;;
        *)               JSON="$1"; shift ;;
    esac
done

# The entire JSON -> Markdown transform lives in this one jq program; bash only
# parsed args and now pipes the coverage document in. The flags cross the
# shell/jq boundary as jq variables: $root (string), $commit (string), and
# $floor/$target/$max_gaps/$min_gap_lines (numbers). `jq -r` emits raw lines
# (no surrounding JSON quotes). The single quotes keep the whole program away
# from the shell, so every $name below is a jq variable, not a shell one.
# shellcheck disable=SC2016
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

  # Proportional bar: round(pct/100 * width) filled blocks, clamped to [0,width]
  # and padded to width with light blocks. Matches Python int(round(...)).
  # ("x" * 0 is null in jq, which concatenates as empty — so a 0- or full-width
  # bar still renders correctly.)
  def bar($p; $w):
    (((($p / 100) * $w) | round) | [., 0] | max | [., $w] | min) as $f
    | ("█" * $f) + ("░" * ($w - $f));

  # Integer percent, rounded (Python f"{x:.0f}").
  def r0: round;

  # 1-decimal fixed print, reproducing Python f"{x:.1f}" (round to a tenth, then
  # guarantee a trailing ".0" for whole numbers so 90 -> "90.0").
  def f1: ((. * 10 | round) / 10) | tostring | if test("[.]") then . else . + ".0" end;

  # Thousands separators: 12345 -> "12,345" (Python f"{x:,}"). jq has no grouping
  # format, so walk the integer digits right-to-left and splice in a comma (44)
  # after every third digit except the very last.
  def commas:
    tostring | explode | reverse
    | [ range(0; length) as $i
        | (.[$i], (if ($i % 3 == 2) and ($i != length - 1) then 44 else empty end)) ]
    | reverse | implode;

  # crate bucket for a workspace-relative path: crates/<x>/... -> <x>;
  # caliban/... -> caliban; anything else -> its first path segment.
  def crate_of($rel):
    ($rel | split("/")) as $p
    | if ($p[0] == "crates" and ($p | length) >= 2) then $p[1]
      elif $p[0] == "caliban" then "caliban"
      else $p[0] end;

  # relpath($abs; $root): strip a trailing-slash-normalized $root prefix. Returns
  # null when $abs is not under $root — the caller then drops it, matching the
  # Python guard that discards os.path.relpath results starting with "..".
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
  # Aggregate line counts per crate, then attach each crate coverage percent.
  | ( $files | group_by(.crate)
      | map({ crate: .[0].crate,
              count: (map(.count) | add),
              covered: (map(.covered) | add) })
      | map(. + { pct: (if .count > 0 then 100 * .covered / .count else 0 end) }) ) as $crates

  # ---- render (each emitted string is one output line) ------------------
  | ($totals.lines.percent) as $line_pct
  # Wrap the whole comma-separated line stream so it stays inside the $line_pct
  # binding (in jq, `X as $v | a, b` would scope only `a` to the binding).
  | (
    "## 📊 Coverage Report",
    "",
    "### \(emoji($line_pct)) **\($line_pct|f1)%** line coverage &nbsp;·&nbsp; floor **\($floor|r0)%** &nbsp;·&nbsp; target **\($target|r0)%**",
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
      | "| `\(.crate)` | \(emoji(.pct)) `\(bar(.pct;12))` \(.pct|r0)% | \(.covered|commas) / \(.count|commas) |" ),
    "",
    "</details>",
    "",
    # Notable gaps: files >= min_gap_lines and below target, most-missed first.
    ( [ $files[] | select(.count >= $min_gap_lines and .pct < $target) ]
      | sort_by(-(.missed)) | .[0:$max_gaps] ) as $gaps
    | ( if ($gaps | length) > 0 then
          ( "### 🔍 Notable gaps",
            "",
            "Files with the most uncovered lines (≥ \($min_gap_lines) lines, below the \($target|r0)% target):",
            "",
            "| File | Coverage | Missed | Lines |",
            "|---|---|---|---|",
            ( $gaps[]
              | "| `\(.rel)` | \(emoji(.pct)) \(.pct|r0)% | \(.missed|commas) | \(.covered|commas) / \(.count|commas) |" ),
            "" )
        else empty end ),
    # Footer credits the .sh (the only intended deviation from the .py output).
    ( (if ($commit | length) > 0 then " · commit `\($commit[0:7])`" else "" end) as $c
      | "<sub>Generated by <code>scripts/coverage-report.sh</code> from cargo-llvm-cov · gate: <code>scripts/coverage.sh</code>\($c)</sub>" )
  )
' "$JSON"
