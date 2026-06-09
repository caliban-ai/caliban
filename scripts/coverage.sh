#!/usr/bin/env bash
# Measure workspace test coverage and enforce a minimum line-coverage
# threshold. This is the single entrypoint used by both humans and CI
# (.github/workflows/ci.yml), so the local and CI code paths are identical.
#
# Why this exists: the workspace had no coverage visibility and no guard
# against regressions. This script runs cargo-llvm-cov over the whole
# workspace (matching `cargo test --workspace` from check.sh) and fails
# when line coverage drops below COVERAGE_MIN — a ratchet that stops new
# work from silently eroding test coverage. See issue #65.
#
# Tooling: cargo-llvm-cov (LLVM source-based coverage). Install with
#   cargo install cargo-llvm-cov --locked
# and ensure the llvm-tools-preview component is present:
#   rustup component add llvm-tools-preview
#
# Usage:
#   scripts/coverage.sh              # summary + lcov artifact, enforce threshold
#   scripts/coverage.sh --html       # also write an HTML report, enforce threshold
#   scripts/coverage.sh --open       # --html, then open the report in a browser
#   scripts/coverage.sh --no-fail    # report only; never fail on low coverage
#   scripts/coverage.sh -h | --help
#
# Environment:
#   COVERAGE_MIN   minimum line-coverage percent (default below). Override to
#                  ratchet the floor up over time, e.g. COVERAGE_MIN=42 scripts/coverage.sh
#
# Outputs (under target/llvm-cov/):
#   target/llvm-cov/lcov.info      LCOV report (consumed by CI artifact / Codecov)
#   target/llvm-cov/html/          HTML report (only with --html / --open)
#
# Exit code is non-zero when coverage is under COVERAGE_MIN (unless --no-fail).

set -euo pipefail

cd "$(dirname "$0")/.."

# Baseline floor — the single source of truth for the coverage gate. CI
# (.github/workflows/ci.yml) calls this script without overriding COVERAGE_MIN,
# so this default governs both local and CI runs. Start at/just below the
# current measured coverage and ratchet upward over time as tests are added.
# Baseline measured 2026-06-08 was 78.61% line coverage; floor set a few points
# below to absorb cross-environment/nondeterministic variance, then ratchet up.
COVERAGE_MIN="${COVERAGE_MIN:-75}"

DO_HTML=0
DO_OPEN=0
DO_FAIL=1

for arg in "$@"; do
    case "$arg" in
        --html)    DO_HTML=1 ;;
        --open)    DO_HTML=1; DO_OPEN=1 ;;
        --no-fail) DO_FAIL=0 ;;
        -h|--help)
            sed -n '2,30p' "$0"
            exit 0
            ;;
        *)
            echo "unknown flag: $arg" >&2
            exit 2
            ;;
    esac
done

if ! cargo llvm-cov --version >/dev/null 2>&1; then
    cat >&2 <<'MSG'
error: cargo-llvm-cov is not installed.

  cargo install cargo-llvm-cov --locked
  rustup component add llvm-tools-preview   # provides llvm-cov / llvm-profdata

See https://github.com/taiki-e/cargo-llvm-cov for details.
MSG
    exit 127
fi

run() {
    echo "==> $*"
    "$@"
}

OUT_DIR="target/llvm-cov"
LCOV_PATH="$OUT_DIR/lcov.info"

# cargo-llvm-cov does not create the parent dir for a custom --output-path.
mkdir -p "$OUT_DIR"

# Threshold enforcement is a flag on the cargo-llvm-cov invocation itself, so
# the same run produces the report *and* gates on it.
fail_args=()
if [[ $DO_FAIL -eq 1 ]]; then
    fail_args=(--fail-under-lines "$COVERAGE_MIN")
fi

echo "coverage floor: ${COVERAGE_MIN}% line coverage (COVERAGE_MIN)"

# Always produce an LCOV artifact + a stdout summary table. --workspace mirrors
# the default-features test suite that check.sh / ci.yml run.
run cargo llvm-cov --workspace --lcov --output-path "$LCOV_PATH" "${fail_args[@]}"

if [[ $DO_HTML -eq 1 ]]; then
    # A second pass reuses the gathered profile data (no re-test) to render HTML.
    run cargo llvm-cov report --html --output-dir "$OUT_DIR"
    echo "HTML report: $OUT_DIR/html/index.html"
    if [[ $DO_OPEN -eq 1 ]]; then
        open "$OUT_DIR/html/index.html" 2>/dev/null \
            || xdg-open "$OUT_DIR/html/index.html" 2>/dev/null \
            || echo "open the report manually: $OUT_DIR/html/index.html"
    fi
fi

echo
echo "coverage report written to $LCOV_PATH"
