# LMStudio support probe — findings (2026-05-27)

Live probe of caliban's LMStudio support against a hosted LMStudio
instance serving three models. Baseline is `main` at `8b87b35` (the
docs-cleanup merge that landed earlier today).

## Method

- **Live endpoint:** LMStudio on `http://localhost:1234/v1` (OpenAI-compatible).
- **Binary:** `target/release/caliban` built from `main` post-merge.
- **Provider routing:** caliban's native OpenAI provider with
  `OPENAI_BASE_URL=http://localhost:1234/v1`. No `lmstudio` provider
  exists today; LMStudio's OpenAI compatibility is the integration
  surface.
- **Models loaded:**
  - `qwen2.5-coder-7b-instruct-mlx` — non-reasoning, tool-capable
  - `qwen3.5-9b-mlx` — reasoning-mode Qwen3 (README documents as
    multi-turn-tools broken)
  - `google/gemma-4-e4b` — Gemma 4 family
  - `text-embedding-nomic-embed-text-v1.5` — embeddings (irrelevant
    for the agent probe)
- **Scenario battery:** 10 per-model scenarios (chat, single tool,
  multi-step chain, parallel tools, session persistence, MaxTokens,
  bad model name, unreachable endpoint) plus 4 cross-cutting checks
  (malformed URL, `caliban doctor`, `caliban doctor --deep`, qwen3.5
  3-step chain stress).
- **Determinism flags:** every run used `--bare --no-skills --no-mcp
  --no-plugins --no-hooks --no-sub-agent --no-permissions` to strip
  orthogonal subsystems and `--workspace
  /Users/johnford2002/dev/personal/caliban/.claude/worktrees/lmstudio-probe`
  to pin path resolution. Run artifacts are in
  `/tmp/lmstudio-probe/{coder,qwen35,gemma,cross}/*.out`.

## Verdict — what works

| Capability | qwen2.5-coder | qwen3.5 | gemma-4 | Notes |
|---|---|---|---|---|
| Plain chat (non-streaming) | ✅ | ❌ crash | ✅ | qwen3.5 crashes LMStudio-side; caliban surfaces cleanly but mis-categorizes |
| Plain chat (stream-json frames) | ✅ | ❌ crash | ✅ | `system/init` → `message` → `result` frame triplet correct on success |
| Single tool call (Read) | ✅ | ✅ | ✅ | All three dispatch correctly; thinking blocks preserved on Qwen3/Gemma |
| Single tool call (Glob) | ✅ | — | — | 30 matches returned correctly; model later answered "23" — model quality, not caliban |
| Multi-step tool chain (Glob → Read) | ⚠️ hit max_turns | ✅ | ✅ | Qwen3 contradicts README documentation; coder hallucinated fake `[TOOL_RESULT]` blocks |
| Parallel tool calls | ⚠️ partial | — | ❌ | Coder emitted two `tool_use` blocks; only first result observed (max_turns interrupted). Gemma never tried parallel — sequential then max_turns |
| 3-step tool chain | — | ❌ loops | — | Qwen3 re-emits the same `Glob` call across 3 turns; never reaches Read or Grep |
| Reasoning capture (Qwen3 `thinking`, Gemma `thinking`) | n/a | ✅ | ✅ | Post-#63 fix verified end-to-end |
| `OPENAI_BASE_URL` honored | ✅ | ✅ | ✅ | Wire confirmed via response routing |
| `done_reason: stop` mapping | ✅ | ✅ | ✅ | Maps to clean end-of-turn |
| Unreachable endpoint error | ✅ | — | — | Clear `network error: HTTP request failed for url (...)`, exit 1 |

## Issues uncovered

