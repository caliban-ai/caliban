#!/usr/bin/env bash
#
# env-registry lint (#701).
#
# Bucket-A `CALIBAN_*` variables that shadow a settings key are migrated into the
# settings env-override registry (`Settings::apply_env_overrides` in
# crates/caliban-settings/src/settings.rs). Once migrated, a variable must be
# read ONLY through that registry — never with an ad-hoc `std::env::var("CALIBAN_…")`
# elsewhere — so env-vs-settings precedence lives in one place and
# `caliban config print` can attribute the value.
#
# This lint fails when a *migrated* var is read ad-hoc outside the registry file.
# As each epic cluster lands, add its variables to BANNED below; the set of
# Bucket-A vars still permitted to be read ad-hoc shrinks toward empty.
#
# Bucket-B vars (path pointers, daemon/launch contract, logging bootstrap,
# secrets/tokens, OTEL/provider SDK contracts, test fixtures) are deliberately
# NOT listed here — they legitimately read the environment directly.
set -euo pipefail

# Migrated Bucket-A vars (must be read only via the registry).
BANNED=(
  CALIBAN_STORAGE_SUBSTRATE
  CALIBAN_STORAGE_REMOTE_URL
  CALIBAN_STORAGE_REMOTE_TOKEN_ENV
  CALIBAN_OUTPUT_STYLE
  CALIBAN_DEFAULT_PERMISSION_MODE
)

# The registry file is the one place these vars are named + read.
ALLOW_FILE="crates/caliban-settings/src/settings.rs"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

banned_re="$(
  IFS='|'
  echo "${BANNED[*]}"
)"

# Ad-hoc reads: std::env::var / env::var / var_os with a CALIBAN_* string literal.
matches="$(grep -rEn 'env::var(_os)?[[:space:]]*\([[:space:]]*"CALIBAN_[A-Z0-9_]+"' \
  --include='*.rs' crates caliban 2>/dev/null || true)"

fail=0
while IFS= read -r line; do
  [ -z "$line" ] && continue
  file="${line%%:*}"
  var="$(printf '%s' "$line" | grep -oE 'CALIBAN_[A-Z0-9_]+' | head -1)"
  if printf '%s' "$var" | grep -qE "^(${banned_re})$" && [ "$file" != "$ALLOW_FILE" ]; then
    echo "✗ $line"
    fail=1
  fi
done <<EOF
$matches
EOF

if [ "$fail" -ne 0 ]; then
  {
    echo ""
    echo "ERROR: the lines above read a migrated Bucket-A CALIBAN_* variable ad-hoc."
    echo "Migrated vars must flow through Settings::apply_env_overrides in"
    echo "  $ALLOW_FILE"
    echo "and consumers should read the (env-folded) settings value instead (#701)."
  } >&2
  exit 1
fi

echo "env-registry lint: ok (no ad-hoc reads of migrated Bucket-A CALIBAN_* vars)"
