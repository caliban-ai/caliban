#!/usr/bin/env bash
#
# bench-local-inference.sh — local LLM prefill/TTFT + decode benchmark
#
# Compares OpenAI-compatible local inference backends on a *caliban-shaped*
# prompt: a large system+tools prefix with a short completion. That shape is
# what actually drives agent latency, so it exercises the axis that matters
# (prefill / time-to-first-token) rather than the decode-tok/s headline.
#
# Default backends (edit / override via env):
#   - Metal (llama.cpp)  via llama-swap        :9292
#   - MLX   (mlx-lm)      via llama-swap        :9292
#   - Ollama             direct OpenAI endpoint :11434   (what you run today)
#
# For each backend it reports, as a median over N runs:
#   - TTFT           time to first token (~= prefill of the whole prompt)
#   - prefill tok/s  prompt_tokens / TTFT
#   - decode tok/s   completion_tokens / (total - TTFT)
# then a summary with each backend relative to the first (baseline).
#
# Requires: curl, jaq, perl (all ship with macOS). No Python. bash 3.2 safe.
#
# Usage:
#   scripts/bench-local-inference.sh                 # cold prefill (default)
#   MODE=warm scripts/bench-local-inference.sh       # warm path (cache hits)
#   RUNS=7 MAX_TOKENS=400 scripts/bench-local-inference.sh
#   MODEL_OLLAMA=qwen3.6:27b-mlx scripts/bench-local-inference.sh  # set your tag
#
# Env knobs (all optional):
#   MODE                 cold | warm                       (default cold)
#   RUNS                 timed runs per backend            (default 7)
#   MAX_TOKENS           completion cap per run            (default 500)
#   PROMPT_TOKENS_TARGET approx. size of the prefix        (default 3000)
#   ENDPOINT             llama-swap base URL      (default 192.168.1.240:9292/v1)
#   OLLAMA_ENDPOINT      Ollama OpenAI base URL   (default 192.168.1.240:11434/v1)
#   MODEL_GGUF           llama-swap key for the GGUF model (default qwen3.6-27b-gguf)
#   MODEL_MLX            llama-swap key for the MLX model  (default mlx repo id)
#   MODEL_OLLAMA         Ollama model tag              (default qwen3.6:27b-mlx)
#   BACKENDS             full override: newline-separated "label|endpoint|model"

set -euo pipefail

RUNS="${RUNS:-7}"
MAX_TOKENS="${MAX_TOKENS:-500}"
PROMPT_TOKENS_TARGET="${PROMPT_TOKENS_TARGET:-3000}"
# cold = unique prompt per run (defeats prefix cache, true cold prefill)
# warm = one fixed prompt reused (warmup fills the cache, every run is a hit)
MODE="${MODE:-cold}"

SWAP="${ENDPOINT:-http://192.168.1.240:9292/v1}"
OLLAMA="${OLLAMA_ENDPOINT:-http://192.168.1.240:11434/v1}"

DEFAULT_BACKENDS="Metal (llama.cpp)|$SWAP|${MODEL_GGUF:-qwen3.6-27b-gguf}
MLX (mlx-lm)|$SWAP|${MODEL_MLX:-mlx-community/Qwen3.6-27B-4bit}
Ollama|$OLLAMA|${MODEL_OLLAMA:-qwen3.6:27b-mlx}"
BACKENDS="${BACKENDS:-$DEFAULT_BACKENDS}"

case "${1:-}" in
  -h|--help) sed -n '2,45p' "$0"; exit 0 ;;
esac

case "$MODE" in
  cold|warm) ;;
  *) echo "error: MODE must be 'cold' or 'warm' (got '$MODE')" >&2; exit 1 ;;
esac

for dep in curl jaq awk sort perl; do
  command -v "$dep" >/dev/null 2>&1 || { echo "error: '$dep' not found on PATH" >&2; exit 1; }
done

# High-resolution wall clock (BSD date has no sub-second precision; perl ships
# with macOS). Prints seconds.microseconds.
hires() { perl -MTime::HiRes=time -e 'printf "%.6f", time'; }

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
SYS="$TMP/system.txt"

# --- build a caliban-shaped system prompt: agent instructions + tool schemas,
#     padded with repository-like context to ~PROMPT_TOKENS_TARGET tokens -------
cat > "$SYS" <<'EOF'
You are a coding agent operating inside a large Rust workspace. You plan, edit
files, run shell commands, and verify your work against the repository's tests.
Prefer minimal, well-scoped changes and follow existing conventions. You have
the following tools:

{"name":"read_file","description":"Read a file from the workspace","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}
{"name":"write_file","description":"Create or overwrite a file","parameters":{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}}
{"name":"bash","description":"Run a shell command in the workspace","parameters":{"type":"object","properties":{"cmd":{"type":"string"}},"required":["cmd"]}}
{"name":"grep","description":"Search file contents with a regex","parameters":{"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string"}},"required":["pattern"]}}