> **Resolution status (2026-05-27).** All findings were addressed by a
> 5-PR series dispatched from this probe. PR → finding mapping:
>
> | PR | Branch | Findings |
> |----|--------|----------|
> | #70 | `fix/headless-reliability` | F1, F7, F12 |
> | #71 | `fix/provider-config-validation` | F2, F3, F4 |
> | #72 | `fix/headless-stream-json` | F8 (documented), F13, F14 (non-bug), F15 |
> | #68 | `fix/agent-core-max-tokens-halt` | F5 |
> | #69 | `fix/probe-quick-wins` | F6, F9, F11 |
>
> Two probe conclusions were overturned during implementation: **F6**
> (the Qwen3 limitation *does* still reproduce — in the thinking
> channel) and **F14** (verbatim concat is correct — not a bug). See
> the correction blocks on those findings. F10 is model-quality, no
> code. A new sub-finding (F16) surfaced during PR #72 — see below.
> #71 and #72 must merge after #70 (shared additive test code in
> `headless/mod.rs`).

### 🐞 F1 — Headless `-p --session` does not persist user/assistant messages (provider-agnostic)

**Where:** `caliban/src/startup.rs:650–664`

This is the same bug as the open ollama-probe F4, now confirmed
provider-agnostic. Reproduced in scenario S7 against the OpenAI
provider with LMStudio backing:

```bash
caliban --provider openai --model qwen2.5-coder-7b-instruct-mlx \
  --bare --no-tools --sessions-dir /tmp/lmstudio-probe-sessions \
  --session lmstest -p "Pick a number between 1 and 10."
# → "7"
caliban [...same flags...] --session lmstest -p "What number did you pick?"
# → "I picked 7." (coincidence — 7 is the default for both turns;
#                   the session file did NOT carry the prior turn)
cat /tmp/lmstudio-probe-sessions/lmstest.json | jq '.messages[].role'
# → "system"  (only)
```

The model's "I picked 7" answer in turn 2 is coincidence (7 is the
most common default both humans and models pick for "1–10"). The
authoritative test is the on-disk session file, which contains only
the system message after both turns — user and assistant content from
either turn never persisted.

**Severity:** Medium. The headline `--session` flow shown in
`README.md:67–84` doesn't actually work via `-p` for any provider.
Promote / update the existing TODO.md entry (currently labeled
"F4 — Ollama probe follow-ups") to note this is now confirmed
provider-agnostic.

---

### 🐞 F2 — Malformed `OPENAI_BASE_URL` surfaces as "OPENAI_API_KEY is not set"

**Where:** `crates/caliban-provider-openai/src/config.rs:58–60`,
flowing into `caliban/src/startup.rs` provider init.

When `OPENAI_BASE_URL` is set but unparseable as a URL, the OpenAI
provider's `DirectConfig::from_env()` returns
`Err(OpenAIError::Transport(...))`. caliban's startup path appears to
treat the failed provider init as "no OpenAI provider available" and
falls through to the API-key-missing error message — even when
`OPENAI_API_KEY` is set:

```bash
OPENAI_API_KEY=lm-studio OPENAI_BASE_URL='not://a:url:!@#' caliban \
  --provider openai --model qwen2.5-coder-7b-instruct-mlx \
  --bare --no-tools -p "ping"
# → "Error: OPENAI_API_KEY is not set — export it, configure `apiKeyHelper`..."
# (the key IS set; the URL parse is what failed)
```

Compare with the valid-key + unreachable-host case, which surfaces
correctly:

```bash
OPENAI_API_KEY=lm-studio OPENAI_BASE_URL='http://this-host-does-not-exist.invalid/v1' \
  caliban [...] -p "ping"
# → "[caliban] run error: network error: HTTP request failed: error sending
#    request for url (http://this-host-does-not-exist.invalid/v1/chat/completions)"
```

This mirrors the ollama F1 silent-fallback pattern: URL parse failures
on provider env vars get swallowed instead of surfacing as
configuration errors.

**Severity:** Low–medium (operator-confusing, surfaces eventually as
the wrong error). **Fix:** match on `Err(OpenAIError::Transport(_))`
from `DirectConfig::from_env()` and propagate it as a distinct
"invalid OPENAI_BASE_URL: ..." error rather than letting the provider
fall out of consideration and producing the key-missing message.

---

### 🐞 F3 — `caliban doctor` only probes Ollama, not the configured provider

**Where:** `caliban/src/diagnostics.rs`

