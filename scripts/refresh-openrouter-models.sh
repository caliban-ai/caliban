#!/usr/bin/env bash
# Regenerate the vendored OpenRouter model snapshot.
#
# The snapshot backs `OpenRouterProvider::capabilities` / `list_models`, which
# must work offline and must never put a third-party endpoint on caliban's boot
# path (#573). Run this by hand when OpenRouter's catalogue moves; commit the
# result. `refresh_models()` is the live, opt-in path — this script is the
# vendored one.
#
# Usage: scripts/refresh-openrouter-models.sh [output-path]
set -euo pipefail

OUT="${1:-crates/caliban-provider-openrouter/models.json}"
URL="${OPENROUTER_MODELS_URL:-https://openrouter.ai/api/v1/models}"

command -v jq >/dev/null || { echo "error: jq is required" >&2; exit 1; }

echo "fetching $URL ..." >&2
raw="$(curl -fsS --max-time 60 "$URL")"

count="$(printf '%s' "$raw" | jq '.data | length')"
[ "$count" -gt 0 ] || { echo "error: catalogue returned 0 models; refusing to write" >&2; exit 1; }

# Trim to the fields that map onto `caliban_provider::Capabilities`. Everything
# else in OpenRouter's payload is presentation metadata we do not consume, and
# vendoring it would bloat the crate for no gain.
printf '%s' "$raw" | jq --arg url "$URL" '{
  schema_version: 1,
  source: $url,
  model_count: (.data | length),
  models: [
    .data[]
    | {
        id,
        context_length,
        max_completion_tokens: (.top_provider.max_completion_tokens // null),
        input_modalities: (.architecture.input_modalities // []),
        supported_parameters: (.supported_parameters // []),
        pricing: {
          prompt: (.pricing.prompt // null),
          completion: (.pricing.completion // null)
        }
      }
  ] | sort_by(.id)
}' > "$OUT"

echo "wrote $OUT ($count models)" >&2
