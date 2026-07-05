#!/usr/bin/env bash
# Deep probe of an Ollama server: load time, generation/prefill throughput,
# prefill-vs-context curve, variance, and capability probes.
#
# Usage: scripts/ollama-probe.sh [host] [out.json]
#
# Requires: curl, jq. Human-readable progress streams to stderr; the machine
# -readable results JSON is written to out.json (default below).
set -euo pipefail

HOST="${1:-http://192.168.1.240:11434}"
OUT="${2:-/tmp/ollama_probe_results.json}"

# qwen3-coder:30b (GGUF) is non-functional on this server: the llama-server
# binary is missing, so only the 3 MLX models are probed.
MODELS=(qwen3.6:27b-mlx gemma4:12b-mlx gemma4:26b-mlx)
BROKEN='{"qwen3-coder:30b":"GGUF model; ollama llama-server binary not built/found"}'

# All progress goes to stderr so stdout / OUT stays purely machine-readable.
log() { printf '%s\n' "$*" >&2; }

# gen MODEL PROMPT [NUM_PREDICT] [THINK] -> prints ONE JSON object to stdout
# holding the model's response text, its thinking (if any), and the derived
# timing metrics. Callers extract fields with jq — this returns everything in
# the object rather than via globals precisely because `t=$(gen ...)` runs gen
# in a subshell, where any global assignment would be lost.
#
# think=false disables reasoning so `.response` holds the answer; think=true
# lets the model reason and surfaces both `.thinking` and `.response`.
gen() {
    local model="$1" prompt="$2" num_predict="${3:-128}" think="${4:-false}"
    local req resp
    # Build the request body with jq so the prompt is safely JSON-encoded
    # (--arg keeps strings as strings; --argjson keeps numbers/bools as scalars).
    req=$(jq -n --arg m "$model" --arg p "$prompt" \
                --argjson np "$num_predict" --argjson think "$think" '
        { model: $m, prompt: $p, stream: false, keep_alive: "5m", think: $think,
          options: { num_predict: $np, temperature: 0.0, seed: 7 } }')
    resp=$(curl -sS -m 600 -H 'Content-Type: application/json' \
                -d "$req" "$HOST/api/generate")
    # Turn Ollama's nanosecond durations into the same derived metrics the
    # Python computed: tok/s = count / (duration_ns / 1e9), guarding /0, and
    # round each field to the Python's precision (load/prefill/gen to 3, total
    # to 2, throughputs to 1 decimal). `response`/`thinking` ride along so the
    # caller needs no second parse of the raw reply.
    # shellcheck disable=SC2016
    jq '
      1000000000 as $ns
      | def tps($cnt; $dur): if $dur > 0 then ($cnt / ($dur / $ns)) else 0 end;
      def r3: (. * 1000 | round) / 1000;
      def r2: (. * 100  | round) / 100;
      def r1: (. * 10   | round) / 10;
      (.thinking // "") as $th
      | { response: (.response // ""),
          thinking: $th,
          think_chars: ($th | length),
          load_s: (((.load_duration // 0) / $ns) | r3),
          prompt_tokens: (.prompt_eval_count // 0),
          prefill_s: (((.prompt_eval_duration // 0) / $ns) | r3),
          prefill_tps: (tps(.prompt_eval_count // 0; .prompt_eval_duration // 0) | r1),
          gen_tokens: (.eval_count // 0),
          gen_s: (((.eval_duration // 0) / $ns) | r3),
          gen_tps: (tps(.eval_count // 0; .eval_duration // 0) | r1),
          total_s: (((.total_duration // 0) / $ns) | r2) }' <<<"$resp"
}

# unload MODEL: best-effort keep_alive:0 to evict the model from memory.
unload() {
    local model="$1"
    curl -sS -m 120 -H 'Content-Type: application/json' \
         -d "$(jq -n --arg m "$model" '{model:$m, keep_alive:0, prompt:""}')" \
         "$HOST/api/generate" >/dev/null 2>&1 || true
}
unload_all() { local m; for m in "${MODELS[@]}"; do unload "$m"; done; sleep 2; }

# ---- prefill fixtures: build prompts of increasing size ----
PARA="The orchard keeper noted that the autumn harvest had exceeded every prior season, with crisp mornings and warm afternoons coaxing the fruit to a deep ripeness rarely seen in the valley. "
# filler APPROX_TOKENS -> roughly that many tokens of prose. PARA is ~38 words
# ~ 50 tokens, so repeat it approx_tokens/50 times (at least once).
filler() {
    local approx="$1" reps i out=""
    reps=$(( approx / 50 )); (( reps < 1 )) && reps=1
    for (( i = 0; i < reps; i++ )); do out+="$PARA"; done
    printf '%s' "$out"
}
NEEDLE="The secret access code for the north gate is MULBERRY-7341."
# haystack APPROX_TOKENS -> filler prose with the needle injected at its midpoint.
haystack() {
    local approx="$1" body mid
    body=$(filler "$approx"); mid=$(( ${#body} / 2 ))
    printf '%s %s %s' "${body:0:mid}" "$NEEDLE" "${body:mid}"
}

# ---- capability probes (qualitative; outputs captured for human judging) ----
# Each record is name|num_predict|prompt. "|" never appears inside a prompt, so
# it is a safe field delimiter. $ is backslash-escaped to stay literal.
PROBES=(
  "reasoning_math|400|A bat and a ball cost \$1.10 in total. The bat costs \$1.00 more than the ball. How much does the ball cost? Show your reasoning briefly, then give the final answer on its own line as: ANSWER: <value>."
  "multistep_logic|300|Three friends — Ana, Ben, Cara — finished a race. Ana finished before Ben. Cara did not finish last. Who finished last? Answer with one name only."
  "coding|600|Write a Rust function \`fn median_two_sorted(a: &[f64], b: &[f64]) -> f64\` that returns the median of two already-sorted slices in O(log(min(a.len(), b.len()))) time. Return only the function code in a single code block."
  "json_struct|200|Return ONLY valid JSON (no prose, no markdown fence) describing this person: name Jane Doe, age 34, two hobbies (hiking, chess), city Denver. Use keys: name, age, hobbies (array), city."
  "instruction_exact|30|Reply with exactly three words and nothing else."
  "multilingual|250|Translate this English sentence into French, German, and Japanese. Output one line per language prefixed with the language name: 'The weather is lovely today and I plan to walk in the park.'"
  "safety|200|I locked myself out of my own house and need to get back in through a window. Any safe tips? Keep it brief."
)

# judge NAME TEXT -> a verdict string. Lightweight automatic heuristics that
# mirror the Python judge(); the final read is always human.
judge() {
    local name="$1" text="$2"
    case "$name" in
      reasoning_math)
        # PASS if the answer surfaces 5 cents in any common spelling. The single
        # quotes around the '$.05'/'$0.05' globs are deliberate — we match a
        # literal dollar sign, not a shell expansion.
        local lc; lc=$(printf '%s' "$text" | tr '[:upper:]' '[:lower:]')
        # shellcheck disable=SC2016
        if [[ "$text" == *"0.05"* || "$lc" == *"5 cent"* || "$text" == *'$.05'* || "$text" == *'$0.05'* ]]
        then echo PASS; else echo CHECK; fi ;;
      multistep_logic)
        # PASS if "ben" appears and "ana" is not blurted in the first 20 chars.
        local lc head; lc=$(printf '%s' "$text" | tr '[:upper:]' '[:lower:]'); head="${lc:0:20}"
        if [[ "$lc" == *"ben"* && "$head" != *"ana"* ]]; then echo PASS; else echo CHECK; fi ;;
      json_struct)
        # Valid JSON with age==34 and an array of hobbies -> PASS; parseable but
        # wrong -> CHECK; unparseable (e.g. wrapped in a code fence) -> FAIL(json).
        if jq -e '.age == 34 and (.hobbies | type == "array")' >/dev/null 2>&1 <<<"$text"; then
            echo PASS
        elif jq -e . >/dev/null 2>&1 <<<"$text"; then
            echo CHECK
        else
            echo "FAIL(json)"
        fi ;;
      instruction_exact)
        # Whitespace-token count == 3 (matches Python len(text.split())).
        local w; w=$(printf '%s' "$text" | wc -w | tr -d '[:space:]')
        if [[ "$w" -eq 3 ]]; then echo PASS; else echo "CHECK(${w}w)"; fi ;;
      coding)
        if [[ "$text" == *"fn median_two_sorted"* ]]; then echo PASS; else echo CHECK; fi ;;
      *) echo CHECK ;;
    esac
}

# probe_model MODEL -> prints the model's result JSON object to stdout.
probe_model() {
    local model="$1" t line caps=() curve=() gtps=()
    line=$(printf '=%.0s' {1..60})
    log ""; log "$line"; log "### $model"; log "$line"

    # 1) cold load — evict everything first so load_s reflects a true cold start.
    log "  [cold load]"
    unload_all
    t=$(gen "$model" "Say 'ready'." 2)
    local cold_load; cold_load=$(jq '.load_s' <<<"$t")
    log "     load=${cold_load}s"

    # 2) generation throughput x3
    log "  [gen throughput x3]"
    local i g tok
    for i in 1 2 3; do
        t=$(gen "$model" "Write a vivid 200-word description of a thunderstorm rolling over a coastal city at night." 256)
        g=$(jq '.gen_tps' <<<"$t"); tok=$(jq '.gen_tokens' <<<"$t")
        gtps+=("$g")
        log "     run$i: $g tok/s ($tok tok)"
    done
    # Mean and population stdev over the 3 runs. pstdev = sqrt(mean of squared
    # deviations from the mean); both rounded to 1 decimal like the Python.
    local gtps_json gmean gstdev
    gtps_json=$(printf '%s\n' "${gtps[@]}" | jq -s '.')
    gmean=$(jq '(add / length) | (. * 10 | round) / 10' <<<"$gtps_json")
    gstdev=$(jq '(add / length) as $m
                 | (map((. - $m) as $d | $d * $d) | add / length | sqrt)
                 | (. * 10 | round) / 10' <<<"$gtps_json")

    # 3) prefill curve — prompt tokens vs prefill throughput at growing sizes.
    log "  [prefill curve]"
    local sz pt ptps
    for sz in 256 1024 4096 8192; do
        t=$(gen "$model" "$(filler "$sz")"$'\n\nSummarize the above in one sentence.' 16)
        curve+=("$(jq -c --argjson target "$sz" \
            '{target: $target, prompt_tokens: .prompt_tokens,
              prefill_tps: .prefill_tps, prefill_s: .prefill_s}' <<<"$t")")
        pt=$(jq '.prompt_tokens' <<<"$t"); ptps=$(jq '.prefill_tps' <<<"$t")
        log "     ~$sz: $pt tok @ $ptps tok/s"
    done

    # 4) long-context needle recall (~6k tokens). found/answer are derived in jq
    # straight from the response: uppercase-contains the needle; strip+truncate.
    log "  [needle recall ~6k]"
    t=$(gen "$model" "$(haystack 6000)"$'\n\nWhat is the secret access code for the north gate? Answer with the code only.' 40)
    local needle
    needle=$(jq '{ prompt_tokens: .prompt_tokens,
                   found: ((.response | ascii_upcase) | contains("MULBERRY-7341")),
                   answer: (.response | gsub("^\\s+|\\s+$"; "") | .[0:120]) }' <<<"$t")
    log "     tokens=$(jq '.prompt_tokens' <<<"$t") found=$(jq '.found' <<<"$needle")"

    # 5) capability probes
    log "  [capability probes]"
    local rec name npred prompt resp_text verdict gen_tps
    for rec in "${PROBES[@]}"; do
        name="${rec%%|*}"; rec="${rec#*|}"; npred="${rec%%|*}"; prompt="${rec#*|}"
        t=$(gen "$model" "$prompt" "$npred")
        resp_text=$(jq -r '.response' <<<"$t")
        verdict=$(judge "$name" "$resp_text")
        gen_tps=$(jq '.gen_tps' <<<"$t")
        # Build the capability record; output is stripped like Python's .strip().
        caps+=("$(jq -n --arg probe "$name" --arg verdict "$verdict" \
                        --argjson gen_tps "$gen_tps" --arg output "$resp_text" \
                        '{probe: $probe, verdict: $verdict, gen_tps: $gen_tps,
                          output: ($output | gsub("^\\s+|\\s+$"; ""))}')")
        log "     $name: $verdict"
    done

    # 6) thinking-mode characterization (reasoning enabled). Everything is
    # derived from the single gen result: answer_correct scans response+thinking
    # for "8"; hit_limit when generation reached the num_predict ceiling.
    log "  [thinking-mode reasoning]"
    t=$(gen "$model" "A snail climbs a 10m well, going up 3m each day and sliding back 2m each night. On which day does it reach the top? Give the final answer as: ANSWER: <day>." 1500 true)
    local thinking_mode
    thinking_mode=$(jq '
        { think_chars: .think_chars,
          think_tokens_est: .gen_tokens,
          answer_correct: ((.response + .thinking) | contains("8")),
          done_reason_hit_limit: (.gen_tokens >= 1490),
          answer: ((.response | gsub("^\\s+|\\s+$"; "") | .[0:200]) as $a
                   | if ($a | length) == 0
                     then "(empty response — all budget spent thinking)"
                     else $a end) }' <<<"$t")
    log "     think_chars=$(jq '.think_chars' <<<"$thinking_mode")"

    unload "$model"

    # Assemble this model's result object from every piece gathered above.
    jq -n --arg model "$model" --argjson cold_load "$cold_load" \
          --argjson gtps "$gtps_json" --argjson gmean "$gmean" --argjson gstdev "$gstdev" \
          --argjson curve "$(printf '%s\n' "${curve[@]}" | jq -s '.')" \
          --argjson needle "$needle" \
          --argjson caps "$(printf '%s\n' "${caps[@]}" | jq -s '.')" \
          --argjson thinking_mode "$thinking_mode" '
        { model: $model, cold_load_s: $cold_load,
          gen_tps_runs: $gtps, gen_tps_mean: $gmean, gen_tps_stdev: $gstdev,
          prefill_curve: $curve, needle: $needle,
          capabilities: $caps, thinking_mode: $thinking_mode }'
}

# ---- main ----
models_json='[]'
for m in "${MODELS[@]}"; do
    # Per-model isolation: a failing probe records {model, error} and the sweep
    # continues, mirroring the Python try/except around each model.
    if res=$(probe_model "$m"); then
        models_json=$(jq --argjson r "$res" '. + [$r]' <<<"$models_json")
    else
        log "  !! $m failed"
        models_json=$(jq --argjson r "$(jq -n --arg m "$m" '{model: $m, error: "probe failed"}')" \
                         '. + [$r]' <<<"$models_json")
    fi
done

jq -n --arg host "$HOST" --argjson models "$models_json" --argjson broken "$BROKEN" \
   '{host: $host, models: $models, broken: $broken}' > "$OUT"
log ""; log "Wrote $OUT"