Running `caliban doctor` against an OpenAI-with-LMStudio configuration
returns 8 checks — none of which probe the configured provider:

```
caliban doctor — 8 check(s):
  ! settings — no scope files found — defaults in effect
  ✓ sandbox — tool dispatch goes via caliban-sandbox::SandboxedShim
  ✓ checkpoint_store — /Users/johnford2002/.caliban/projects
  ✓ session_store — ~/Library/Application Support/caliban/sessions (writable)
  ✓ skills — 0 skill(s) loaded (no skill roots present; ...)
  ! claudemd — no CLAUDE.md found in ancestry
  ✓ workspace — <cwd> (writable)
  ✓ ollama — OLLAMA_BASE_URL unset (no probe attempted; use --deep to ping localhost)
```

`--deep` only deepens the Ollama check (`http://localhost:11434/api/tags`
unreachable); it does not probe the configured `OPENAI_BASE_URL` or
verify any other provider's auth. This matches the existing open
ollama F3 in TODO.md — `caliban doctor` was partially closed
(Ollama probe added) but not extended to other providers.

**Severity:** Low (informational; `doctor` is supposed to be the
zero-friction config-validation entry point). **Fix:** generalize the
ollama-probe pattern in `diagnostics.rs` to: detect which providers
are configured (via env vars or settings), then issue cheap probes
per-provider — `GET /v1/models` for OpenAI-compatible endpoints
(including LMStudio + vLLM), `GET /v1/models` for Anthropic, similar
for Google AI Studio. Gate the actual auth/dispatch behind `--deep`.

---

### 🐞 F4 — Silent model substitution against LMStudio

**Where:** caliban request path; LMStudio behavior is the trigger.

LMStudio appears to route requests for unknown model IDs to the
first-loaded model rather than rejecting them. caliban does not
detect the substitution:

```bash
OPENAI_API_KEY=lm-studio caliban --provider openai \
  --model this-model-does-not-exist-foo-bar --bare --no-tools \
  -p "hi"
# → "Hello! How can I assist you today?"
# (exit 0 — no warning that the requested model wasn't used)
```

Operator impact: a typo in `--model` runs against a different model
than intended and the user has no signal.

**Severity:** Low–medium (silent-misconfiguration class; only affects
local OpenAI-compatible servers that route unknown models). **Fix:**
either (a) probe `/v1/models` before the first request and validate
the requested model exists, refusing with a clear error if not, or
(b) compare the OpenAI response's `model` field against the requested
model and emit a one-line warning if they differ. Option (b) is
cheaper and surfaces the issue even when the model list shifts
mid-session.

---

### 🐞 F5 — Agent loop continues past `StopReason::MaxTokens` (LMStudio confirmation of ollama F6)

**Where:** `crates/caliban-agent-core/src/turn.rs:39`, agent loop in
`crates/caliban-agent-core/src/stream/mod.rs`.

Reproduced against `qwen2.5-coder-7b-instruct-mlx` with
`--max-tokens 8`:

```bash
caliban --provider openai --model qwen2.5-coder-7b-instruct-mlx \
  --bare --no-tools --max-tokens 8 --output-format stream-json \
  -p "Count slowly from one to twenty in English, one number per line."
# Frames: two assistant messages, total output_tokens=57, turns=2, exit=0
```

`turn.rs:39` sets `continue_loop = stop_reason == StopReason::ToolUse`,
so a non-ToolUse stop should halt the loop. With `--max-tokens 8`
the per-turn cap should produce ~8 tokens and stop, but the loop
re-issues with what looks like an escalated budget on turn 2 (40
tokens). This matches the existing ollama F6 in TODO.md and is
confirmed not to be Ollama-specific.

**Severity:** Low-medium (UX is poor — small caps get silently
ignored, double-billing tokens; relevant when an operator sets a
tight cap deliberately). **Fix:** update the open F6 entry in
TODO.md to note the LMStudio reproduction; investigate the
continue-loop branch where non-tool-use stop reasons fall through.

---

### 🐞 F6 — README's documented Qwen3 multi-turn-tools limitation is misdescribed (resolved in #69)

