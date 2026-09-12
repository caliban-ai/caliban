#!/usr/bin/env bash
#
# conformance-local-inference.sh — do local backends BEHAVE like caliban needs?
#
# The perf benchmark (bench-local-inference.sh) answers "how fast". This answers
# "does it work at all as a caliban backend" — the capability surface previously
# vetted against Ollama (see docs/evaluation/probes/*), asserted against the exact
# OpenAI-compatible wire shapes caliban's provider parses.
#
# Runs a battery per backend and prints a PASS/FAIL/WARN matrix. Defaults to the
# three .240 engines (llama-swap Metal + MLX, and Ollama direct).
#
# Requires: curl, jaq. No Python. bash 3.2 safe.
#
# Usage:
#   scripts/conformance-local-inference.sh
#   MODEL_OLLAMA=qwen3.6:27b-mlx scripts/conformance-local-inference.sh
#
# Status legend:
#   PASS  behaves as caliban requires
#   FAIL  breaks a caliban requirement (P0) or a real correctness contract
#   WARN  model-quality / optional gap, not a caliban bug (e.g. model declined a tool)
#   NA    not applicable (a prior step didn't produce the precondition)
#
# P0 tests (must pass or caliban cannot drive the backend): TOOLns, JSONargs,
# TOOLstream, NOLEAK, STOP, LENGTH, ROUNDTRIP.  P1: USAGE, REASONING.

set -euo pipefail

SWAP="${ENDPOINT:-http://192.168.1.240:9292/v1}"
OLLAMA="${OLLAMA_ENDPOINT:-http://192.168.1.240:11434/v1}"

DEFAULT_BACKENDS="Metal (llama.cpp)|$SWAP|${MODEL_GGUF:-qwen3.6-27b-gguf}
MLX (mlx-lm)|$SWAP|${MODEL_MLX:-mlx-community/Qwen3.6-27B-4bit}
Ollama|$OLLAMA|${MODEL_OLLAMA:-qwen3.6:27b-mlx}"
BACKENDS="${BACKENDS:-$DEFAULT_BACKENDS}"

case "${1:-}" in -h|--help) sed -n '2,33p' "$0"; exit 0 ;; esac
for dep in curl jaq; do
  command -v "$dep" >/dev/null 2>&1 || { echo "error: '$dep' not found on PATH" >&2; exit 1; }
done

TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT

# ordered test ids (columns) and their short headers
TESTS="TOOLns JSONargs TOOLstream NOLEAK STOP LENGTH ROUNDTRIP USAGE REASONING"

TOOLS='[{"type":"function","function":{"name":"get_weather","description":"Get the current weather for a city","parameters":{"type":"object","properties":{"location":{"type":"string","description":"City name"}},"required":["location"]}}}]'
TOOLPROMPT="What is the current weather in Paris? Use the get_weather tool to find out."

post()        { curl -s  "$1/chat/completions" -H 'Content-Type: application/json' -d "$2" -o "$3" 2>/dev/null; }
post_stream() { curl -sN "$1/chat/completions" -H 'Content-Type: application/json' -d "$2" -o "$3" 2>/dev/null; }

# SSE payload lines (drops "data: " prefix, [DONE] and blanks)
sse() { grep '^data: ' "$1" | sed 's/^data: //' | grep -v -e '^\[DONE\]' -e '^$'; }

chat_body() { # model user max stream
  jaq -n --arg m "$1" --arg u "$2" --argjson max "$3" --argjson s "$4" \
    '{model:$m,messages:[{role:"user",content:$u}],max_tokens:$max,temperature:0,stream:$s}'
}
tool_body() { # model user stream
  jaq -n --arg m "$1" --arg u "$2" --argjson s "$3" --argjson tools "$TOOLS" \
    '{model:$m,messages:[{role:"user",content:$u}],tools:$tools,tool_choice:"auto",max_tokens:512,temperature:0,stream:$s}'
}

CUR_TAG=""
record() { # testid status note
  printf '%s|%s' "$2" "$3" > "$TMP/res.$CUR_TAG.$1"
  printf '   %-11s %-4s %s\n' "$1" "$2" "$3" >&2
}