Repository context follows (excerpts from the crate you are working in):

EOF

# Pad with representative code-like lines until the file reaches the target size
# (~4 chars/token is a rough estimate; the real prompt_tokens count is read back
# from the server usage and reported, so the estimate only sets the ballpark).
target_chars=$(( PROMPT_TOKENS_TARGET * 4 ))
pad='    let result = compute_metrics(&input, &config)?;  // accumulate rolling stats over the batch window'
while [ "$(wc -c < "$SYS")" -lt "$target_chars" ]; do
  printf '%s\n' "$pad" >> "$SYS"
done

USER_Q="Given the repository context above, explain in two sentences what compute_metrics appears to do, then suggest one concrete improvement."

# build_body <model-id> <stream: true|false> <nonce>
# The nonce is prepended to the system content so every run's sequence differs
# from token 0 — this defeats the server's prefix KV cache and forces a true
# COLD prefill each run (otherwise repeated identical prompts are cache hits and
# TTFT/prefill are meaningless).
build_body() {
  jaq -n --arg model "$1" --rawfile sys "$SYS" --arg user "$USER_Q" \
        --argjson max "$MAX_TOKENS" --argjson stream "$2" --arg nonce "$3" '
    {model: $model,
     messages: [{role:"system", content:($nonce + "\n\n" + $sys)}, {role:"user", content:$user}],
     max_tokens: $max, temperature: 0, stream: $stream}'
}

# median of numbers on stdin (one per line)
median() {
  sort -n | awk '{a[NR]=$1}
    END{ if(NR==0){print "0"}
         else if(NR%2){printf "%.4f\n", a[(NR+1)/2]}
         else {printf "%.4f\n", (a[NR/2]+a[NR/2+1])/2} }'
}

# run_backend <tag> <label> <endpoint> <model-id>  -> 0 on success, 1 to skip
run_backend() {
  local tag="$1" label="$2" endpoint="$3" model="$4"
  echo ">> $label   [$model @ $endpoint]" >&2

  # Warmup: forces a model load/swap and captures prompt_tokens from a
  # non-streaming response (servers return usage there reliably).
  echo "   warming up (model load + first prefill)…" >&2
  # In warm mode, warmup and every run share this fixed marker so the prefix
  # cache is populated once and then hit on every timed run.
  local probe P wnonce
  if [ "$MODE" = warm ]; then wnonce="warm-fixed"; else wnonce="warmup-$RANDOM$RANDOM"; fi
  probe="$(curl -s "$endpoint/chat/completions" -H 'Content-Type: application/json' \
             -d "$(build_body "$model" false "$wnonce")")" \
    || { echo "   request failed — is the server up at $endpoint ?" >&2; return 1; }
  P="$(printf '%s' "$probe" | jaq -r '.usage.prompt_tokens // empty' 2>/dev/null || true)"
  if [ -z "$P" ]; then
    echo "   error: no usage.prompt_tokens. Check the model id / that it is installed." >&2
    printf '   raw: %s\n' "$(printf '%s' "$probe" | head -c 300)" >&2
    return 1
  fi

  : > "$TMP/ttft.$tag"; : > "$TMP/pre.$tag"; : > "$TMP/dec.$tag"
  local i
  for i in $(seq 1 "$RUNS"); do
    local body t0 tf te lines line ttft total C rnonce
    if [ "$MODE" = warm ]; then rnonce="warm-fixed"; else rnonce="run$i-$RANDOM$RANDOM"; fi
    body="$(build_body "$model" true "$rnonce")"
    tf=""; lines=0
    t0="$(hires)"
    # Timestamp the first real SSE data chunk ourselves — curl's TTFB is wrong
    # through a proxy (llama-swap flushes headers before the backend prefills).
    while IFS= read -r line; do
      case "$line" in
        "data: [DONE]"*) : ;;
        "data: "*)
          [ -z "$tf" ] && tf="$(hires)"
          lines=$(( lines + 1 ))
          ;;
      esac
    done < <(curl -sN "$endpoint/chat/completions" \
               -H 'Content-Type: application/json' -d "$body")
    te="$(hires)"
    [ -z "$tf" ] && tf="$te"
    C=$lines   # ~1 token per streamed chunk; leading role chunk ~cancels

    ttft="$(awk -v a="$t0" -v b="$tf" 'BEGIN{printf "%.4f", b-a}')"
    total="$(awk -v a="$t0" -v b="$te" 'BEGIN{printf "%.4f", b-a}')"
    echo "$ttft" >> "$TMP/ttft.$tag"
    awk -v p="$P" -v t="$ttft" 'BEGIN{ if(t<=0)t=0.0001; printf "%.2f\n", p/t }' >> "$TMP/pre.$tag"
    awk -v c="$C" -v tot="$total" -v t="$ttft" \
        'BEGIN{ d=tot-t; if(d<=0)d=0.0001; printf "%.2f\n", c/d }' >> "$TMP/dec.$tag"
    printf "   run %d/%d  ttft=%ss total=%ss  prompt=%s completion≈%s\n" \
      "$i" "$RUNS" "$ttft" "$total" "$P" "$C" >&2
  done

  median < "$TMP/ttft.$tag" > "$TMP/med_ttft.$tag"
  median < "$TMP/pre.$tag"  > "$TMP/med_pre.$tag"
  median < "$TMP/dec.$tag"  > "$TMP/med_dec.$tag"
  echo "$P"     > "$TMP/P.$tag"
  echo "$label" > "$TMP/label.$tag"
  return 0
}