> **CORRECTION (post-investigation, PR #69).** My initial probe
> conclusion below — "no longer reproduces" — was wrong. The PR #69
> agent re-ran the chains and the limitation **does still reproduce**:
> the `<tool_call>` XML now appears in the `reasoning_content` /
> `thinking` channel (which caliban began parsing in PR #63), not in
> the `content` field as the README claimed. 2-step chains succeed;
> 3-step chains stall. Same root cause, different surface. PR #69
> rewrote the README section accordingly and stamped it
> "verified 2026-05-27". The original (partly-wrong) probe notes are
> kept below for the record.

**Where:** `README.md:279–326` ("Known model limitations" section).

The README claims that Qwen3-reasoning models on LM Studio break
multi-turn tool use because the model emits Qwen-native `<tool_call>`
XML inside the OpenAI `content` field on the *second* tool call.
Probing today did not reproduce that specific symptom *in the content
field* (it surfaces in the thinking channel instead — see correction
above):

- **Q2 (single Read):** worked end-to-end. `thinking` blocks preserved
  correctly.
- **Q3 (Glob → Read, 2-step chain):** worked end-to-end. Both tool
  calls succeeded; no XML-in-content seen anywhere in the stream.
- **C4 (Glob → Read → Grep, 3-step chain):** failed, but with a
  *different* symptom — the model emitted the same `Glob` tool call
  twice in succession and looped re-planning ("I'll execute these
  steps in sequence: 1. First, use Glob..." repeated) until hitting
  `--max-turns 8`. No XML emission.

The model loaded today is `qwen3.5-9b-mlx`, the same name referenced
in the README. Either LMStudio has shipped a Qwen3 tool-call-adapter
fix, the model file itself has been updated to behave differently, or
the original probe captured a transient state.

**Severity:** Low (docs-only). **Fix:** re-verify the limitation
section before the next release. If the XML-in-content symptom truly
no longer reproduces, replace the section with the current reality —
"Qwen3 reasoning models on LM Studio re-plan repeatedly on chains
longer than 2 tool calls, hitting `--max-turns` without progressing."
Or remove the section entirely if 2-step chains are reliable and
3-step is rare enough.

---

### 🐞 F7 — `subtype: max_turns` result frame returns concatenated assistant text instead of a clear "agent gave up" signal

**Where:** headless result-frame emit path; ADR 0025 spec.

When the agent loop hits `--max-turns` without a clean terminal
state, the `result` frame's `subtype` is `"max_turns"` but the
`result` string is the concatenation of every assistant text
fragment emitted across the truncated run. In several probes this
produced output ranging from "stuck repeating the same plan
preamble" (qwen3.5 C4) to an empty string (gemma G4 — `result:""`
despite 703 output tokens generated). Consumers parsing the result
field can't easily distinguish "model gave a useful partial answer"
from "model looped without progressing" from "no observable output".

**Severity:** Low (UX). **Fix:** when `subtype == "max_turns"`,
emit a structured payload instead of free-form concatenation:
`{"reason": "max_turns_exceeded", "turn_count": N, "last_assistant_text": "...", "tool_calls_seen": M}`.
Leaves the existing `result` field for the success path; failure
paths get a typed signal that callers can act on.

---

### 📝 F8 — Stream-json emits both a short `tool_use` frame and a `tool_use` block inside the `message` frame (CONFIRMED INTENTIONAL — documented in #72)

> **RESOLUTION (PR #72).** Confirmed intentional and now documented in
> ADR 0025: short frames are progress indicators; the `message` frame
> is the authoritative full assistant turn (mirrors Claude Code's
> `assistant` event). Consumers should read the `message` frame and not
> double-count. A regression test
> (`tool_use_appears_in_short_frame_and_message_frame`) pins the
> contract.

**Where:** headless emit path in `caliban/src/headless/`.

Across multiple successful tool-call runs, the frame sequence is:

```
{"type":"tool_use","id":"X","name":"Glob","input":{...}}                    ← short frame
{"type":"tool_result","tool_use_id":"X","is_error":false,"content":[...]}   ← short frame
{"type":"message","role":"assistant","content":[...,{"id":"X","input":{...},"name":"Glob","type":"tool_use"}]}  ← duplicated tool_use inside message
```

The `tool_use` for the same call appears twice — once as a top-level
short frame (with full `input` populated, post-#66), and again as a
`tool_use` content block nested inside the assistant `message` frame.
Consumers may double-count if they aren't aware. Likely intentional
(matches Claude Code's frame stream where the `message` frame is the
authoritative record), but worth either documenting in ADR 0025 or
deduplicating at the headless layer.

**Severity:** Informational — verify against ADR 0025's emit
contract.

---

### 📝 F9 — qwen3.5-9b crashed on a trivial chat prompt (LMStudio-side); caliban classifies as "invalid request"

**Where:** LMStudio model crash; caliban error categorization in
`crates/caliban-provider-openai/src/`.

Q1 reproduction:

```bash
OPENAI_API_KEY=lm-studio OPENAI_BASE_URL=http://localhost:1234/v1 \
  caliban --provider openai --model qwen3.5-9b-mlx --bare --no-tools \
  --max-tokens 256 --output-format stream-json \
  -p "Reply with exactly the single word: pong"
# → {"type":"result","subtype":"error","result":"","error":
#    "invalid request: The model has crashed without additional information. (Exit code: null)"}
```

The error envelope IS extracted correctly (matches the upstream-error
SSE-body handling that PR #62 closed). But the error category is
`"invalid request"` even though the upstream message says the model
crashed — that's not an invalid request, it's a server-side fault.

**Severity:** Low (only affects how the error is displayed; the run
correctly halts with exit 1). **Fix:** when the extracted upstream
message contains "crashed" / "exit code" / similar server-fault
markers, route to a new `UpstreamServerFault` variant rather than
`InvalidRequest`. Optional and low priority — the message itself
already says "model has crashed" verbatim.

---

### 📝 F10 — qwen2.5-coder fabricates `[TOOL_RESULT]...[END_TOOL_RESULT]` text blocks instead of calling tools (model quality, not caliban)

In scenario S5 (Glob → Read chain), `qwen2.5-coder-7b-instruct-mlx`
mid-run emitted a literal text block of the form:

```
[TOOL_RESULT]
/Users/johnford2002/dev/personal/caliban/.claude/worktrees/lmstudio-probe/rust-project/Cargo.toml:10:channel = "stable"
[END_TOOL_RESULT]
```

Pretending to have called a tool. The fabricated path doesn't exist
in the workspace, and the real `Read` result earlier in the run
correctly returned `channel = "1.95.0"`. caliban happily passes the
fabricated text through to the user.

**Severity:** None — model quality issue, not caliban behavior.
Noted here in case the pattern recurs across more 7B models and
becomes worth a content-side filter at the tool-result layer.

---

### 🐞 F11 — `--continue` with no prior session silently runs a fresh ephemeral session

**Where:** session-resume path in `caliban/src/startup.rs` (or the
`-c` / `--continue` handler).

```bash
mkdir -p /tmp/empty-sessions
caliban --provider openai --model qwen2.5-coder-7b-instruct-mlx \
  --bare --no-tools --sessions-dir /tmp/empty-sessions --continue \
  --max-tokens 32 -p "hi"
# → "Hello! How can I assist you today?"
# exit 0; no warning that there was no session to continue
```

The flag means "resume the most recently updated session". With an
empty sessions directory there is no session to resume; the expected
behavior is to error with something like "no prior session in
/tmp/empty-sessions" and exit non-zero. Instead caliban silently
creates a new ephemeral session and runs the prompt. An operator who
typo'd `--session foo` as `--continue` (or expected a resumable
session that wasn't actually persisted, see F1) gets confusing
behavior — their "continuation" actually starts from scratch with no
indication.

**Severity:** Low. **Fix:** in the `--continue` handler, after
listing sessions in `--sessions-dir`, if the list is empty return
`HeadlessError` (or equivalent) with "no prior session to continue
in <dir>" and a non-zero exit code. Optional: print the directory
that was searched so the operator can investigate (matches the
ergonomics of `git status` when there's no repo).

---

### 🐞 F12 — Exit code 130 for `max_turns_exceeded` collides with the conventional SIGINT exit code

**Where:** headless exit-status mapping in `caliban/src/headless/`
or the main binary exit handler.

Reproduced via scenario E6 (`--max-turns 1` with a tool-using
prompt). The agent emits a clean `result` frame with `subtype:
"max_turns"`, then the process exits with status 130:

```bash
caliban --provider openai --model qwen2.5-coder-7b-instruct-mlx \
  --bare --no-skills --no-mcp --no-plugins --no-hooks --no-sub-agent \
  --no-permissions --workspace . --max-turns 1 --max-tokens 256 \
  --output-format stream-json \
  -p "Use the Read tool to read README.md and tell me the first line."
# Frames: tool_use → tool_result → message → result(subtype=max_turns)
# Then: process exits with status 130
```

130 is the conventional shell exit code for "killed by SIGINT"
(128 + signal-2). An operator (or CI script) inspecting `$?` sees
130 and reasonably concludes "user hit Ctrl-C" — but no one did.
The agent hit its `--max-turns` cap; it wasn't interrupted. CI
pipelines that distinguish "user cancelled" from "agent gave up"
get the wrong category.

For comparison, clean runs exit 0 (S1, S3, E1, E3) and provider
errors exit 1 (S10, C1, E2's variant, Q1).

**Severity:** Low-medium (CI / orchestrator confusion). **Fix:**
emit a distinct exit code for `max_turns_exceeded` that doesn't
collide with signal conventions. Suggested values: `2` (already
used by clap for argument-parse errors — would collide), `64`–`78`
range (sysexits.h territory — `EX_TEMPFAIL=75` is a reasonable
fit), or a caliban-specific code documented in ADR 0025. Pairs
naturally with F7 (structured `max_turns` result payload) — the
exit code and the result frame's `subtype` should agree.

---

### 🐞 F13 — `--input-format stream-json` + `-p "-"` interaction silently swallows malformed input frames

**Where:** `caliban/src/headless/input.rs:45–53` (`parse_input_line`)
and the `-p "-"` stdin-prompt handler.

The documented `InputFrame` shape (per
`caliban/src/headless/input.rs:107` test) is the simple form:

```
{"type":"user","content":"hi"}
```

E4 fed a Claude-Code-style structured user message
(`{"type":"user","message":{"role":"user","content":[{"type":"text","text":"..."}]}}`)
via stdin with `--input-format stream-json --output-format stream-json -p "-"`.
Result: no parse error surfaced; caliban ran the agent with what
looks like an empty or default prompt:

```json
{"type":"message","role":"assistant","content":[{"text":"Hello! It seems you've encountered a blank command. How can I assist you today? ..."}]}
```

Two possible failure paths, both bugs:
1. `parse_input_line` should reject the malformed frame with
   `HeadlessError::InputParse`, but didn't fire — meaning the
   stdin contents were never actually parsed against the
   `InputFrame` enum.
2. `-p "-"` precedence: when `-p` is given a literal `-`, caliban
   may be reading stdin as plain text for the prompt (ignoring
   `--input-format stream-json`). If so, the two flags' interaction
   is unclear / undocumented and probably warrants a refusal at
   parse time ("`--input-format stream-json` requires omitting `-p`
   or passing `-p -`; received conflicting `-p` arg").

Either way, the operator combining these flags gets no signal that
the input wasn't consumed as intended.

**Severity:** Low-medium (only operators using the bidirectional
headless protocol hit this). **Fix:** clarify the conflict at clap
level: when `--input-format` is `stream-json`, require `--prompt -`
(or no prompt at all) and route stdin through `parse_input_line`
unconditionally. Surface any `HeadlessError::InputParse` on stderr
+ exit non-zero before invoking the agent. Add a documented
contract in ADR 0025 for the input-frame shape.

---

### 📝 F14 — stream-json `result.result` concatenation (NOT A BUG — verified in #72)

> **CORRECTION (post-investigation, PR #72).** This is not a caliban
> bug. The aggregation does a verbatim `push_str` concat with no
> whitespace inserted or dropped. The "feelfree" splice was the model
> emitting two adjacent tokens with no separator — caliban faithfully
> concatenates them. PR #72 added two regression tests
> (`result_field_verbatim_concats_adjacent_text_deltas` and
> `result_field_preserves_whitespace_at_chunk_boundary`) pinning the
> contract. No code change. Original notes kept below for the record.

**Where:** headless result-aggregation path in
`caliban/src/headless/` (`final_text.push_str(&text)`).

Observed in E4: the model emitted two assistant text frames in
sequence:

```
{"type":"message","role":"assistant","content":[{"text":"...feel"}]}
{"type":"message","role":"assistant","content":[{"text":"free to let me know!"}]}
```

The final `result` frame's `result` field concatenated them
literally:

```
"result":"...feelfree to let me know!"
```

— "feel" + "free" → "feelfree", joined with no separator. Likely
either a tokenization-boundary split that dropped the inter-chunk
space, or a deliberate `+`-concat that doesn't reconstruct the
intended whitespace. Operators parsing `result` literally see
mangled prose whenever the model emits split text frames.

For comparison, the *frame-by-frame* stream is correct (each
`message` frame stands alone); only the aggregated `result` is
broken.

**Severity:** Low (only affects consumers that read `result`
instead of streaming frames; the streaming frames themselves are
right). **Fix:** confirm the aggregation logic: it should
concatenate raw frame text verbatim if frames are token-boundary
splits, OR insert a single space between consecutive
text-fragment frames if frames are sentence/word-boundary splits.
The former is closer to OpenAI / Anthropic semantics where
streaming tokens already include leading/trailing whitespace as
appropriate. If the input frames really did contain "feel"
(without trailing space) and "free" (without leading space), the
upstream model likely produced that literal sequence — the bug
would then be in the *streaming* layer dropping a token, not the
aggregation. Worth one focused debugging pass against a known
prompt that triggers split frames reproducibly.

---

### 📝 F15 — Headless `-p` auto-allows tool dispatch without `--no-permissions` (intentional but undocumented)

**Where:** permission-mode dispatch in headless mode; ADR 0029.

E5 ran a tool-using prompt under headless `-p` WITHOUT
`--no-permissions` and without any explicit `--permission-mode`,
`--allow`, or `--ask`. The 4 Read tool calls dispatched cleanly
with `is_error:false`; no permission prompts, no auto-denials, no
hang waiting for interactive confirmation.

This is reasonable behavior for headless mode — there's no TTY for
an interactive prompt and waiting on stdin would deadlock the
operator. But it's worth two notes:

1. **README inaccuracy:** the docs imply (`README.md:231`) that
   `/permissions` and permission modes are central; in headless
   mode the user gets a permissive default by accident if they
   don't pass `--no-permissions`. The trip wire is invisible.
2. **Surprising-default risk:** an operator who wanted to be
   prompted (e.g., piping caliban into a UI that proxies
   permission prompts via `--permission-prompt-tool`) may not
   notice that tools were auto-allowed. The result frame doesn't
   indicate the permission mode actually used.

**Severity:** Low (intentional behavior; documentation /
transparency issue). **Fix:** include `permission_mode` in the
`system/init` frame's payload alongside `tools`, `plugins`, etc.
so a consumer can see the effective mode at session start. Add a
sentence to ADR 0029 + the README explaining that headless `-p`
without explicit permission flags defaults to bypass (or whatever
the effective default actually is — needs code-side confirmation).

> **Resolved in #72.** `permission_mode` added to the `system/init`
> frame; ADR 0029 documents the headless default. Correction to the
> probe's framing: the effective mode is `PermissionMode::Default`
> (NOT bypass). The "auto-allow" the probe observed is the
> default-rules tail Allowing read-only tools (Read/Glob/Grep). See
> F16 for the consequence on write tools.

---

### 🐞 F16 — Headless `-p` Write/Edit/Bash without `--auto-allow` fails on first call (surfaced during #72, NOT yet fixed)

**Where:** permission resolution for headless mode; `caliban-agent-core::permissions` + the headless dispatch path.

Surfaced while documenting F15. Headless `-p` resolves to
`PermissionMode::Default`, whose rule tail **Asks** for `Write` /
`Edit` / `Bash`. In a non-interactive context there's no one to
answer the `Ask`, so it resolves to a hard deny — meaning a headless
prompt that needs to write a file or run a command will **fail on the
first such tool call** unless the operator passes `--auto-allow` (or
an explicit `--allow` pattern). Read-only tools (Read/Glob/Grep) are
Allowed by the default tail, which is why the F15/E5 probe saw tools
"just work" — it only exercised read tools.

**Severity:** Medium (silent failure of a whole tool class in the
headline headless mode). **Fix (proposed):** decide the intended
headless default — either (a) document loudly that headless `-p`
needs `--auto-allow` for mutating tools and emit a clear error
("tool X requires --auto-allow or an --allow rule in headless mode")
instead of an opaque deny, or (b) make headless `-p` default to a
more permissive mode for the workspace-scoped mutating tools. Pairs
with F15's `permission_mode` surfacing — once the mode is visible in
`system/init`, the failure is at least diagnosable. Not addressed by
any PR in this series; needs its own.

---

## Bug priorities (suggested)

1. **F1** (headless session persistence broken, provider-agnostic) —
   already in TODO.md; promote/expand to note LMStudio confirmation.
2. **F12** (exit code 130 for `max_turns_exceeded` collides with
   SIGINT) — small CI / orchestrator confusion; one-line fix in the
   exit-status mapping; pairs naturally with F7.
3. **F3** (`caliban doctor` only probes Ollama) — informational
   tracker; expand the existing F3 entry in TODO.md to cover all
   providers.
4. **F2** (malformed `OPENAI_BASE_URL` → wrong error) — small
   ergonomic fix, single match arm in startup.
5. **F4** (silent model substitution on LMStudio) — operator-trust
   issue; easy to surface via response-model comparison.
6. **F5** (continue-loop past MaxTokens) — already in TODO.md as
   ollama F6; note LMStudio reproduction.
7. **F13** (`--input-format stream-json` + `-p "-"` interaction
   silently swallows malformed input) — clap-level conflict rule + a
   parse-error path that actually fires.
8. **F11** (`--continue` with no prior session silently runs fresh)
   — small error-path fix; one extra check after listing sessions.
9. **F14** (stream-json `result` field concatenation mangles text:
   "feel" + "free" → "feelfree") — investigation first, then either
   verbatim concat or whitespace-aware join.
10. **F6** (README Qwen3 limitation no longer reproduces) — docs
    refresh required before next release.
11. **F7** (`max_turns` result subtype emits raw concatenation) — UX
    polish.
12. **F15** (headless `-p` auto-allows tools without
    `--no-permissions`) — intentional behavior; surface in
    `system/init` frame + ADR 0029 wording.
13. **F8** (stream-json `tool_use` frame duplication) — verify intent
    in ADR 0025; document or dedupe.
14. **F9** (qwen3.5 crash classified as invalid_request) — error
    classification polish.
15. **F10** — informational only (model fabrication, not caliban).

## Detailed run log

Probe artifacts (raw stream-json frames, command lines, exit codes)
live in `/tmp/lmstudio-probe/`:

- `coder/s{1..10}*.out` — qwen2.5-coder scenario battery
- `qwen35/q{1..3}*.out` — qwen3.5 chat / single / multi-step
- `gemma/g{1..4}*.out` — gemma-4 chat / single / multi-step / parallel
- `cross/c{1..4}*.out` — malformed URL, doctor, doctor --deep, qwen3.5
  3-step stress
- `extra/e{1..8}*.out` — follow-up probes (tool error recovery,
  `--continue` empty session, `--restrict-paths`, `--input-format
  stream-json`, `-p` without `--no-permissions`, `--max-turns 1`,
  `--temperature` edge cases, missing `OPENAI_API_KEY`)