run_conformance() { # tag label endpoint model
  local tag="$1" label="$2" ep="$3" model="$4"
  CUR_TAG="$tag"
  echo ">> $label   [$model @ $ep]" >&2

  # reachability + warmup
  if ! post "$ep" "$(chat_body "$model" "ping" 8 false)" "$TMP/ping.json" \
     || ! jaq -e '.choices[0] // .error' "$TMP/ping.json" >/dev/null 2>&1; then
    echo "   unreachable or invalid response — skipping" >&2
    local t; for t in $TESTS; do printf 'SKIP|unreachable' > "$TMP/res.$tag.$t"; done
    return 1
  fi

  # --- T1 tool call (non-streaming) + T2 valid-JSON args -----------------
  post "$ep" "$(tool_body "$model" "$TOOLPROMPT" false)" "$TMP/t1.json"
  local name fr args
  name="$(jaq -r '.choices[0].message.tool_calls[0].function.name // empty' "$TMP/t1.json" 2>/dev/null || true)"
  fr="$(jaq -r '.choices[0].finish_reason // empty' "$TMP/t1.json" 2>/dev/null || true)"
  if [ -n "$name" ]; then
    record TOOLns PASS "tool_calls[0].name=$name finish_reason=$fr"
    args="$(jaq -r '.choices[0].message.tool_calls[0].function.arguments // empty' "$TMP/t1.json" 2>/dev/null || true)"
    if printf '%s' "$args" | jaq -e . >/dev/null 2>&1; then
      record JSONargs PASS "arguments parse as JSON: $(printf '%s' "$args" | head -c 48)"
    else
      record JSONargs FAIL "arguments NOT valid JSON: $(printf '%s' "$args" | head -c 48)"
    fi
    [ "$fr" = "tool_calls" ] || record TOOLns WARN "tool_calls present but finish_reason=$fr (expected tool_calls)"
  else
    record TOOLns WARN "no tool_calls emitted (model declined or unsupported); finish_reason=$fr"
    record JSONargs NA "no tool call to validate"
  fi

  # --- T3 tool call (streaming) ------------------------------------------
  post_stream "$ep" "$(tool_body "$model" "$TOOLPROMPT" true)" "$TMP/t2.txt"
  local sname sargs
  sname="$(sse "$TMP/t2.txt" | jaq -r 'try (.choices[0].delta.tool_calls[0].function.name // empty) catch empty' 2>/dev/null | grep -m1 . || true)"
  sargs="$(sse "$TMP/t2.txt" | jaq -r 'try (.choices[0].delta.tool_calls[0].function.arguments // empty) catch empty' 2>/dev/null | tr -d '\n')"
  if [ -n "$sname" ]; then
    if printf '%s' "$sargs" | jaq -e . >/dev/null 2>&1; then
      record TOOLstream PASS "streamed tool_calls reassemble to valid JSON (name=$sname)"
    else
      record TOOLstream FAIL "streamed arguments fragments do NOT reassemble to valid JSON"
    fi
  else
    record TOOLstream WARN "no streamed tool_calls emitted"
  fi

  # --- T4 no tool-XML leak (the LM Studio MLX failure) -------------------
  local blob
  blob="$( { jaq -r '[.choices[0].message.content,.choices[0].message.reasoning_content,.choices[0].message.reasoning]|map(select(.!=null))|join(" ")' "$TMP/t1.json" 2>/dev/null || true
            sse "$TMP/t2.txt" | jaq -r 'try ([.choices[0].delta.content,.choices[0].delta.reasoning_content,.choices[0].delta.reasoning]|map(select(.!=null))|join(" ")) catch empty' 2>/dev/null || true; } )"
  case "$blob" in
    *"<tool_call>"*|*"<function="*|*"</think>"*|*"<|python_tag|>"*)
      record NOLEAK FAIL "tool-call markup leaked into text/reasoning (needs tool-call/reasoning parser)" ;;
    *)
      record NOLEAK PASS "no tool-call markup in content/reasoning" ;;
  esac

  # --- T5 finish_reason=stop halts --------------------------------------
  post "$ep" "$(chat_body "$model" "Reply with a one sentence greeting." 2048 false)" "$TMP/t4.json"
  fr="$(jaq -r '.choices[0].finish_reason // "MISSING"' "$TMP/t4.json" 2>/dev/null || echo MISSING)"
  [ "$fr" = "stop" ] && record STOP PASS "finish_reason=stop" || record STOP FAIL "expected stop, got '$fr'"

  # --- T6 finish_reason=length respects the cap -------------------------
  post "$ep" "$(chat_body "$model" "Write a 500 word essay about the ocean." 8 false)" "$TMP/t5.json"
  fr="$(jaq -r '.choices[0].finish_reason // "MISSING"' "$TMP/t5.json" 2>/dev/null || echo MISSING)"
  local ct; ct="$(jaq -r '.usage.completion_tokens // -1' "$TMP/t5.json" 2>/dev/null || echo -1)"
  if [ "$fr" = "length" ]; then
    if [ "$ct" -ge 0 ] 2>/dev/null && [ "$ct" -le 32 ] 2>/dev/null; then
      record LENGTH PASS "finish_reason=length, completion_tokens=$ct (cap respected)"
    else
      record LENGTH WARN "finish_reason=length but completion_tokens=$ct (cap loosely respected)"
    fi
  else
    record LENGTH FAIL "expected length, got '$fr' (completion_tokens=$ct — cap ignored?)"
  fi

  # --- T7 multi-turn tool round-trip ------------------------------------
  if [ -n "$name" ]; then
    local tcid ast rt err c
    tcid="$(jaq -r '.choices[0].message.tool_calls[0].id // "call_1"' "$TMP/t1.json")"
    ast="$(jaq -c '.choices[0].message' "$TMP/t1.json")"
    rt="$(jaq -n --arg m "$model" --argjson ast "$ast" --arg tcid "$tcid" --arg u "$TOOLPROMPT" \
          '{model:$m,max_tokens:512,temperature:0,stream:false,
            messages:[{role:"user",content:$u},$ast,
                      {role:"tool",tool_call_id:$tcid,content:"{\"tempC\":19,\"sky\":\"clear\"}"}]}')"
    post "$ep" "$rt" "$TMP/t7.json"
    err="$(jaq -r '.error // empty' "$TMP/t7.json" 2>/dev/null | head -c 80 || true)"
    c="$(jaq -r '.choices[0].message.content // .choices[0].message.reasoning_content // .choices[0].message.reasoning // empty' "$TMP/t7.json" 2>/dev/null || true)"
    if [ -n "$err" ]; then record ROUNDTRIP FAIL "server rejected tool-result turn: $err"
    elif [ -n "$c" ]; then record ROUNDTRIP PASS "accepted tool result, produced a follow-up turn"
    else record ROUNDTRIP WARN "tool-result turn returned no content"; fi
  else
    record ROUNDTRIP NA "no initial tool call to feed back"
  fi

  # --- T8 usage accounting ----------------------------------------------
  local pt cc
  pt="$(jaq -r '.usage.prompt_tokens // -1' "$TMP/t4.json" 2>/dev/null || echo -1)"
  cc="$(jaq -r '.usage.completion_tokens // -1' "$TMP/t4.json" 2>/dev/null || echo -1)"
  if [ "$pt" -ge 0 ] 2>/dev/null && [ "$cc" -ge 0 ] 2>/dev/null; then
    record USAGE PASS "prompt_tokens=$pt completion_tokens=$cc"
  else
    record USAGE WARN "usage not reported (accounting only; non-fatal)"
  fi

  # --- T9 reasoning field name (streaming path caliban uses) -------------
  post_stream "$ep" "$(chat_body "$model" "Briefly, why is the sky blue?" 2048 true)" "$TMP/t9.txt"
  local drc drr
  drc="$(sse "$TMP/t9.txt" | jaq -r 'try (.choices[0].delta.reasoning_content // empty) catch empty' 2>/dev/null | grep -m1 . || true)"
  drr="$(sse "$TMP/t9.txt" | jaq -r 'try (.choices[0].delta.reasoning // empty) catch empty' 2>/dev/null | grep -m1 . || true)"
  if [ -n "$drc" ]; then
    record REASONING PASS "streams reasoning_content (caliban parses it)"
  elif [ -n "$drr" ]; then
    record REASONING FAIL "streams 'reasoning', not 'reasoning_content' — caliban DROPS thinking (needs a server flag or an adapter alias)"
  else
    record REASONING NA "no streamed reasoning field (model not thinking, or none exposed)"
  fi

  return 0
}