echo "caliban local-inference benchmark"
echo "mode=$MODE  runs=$RUNS  max_tokens=$MAX_TOKENS  prompt≈${PROMPT_TOKENS_TARGET} tok"
echo

# iterate backends; collect the tags that succeed so an unreachable one just
# gets skipped instead of aborting the whole run.
OKTAGS=""
idx=0
while IFS='|' read -r label endpoint model; do
  [ -z "${label:-}" ] && continue
  if run_backend "$idx" "$label" "$endpoint" "$model"; then
    OKTAGS="$OKTAGS $idx"
  else
    echo "   -> skipping $label" >&2
  fi
  idx=$(( idx + 1 ))
done <<EOF
$BACKENDS
EOF

echo
if [ -z "$OKTAGS" ]; then
  echo "no backend produced results — check the servers are up and model ids are correct." >&2
  exit 1
fi

# --- results table ---------------------------------------------------------
printf '%-20s | %8s | %9s | %13s | %12s\n' \
  "backend" "prompt" "TTFT (s)" "prefill tok/s" "decode tok/s"
printf '%-20s-+-%8s-+-%9s-+-%13s-+-%12s\n' \
  "--------------------" "--------" "---------" "-------------" "------------"
for tag in $OKTAGS; do
  printf '%-20s | %8s | %9s | %13s | %12s\n' \
    "$(cat "$TMP/label.$tag")" "$(cat "$TMP/P.$tag")" \
    "$(cat "$TMP/med_ttft.$tag")" "$(cat "$TMP/med_pre.$tag")" "$(cat "$TMP/med_dec.$tag")"
done

# --- summary: each backend relative to the first (baseline) ----------------
base_tag="$(echo "$OKTAGS" | awk '{print $1}')"
echo
echo "Relative to baseline [$(cat "$TMP/label.$base_tag")] (>1 = better):"
for tag in $OKTAGS; do
  [ "$tag" = "$base_tag" ] && continue
  awk -v lb="$(cat "$TMP/label.$tag")" \
      -v ba_ttft="$(cat "$TMP/med_ttft.$base_tag")" -v tt="$(cat "$TMP/med_ttft.$tag")" \
      -v ba_pre="$(cat "$TMP/med_pre.$base_tag")"   -v pr="$(cat "$TMP/med_pre.$tag")" \
      -v ba_dec="$(cat "$TMP/med_dec.$base_tag")"   -v de="$(cat "$TMP/med_dec.$tag")" 'BEGIN{
    printf "  %-20s  TTFT %.2fx   prefill %.2fx   decode %.2fx\n", lb,
      (tt>0 ? ba_ttft/tt : 0), (ba_pre>0 ? pr/ba_pre : 0), (ba_dec>0 ? de/ba_dec : 0);
  }'
done

cat >&2 <<'EOF'

Notes:
  - TTFT is measured at the first SSE token chunk (not curl TTFB, which a proxy
    fakes early); it is the number agent latency actually rides on.
  - MODE=cold (default) prepends a unique marker per run to defeat prefix KV
    caching -> COLD prefill (cache miss). MODE=warm reuses one fixed prompt so
    the warmup fills the cache and every run is a hit -> WARM prefill, the
    multi-turn agent path. Engines with mature prompt caching (llama.cpp slots)
    gain most from warm; decode is unaffected either way. Run both and compare.
  - Ollama on Apple Silicon 0.19+ runs MLX; llama.cpp runs Metal. So this is
    three engine+quant pairs as you would actually run them, not raw kernels.
  - Qwen3.6 is a reasoning model: completion tokens include the thinking stream,
    so decode tok/s reflects real agent decode load. Both are treated the same.
  - completion count is approximate (streamed-chunk count); prompt_tokens is
    exact (server usage). Single stream, no concurrency.
  - If Ollama is skipped: check it is running and MODEL_OLLAMA matches a tag from
    `ollama list` (e.g. qwen3.6:27b).
EOF
