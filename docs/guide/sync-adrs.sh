#!/usr/bin/env bash
# Copy docs/adr/*.md into the mdBook guide and regenerate the ADR SUMMARY block.
# Run from the repo root (the docs workflow runs it before `mdbook build`).
# Portable across BSD (macOS) and GNU awk.
set -euo pipefail

ADR_SRC="docs/adr"
GUIDE_SRC="docs/guide/src"
DEST="$GUIDE_SRC/adr"
SUMMARY="$GUIDE_SRC/SUMMARY.md"
MARKER="<!-- adrs -->"

# Files in docs/adr that are not themselves ADRs.
is_adr() {
  case "$(basename "$1")" in
    index.md | README.md | template.md) return 1 ;;
    *) return 0 ;;
  esac
}

mkdir -p "$DEST"
cp "$ADR_SRC"/*.md "$DEST"/

# Build an ADR index page from the file titles (first markdown H1 of each file).
{
  echo "# Architecture Decision Records"
  echo
  for f in "$DEST"/*.md; do
    is_adr "$f" || continue
    base="$(basename "$f")"
    title="$(grep -m1 '^# ' "$f" | sed 's/^# //')"
    echo "- [${title:-$base}](./${base})"
  done
} > "$DEST/index.md"

# Build the nested SUMMARY entries (newest mdBook needs every page listed).
entries=""
for f in "$DEST"/*.md; do
  is_adr "$f" || continue
  base="$(basename "$f")"
  title="$(grep -m1 '^# ' "$f" | sed 's/^# //')"
  entries+="  - [${title:-$base}](./adr/${base})"$'\n'
done

# Regenerate everything after the marker: keep the file up to and including the
# marker line, then append the fresh entries. Uses sed (not a multi-line awk -v),
# which is portable across BSD/macOS and GNU awk.
grep -qF -- "$MARKER" "$SUMMARY" || {
  echo "error: ADR marker '$MARKER' not found in $SUMMARY" >&2
  exit 1
}
tmp="$SUMMARY.tmp"
sed "/$MARKER/q" "$SUMMARY" > "$tmp"
printf '%s' "$entries" >> "$tmp"
mv "$tmp" "$SUMMARY"

# Completeness gate for the hand-curated topical appendix (#725).
#
# `appendix/adrs.md` groups records into editorial topical sections, so unlike
# the index/SUMMARY above it is NOT generated — and nothing used to fail when a
# new ADR skipped it, so it drifted seven records behind before #712 caught it.
# Fail here, naming any ADR present in docs/adr/ but absent from the appendix, so
# a missed record shows up at docs-build time instead of silently rotting. The
# curation (topical grouping) is preserved; only completeness is enforced.
APPENDIX="$GUIDE_SRC/appendix/adrs.md"
if [[ -f "$APPENDIX" ]]; then
  missing=""
  for f in "$ADR_SRC"/*.md; do
    is_adr "$f" || continue
    num="$(basename "$f" | sed -E 's/^([0-9]{4})-.*/\1/')"
    # ADRs appear in the appendix as the Markdown link `[NNNN](…)`; a plain
    # fixed-string match on `[NNNN]` is enough and avoids false hits on digits
    # inside prose.
    grep -qF -- "[$num]" "$APPENDIX" || missing+="  $num  ($(basename "$f"))"$'\n'
  done
  if [[ -n "$missing" ]]; then
    {
      echo "error: these ADRs are missing from $APPENDIX (#725):"
      printf '%s' "$missing"
      echo "Add each to the appropriate topical section — the appendix is hand-curated, not generated."
    } >&2
    exit 1
  fi
fi
