#!/usr/bin/env bash
#
# probe-discovery-endpoints.sh — dump what each local engine exposes for
# model / capability / context-window DISCOVERY, so the caliban OpenAI adapter
# shim (#632, ADR 0056) can parse the real response shapes rather than guesses.
#
# Uses llama-swap's `/upstream/<model_id>/<path>` passthrough, so it reaches each
# backend's endpoints (including llama.cpp-only ones like /props and /slots)
# THROUGH the :9292 front door — no need to start servers separately. The first
# hit to a model loads it on demand, so give it a moment.
#
# Requires: curl, jaq. No Python.
#
# Usage:
#   ./probe-discovery-endpoints.sh
#   SWAP=http://192.168.1.240:9292 ./probe-discovery-endpoints.sh
#
# Env:
#   SWAP     llama-swap base URL (no /v1)  (default http://192.168.1.240:9292)
#   MODELS   newline-separated llama-swap model keys to probe
#            (default: the two from the runbook)

set -euo pipefail

SWAP="${SWAP:-http://192.168.1.240:9292}"
MODELS="${MODELS:-qwen3.6-27b-gguf
mlx-community/Qwen3.6-27B-4bit}"

for dep in curl jaq; do
  command -v "$dep" >/dev/null 2>&1 || { echo "error: '$dep' not found on PATH" >&2; exit 1; }
done

# GET a URL; print "HTTP <code>" then pretty JSON (or a short raw snippet).
dump() { # label url
  local label="$1" url="$2" body code
  body="$(curl -s -m 120 -w $'\n__HTTP__%{http_code}' "$url" 2>/dev/null || true)"
  code="${body##*__HTTP__}"
  body="${body%$'\n'__HTTP__*}"
  printf '  --- %-34s ' "$label"
  if [ -z "$code" ]; then echo "(no response / unreachable)"; return; fi
  printf 'HTTP %s\n' "$code"
  if printf '%s' "$body" | jaq -e . >/dev/null 2>&1; then
    printf '%s\n' "$body" | jaq . | sed 's/^/      /'
  elif [ -n "$body" ]; then
    printf '      %s\n' "$(printf '%s' "$body" | head -c 200)"
  fi
}

echo "================================================================"
echo ">> llama-swap itself  ($SWAP)"
echo "================================================================"
dump "GET /v1/models (aggregate)" "$SWAP/v1/models"
dump "GET /running" "$SWAP/running"
echo

while IFS= read -r model; do
  [ -z "${model:-}" ] && continue
  enc="${model//\//%2F}"   # url-encode any '/' in the model id for the path segment
  echo "================================================================"
  echo ">> upstream backend: $model"
  echo "   (first hit loads the model on demand — may take a while)"
  echo "================================================================"
  for ep in v1/models props slots health; do
    dump "GET /upstream/<model>/$ep" "$SWAP/upstream/$enc/$ep"
    # if the id contained a '/', also try the raw (unencoded) path form in case
    # llama-swap's router wants the literal slash rather than %2F.
    if [ "$enc" != "$model" ]; then
      dump "  (raw-slash) /upstream/$model/$ep" "$SWAP/upstream/$model/$ep"
    fi
  done
  echo
done <<EOF
$MODELS
EOF

cat <<'EOF'
What to look for (feeds the #632 shim):
  - /upstream/<model>/v1/models .data[].id  — per-backend model identity
    (mlx-lm reports "default_model"; llama.cpp the repo/file name).
  - /upstream/<gguf>/props .default_generation_settings.n_ctx (or .n_ctx)
    — llama.cpp's LOADED context window. The number we want to recover.
  - /upstream/<mlx>/props — expect 404 (mlx-lm has no /props); that confirms MLX
    context-window falls back to a configured value, per ADR 0056.
  - Which model-id URL form works for the slash-containing MLX key (%2F vs raw).
Paste the whole output and I'll build the parser against these exact shapes.
EOF