echo "caliban local-inference conformance"
echo

OKTAGS=""
idx=0
while IFS='|' read -r label endpoint model; do
  [ -z "${label:-}" ] && continue
  echo "$label" > "$TMP/label.$idx"
  run_conformance "$idx" "$label" "$endpoint" "$model" || true
  OKTAGS="$OKTAGS $idx"
  idx=$(( idx + 1 ))
  echo >&2
done <<EOF
$BACKENDS
EOF

# --- matrix ---------------------------------------------------------------
code() { case "$1" in PASS) echo "P";; FAIL) echo "F";; WARN) echo "W";; NA) echo "-";; SKIP) echo "x";; *) echo "?";; esac; }

echo
printf '%-20s' "backend"
for t in $TESTS; do printf ' %-10s' "$t"; done
printf '\n'
printf '%-20s' "--------------------"
for t in $TESTS; do printf ' %-10s' "----------"; done
printf '\n'
for tag in $OKTAGS; do
  printf '%-20s' "$(cat "$TMP/label.$tag")"
  for t in $TESTS; do
    st="$(cut -d'|' -f1 "$TMP/res.$tag.$t" 2>/dev/null || echo '?')"
    printf ' %-10s' "$(code "$st")"
  done
  printf '\n'
done

cat <<'EOF'

Legend: P=pass  F=fail  W=warn(model-quality/optional)  -=n/a
P0 (must pass): TOOLns JSONargs TOOLstream NOLEAK STOP LENGTH ROUNDTRIP
P1 (correctness/accounting): USAGE REASONING

Reading it:
  - Any F in a P0 column = that backend cannot reliably drive caliban as-is.
  - NOLEAK=F or REASONING=F on an MLX backend usually means mlx_lm.server needs
    --tool-call-parser / --reasoning-parser flags (or caliban needs a 'reasoning'
    alias). See docs/adr/0056 + docs/evaluation/probes/.
  - W on tool tests is often the model declining to call — rerun or use a
    tool_choice of "required" if your server supports it; not a caliban bug.
EOF
