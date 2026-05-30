#!/usr/bin/env bash
# Mirror CI's default-features check suite locally. Run before pushing
# to catch the same errors CI catches (fmt, clippy::pedantic with
# warnings-as-errors, build, test) without waiting for the GitHub Actions
# round trip.
#
# Why this exists: PR #78 merged with a CI fail because local `cargo
# test` alone doesn't run clippy. The workspace's `[workspace.lints]`
# sets `clippy::pedantic = "deny"`, and a single doc-markdown warning
# in caliban/src/tui/clipboard.rs slipped past the local test run. The
# follow-up PR #79 fixed it; this script prevents the next iteration.
#
# Usage:
#   scripts/check.sh             # fmt + clippy + build + test (default features)
#   scripts/check.sh --cloud     # additionally run the cloud-features build
#                                #   (bedrock + vertex + azure — slow)
#   scripts/check.sh --no-test   # skip the test step (still does fmt + clippy + build)
#   scripts/check.sh -h | --help
#
# Exit codes match the first failing step. Pristine output (no warnings)
# is the goal: clippy is invoked with -D warnings so any warning fails.

set -euo pipefail

cd "$(dirname "$0")/.."

DO_CLOUD=0
DO_TEST=1

for arg in "$@"; do
    case "$arg" in
        --cloud)   DO_CLOUD=1 ;;
        --no-test) DO_TEST=0 ;;
        -h|--help)
            sed -n '2,21p' "$0"
            exit 0
            ;;
        *)
            echo "unknown flag: $arg" >&2
            exit 2
            ;;
    esac
done

run() {
    echo "==> $*"
    "$@"
}

run cargo fmt --all -- --check
run cargo clippy --workspace --all-targets -- -D warnings
run cargo build --workspace --all-targets

if [[ $DO_TEST -eq 1 ]]; then
    run cargo test --workspace
fi

if [[ $DO_CLOUD -eq 1 ]]; then
    # Mirrors .github/workflows/ci-cloud.yml. Slower because it pulls
    # aws-sdk-bedrockruntime, oauth2, etc.
    CLOUD_FEATURES="caliban-provider-anthropic/bedrock,caliban-provider-anthropic/vertex,caliban-provider-openai/azure,caliban-provider-google/vertex"
    run cargo build  --workspace --features "$CLOUD_FEATURES"
    run cargo clippy --workspace --features "$CLOUD_FEATURES" --all-targets -- -D warnings
    if [[ $DO_TEST -eq 1 ]]; then
        run cargo test --workspace --features "$CLOUD_FEATURES"
    fi
fi

echo
echo "all checks passed"
