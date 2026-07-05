# Python → Bash script migration (coverage-report + ollama-probe)

**Date:** 2026-07-05
**Ticket:** #360 (migrate `ollama_probe.py` + `coverage-report.py` off Python)
**Status:** Approved design

## Problem

Caliban's policy is that repo scripts are **bash or Rust — never Python** (see the
`no-python-in-project` convention). Two Python files violate this:

- `scripts/coverage-report.py` — renders a `cargo-llvm-cov` JSON export into a
  Markdown coverage report. Invoked by CI (`.github/workflows/ci.yml`) to build
  the sticky PR comment / job summary, and locally to preview it.
- `scripts/ollama_probe.py` — a standalone diagnostic that probes an Ollama
  server (load time, throughput, prefill curve, needle recall, capability
  probes) and writes a JSON result file. Not in CI; run by hand. This file is
  **untracked** on `main`, so it exists only in the local working tree.

## Goals

- Replace both with **fully-bash** scripts (Option A), using `jq` for JSON and
  `curl` for HTTP — the standard shell idioms. No new Cargo/publish surface.
- **Preserve CLI interfaces and outputs byte-for-byte** where feasible. This is
  a language migration, not a behavior change.
- **Self-documenting code:** every non-trivial `jq` block carries a prose
  comment explaining input → output and any non-obvious transform stage
  (grouping, multi-key sort direction, bar-width math, stdev formula). A reader
  who knows jq only casually should follow each stage without running it.

## Non-goals

- No change to what CI posts, what the gate enforces, or what the probe
  measures.
- No change to `scripts/coverage.sh` beyond updating its inline reference to the
  renderer's new filename.
- No migration to Rust. (Considered and rejected: adds a workspace crate that CI
  must build and `cargo publish` must exclude — architecturally heavy for a
  coverage comment and a throwaway diagnostic.)

## Prerequisites

`jq`, `curl`, and (for local lint) `shellcheck` are available locally and `jq`
is preinstalled on GitHub `ubuntu` runners. Confirmed present in the dev
environment.

## Deliverables

| Action | Path |
|---|---|
| **New** | `scripts/coverage-report.sh` (bash + jq) |
| **New** | `scripts/ollama-probe.sh` (bash + curl + jq) |
| **Delete (git rm)** | `scripts/coverage-report.py` |
| **Delete (manual, untracked)** | `scripts/ollama_probe.py` in the local checkout — a branch can't remove an unstaged file; flagged for the user |
| **Edit** | `.github/workflows/ci.yml` — `python3 scripts/coverage-report.py …` → `scripts/coverage-report.sh …` |
| **Edit** | `README.md` — `python3 scripts/coverage-report.py` → `scripts/coverage-report.sh` |
| **Edit** | `scripts/coverage.sh` — inline `scripts/coverage-report.py` mentions → `.sh` |
| **Edit (conditional)** | Any doc that names `ollama_probe.py` literally (grep first; the probe-findings docs reference artifact dirs, not the filename — touch only on a real hit) |

## Design: `scripts/coverage-report.sh`

Same interface as the Python:

```
scripts/coverage-report.sh [JSON] [--root DIR] [--floor N] [--target N]
                                  [--commit SHA] [--max-gaps N] [--min-gap-lines N]
```

- `JSON` positional, default `target/llvm-cov/coverage.json`.
- `--commit` default `$GITHUB_SHA`.
- Tier thresholds `GREEN=85.0 / YELLOW=75.0` (unchanged).

**Structure:** bash owns only arg parsing (a `while`/`case` loop) and piping.
The entire JSON → Markdown transform is **one `jq -r` program**, fed the flags
via `--arg`/`--argjson`. jq owns:

1. **Relpath filtering** — turn each `entry.filename` into a path relative to
   `--root`; drop files outside the workspace (registry deps).
2. **Per-crate grouping + summation** — bucket files by crate (`crates/<x>/…`
   → `<x>`; `caliban/…` → `caliban`; else first path segment), summing line
   `count`/`covered`.
3. **Multi-key sort** — crates by `(pct ascending, count descending)` so the
   lowest-coverage, largest crates float to the top.
4. **Bar rendering** — `filled = round(pct/100 * width)`, clamped to
   `[0, width]`; `"█" × filled + "░" × (width − filled)`.
5. **Emoji tiers** — `≥85 🟢`, `≥75 🟡`, else `🔴`.
6. **Gap table** — files with `count ≥ --min-gap-lines` and `pct < --target`,
   sorted by missed lines descending, capped at `--max-gaps`.

**Output:** identical Markdown to the Python (totals table, collapsed by-crate
`<details>`, notable-gaps table, generated-by footer with short commit).
Numeric formatting matches Python's `round`/`:.1f`/`:,` (thousands separators);
where jq lacks a direct equivalent, a helper filter reproduces it and is
commented as such.

## Design: `scripts/ollama-probe.sh`

Same interface: `scripts/ollama-probe.sh [host] [out.json]`
(defaults `http://192.168.1.240:11434`, `/tmp/ollama_probe_results.json`).

- **`$MODELS`** and **`$BROKEN`** constants preserved verbatim.
- **`gen()`** — bash function: `curl` POST to `/api/generate` (non-streaming),
  pipe the response through a commented `jq` filter that extracts the timing
  fields and computes derived metrics (`prefill_tps`, `gen_tps` via
  `count / (duration_ns / 1e9)`; guard divide-by-zero). Returns the response
  text and a timings JSON object.
- **Prefill fixtures** — `filler(approx_tokens)` and `haystack(approx_tokens)`
  (needle injection) reproduced in bash string ops.
- **Capability probes** — stored as a bash array of
  `name|prompt|num_predict` records (a delimiter safe for the prompt text);
  looped. **`judge()`** implements each probe's heuristic in jq/bash
  (substring match, JSON-shape check, word count).
- **Stats** — `gen_tps_mean` / `gen_tps_stdev` (population stdev) computed in jq
  with the formula spelled out in a comment.
- **Progress** — human-readable progress lines go to **stderr** (matching the
  Python's `flush=True` streaming); the machine-readable result JSON is
  assembled with jq and written to the out path. Per-model failures are caught
  and recorded as `{model, error}` without aborting the run.

## Verification

Both old and new scripts run against the **same** input; outputs diffed.

- **coverage-report:** obtain a real `coverage.json` (from `scripts/coverage.sh`
  or a saved fixture), run `coverage-report.py` and `coverage-report.sh` with
  identical args, `diff` the Markdown. Must be identical (any intentional
  float-format deviation documented and justified). Also validate the jq program
  standalone.
- **ollama-probe:** the server (`192.168.1.240:11434`) may be unreachable from
  the dev host. If reachable, run end-to-end and sanity-check the JSON. If not,
  verify structurally: feed a recorded `/api/generate` response through `gen()`
  and confirm the timings/derived metrics/result JSON shape match the Python's;
  validate every embedded jq filter with `jq -n`.
- **Lint/gate:** `shellcheck` clean on both scripts. The Rust fmt/clippy/
  build/test gate is unaffected (no Rust changed) but run per the repo's
  local-verification policy before the PR.

## Rollback

Pure additive-then-swap: reverting the CI/README/`coverage.sh` edits and
restoring `coverage-report.py` from git history returns to the prior state. No
data migration, no schema change.
