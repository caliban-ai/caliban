# LM Studio probe findings — 2026-05-25

> **2026-05-25, mid-session correction.** Source readings used to author
> Findings 1-4 below were stale relative to the actual branch. The
> repo had shipped 53 commits since the source I had originally read,
> including a `caliban-common` foundation crate and a unified
> `caliban-settings` layer. After re-baselining I confirmed:
>
> - **Finding 1 is already implemented in the current source.**
>   Caliban's streaming OpenAI request does send
>   `stream_options: {include_usage: true}`. Probe-captured traffic
>   confirms this. Strike Finding 1 from the fix list.
> - **Finding 5 (new and critical), below**, supersedes the
>   "empty assistant turn" speculation that was tentatively folded
>   into Findings 2 and 4. The actual root cause of every empty-turn
>   failure we saw was a silently-swallowed HTTP 400 from LM Studio
>   ("context length exceeded"), not the streaming parser dropping
>   deltas.
> - Findings 2, 3, and 4 remain valid as written; their severity may
>   be lower in practice now that Finding 5 explains the worst-looking
>   symptoms.

> **2026-05-25, validation pass (post-cleanup-sprint).** Re-checked
> all 10 findings against current `main` (`e3c4b93` — after the
> Tier 1 + Tier 2 splits + tools-builtin grouping merged). Results:
>
> - **Finding 1's earlier "already fixed" correction was itself wrong.**
>   `crates/caliban-provider-openai/src/ir_convert.rs:254` still reads
>   `stream_options: None`. The probe traffic the original correction
>   relied on must have been against a different binary. Finding 1 is
>   **open**.
> - **Findings 2, 4, 5, 6, 7, 9** confirmed open in current source.
> - **Finding 8** line refs shifted: the second `emit_init()` site
>   moved from `caliban/src/main.rs:910` to `caliban/src/startup.rs:525`
>   (the `main.rs` god-file split — PR-T2-B — extracted the startup
>   pipeline). `caliban/src/headless/mod.rs:301` unchanged.
> - **Finding 10** line refs shifted from `caliban/src/main.rs:759-781`
>   to `caliban/src/startup.rs:374-383` (same T2-B split). The
>   single-frame `break;` is still present.
> - **MCP probe cluster correction** (existing note at "Probe cluster:
>   MCP") remains accurate; current `main` has Phases A + B + C
>   merged.
>
> Net: all 10 findings are still actionable. Finding 1's status flips
> back to **open**; the table at the bottom of this doc has been
> corrected.

> **2026-05-25, fix pass.** Findings 1 and 6 are now **fixed** on
> branch `jf/fix/openai-stream-and-completion-tokens`.
> `ir_to_native_request` sets
> `stream_options: Some(StreamOptions { include_usage: true })` whenever
> `stream` is true (Finding 1) and routes the token cap to
> `max_completion_tokens` when the model is in the GPT-5 / o-series
> family — matched by a new `uses_completion_tokens(model)` helper that
> matches `gpt-5*`, `o1*`, `o3*`, `o4*` case-insensitively (Finding 6).
> Six net new tests cover the model-family branch + stream-options
> shape. Status table updated below.


Findings from probing caliban against LM Studio (model: `qwen3.5-9b-mlx`) using
the `--provider openai` path. Each finding is a candidate for a follow-up
fix pass; severity and suggested fix are noted inline.

Probe environment:

- caliban built from `main` at the time of probing (`target/release/caliban`).
- LM Studio listening at `http://localhost:1234/v1`, serving `qwen3.5-9b-mlx`
  (a Qwen-family reasoning model that emits separate `reasoning_content`).
- Run shape: `OPENAI_API_KEY=lm-studio OPENAI_BASE_URL=http://localhost:1234/v1
  caliban --provider openai --model qwen3.5-9b-mlx ...`

---

## Finding 1 — Streaming token usage is never requested from OpenAI-compatible providers

**Severity:** low-medium (cosmetic in TUI + downstream blind spot for token-aware compaction).

**Where:** `crates/caliban-provider-openai/src/ir_convert.rs:247`
(`NativeRequest { ..., stream_options: None, .. }`).

**Symptom.** Every TUI run against LM Studio (and OpenAI directly, when
streaming) shows `[caliban: N turns · 0↑ 0↓ tokens]`. The
`UsageSummary` line surfaces zeroes even when the underlying API would
return real numbers.

**Root cause.** Per the OpenAI Chat Completions streaming spec, the
final `usage` object is included in the stream **only if** the request
sets `stream_options.include_usage: true`. caliban currently sends
`stream_options: None` for every streaming request. As a result, the
streamed chunk has no `usage` field, `stream_parse.rs:191-199` builds
`usage_delta = None`, and the agent's `total_usage` stays at zero.

**Evidence.** Direct `curl` to LM Studio without streaming returns full
usage:

```
"usage": {
  "prompt_tokens": 12,
  "completion_tokens": 7,
  "total_tokens": 19,
  "completion_tokens_details": { "reasoning_tokens": 7 }
}
```

Direct `curl` with `"stream": true` and `stream_options:
{include_usage: true}` also returns usage in the terminal chunk.
caliban's streamed request omits the option, so it gets nothing.

**Suggested fix.** In `ir_to_native_request` (or its `stream=true`
branch), populate `stream_options` when `stream` is true. The
`NativeRequest` schema already carries a `stream_options:
Option<StreamOptions>` field — verify the struct exists and add it if
not. One- or two-line change.

**Downstream impact.** Any compactor that decides whether to truncate
based on the `Usage` field (e.g., `DropOldestCompactor` reading
`history` token estimates against `capabilities.max_input_tokens`) is
fine today because it uses the local `chars/4` heuristic
(`crates/caliban-agent-core/src/compact.rs:32`). But anything that
chooses to consume the provider-reported `Usage` directly (e.g., a
future "compact when we've spent N tokens" rule) is silently broken
for streaming OpenAI routes.

---

## Finding 2 — `reasoning_content` from Qwen/DeepSeek-style reasoning models is silently dropped

**Severity:** medium-high for reasoning models; zero impact for non-reasoning models.

**Where:**

- `crates/caliban-provider-openai/src/schema/events.rs:36-49`
  (`NativeDelta` has no `reasoning_content` field).
- `crates/caliban-provider-openai/src/stream_parse.rs:99-133`
  (delta handler only inspects `content`, `refusal`, `tool_calls`).

**Symptom.** When caliban talks to a reasoning model that emits
`reasoning_content` (Qwen3.x reasoning variants, DeepSeek-R1, and
similar), the model's thinking trace is invisible in caliban — no
transcript entry, no streamed event, no debug-log trace. Worse, those
reasoning tokens **still count against `max_tokens`**: the model can
burn its whole budget reasoning and return empty `content` with
`finish_reason: "length"`. From caliban's perspective that looks like
the model "returned nothing"; the user sees a blank assistant turn
and no signal as to why.

**Evidence.** Direct `curl` against `qwen3.5-9b-mlx` with
`max_tokens=8`:

```
"message": { "role": "assistant", "content": "",
             "reasoning_content": "Thinking Process:\n\n1. ", ... },
"finish_reason": "length",
"usage": { ..., "completion_tokens": 7,
           "completion_tokens_details": { "reasoning_tokens": 7 } }
```

All seven completion tokens were reasoning tokens; `content` was empty;
caliban would have shown nothing.

**Root cause.** The OpenAI streaming `delta` schema in caliban only
models `content`/`tool_calls`/`refusal`. Serde silently drops unknown
fields, so the reasoning stream is parsed-and-thrown-away. The IR
already has a `StreamingContentType::Thinking` channel and a
corresponding `(thinking) ...` rendering in the TUI
(`caliban/src/tui.rs:1271`) wired for Anthropic extended-thinking;
nothing on the OpenAI side opens it.

**Suggested fix.** Mirror the existing text-block handling but for a
new reasoning channel:

1. Add `reasoning_content: Option<String>` to `NativeDelta` in
   `schema/events.rs` (and to the non-streaming `NativeMessage` if we
   ever want to consume reasoning from non-streamed responses).
2. In `stream_parse.rs`, treat the first reasoning delta as a
   `ContentBlockStart { content_type: StreamingContentType::Thinking }`
   block; subsequent reasoning deltas yield
   `StreamingDelta::Thinking(text)`; close the block when a
   `content`/`tool_call` delta or `finish_reason` arrives.
3. Decide the channel-switching rule explicitly: the reasoning block
   should close (emit `ContentBlockStop`) before opening the text
   block so the IR stays well-formed. The existing text/tool-call
   switch at line 138-144 is the template.

**Open question for the fix pass.** Some providers (notably DeepSeek)
return `reasoning_content` interleaved with `content`; others emit all
reasoning first. The handler needs to close-then-reopen rather than
assume a single contiguous reasoning block. Worth a property test
against both orderings.

---

## Notes on probe environment / unrelated observations

- TTFT on the first request to LM Studio (cold load) was ~20s for
  `qwen3.5-9b-mlx` on this machine; subsequent calls should be much
  faster. Not a caliban issue, but worth remembering when interpreting
  latency numbers in future probes.
- LM Studio's `chat/completions` endpoint reports usage correctly for
  non-streamed requests (see Finding 1 evidence). Streaming usage is
  the specific blind spot.
- `capabilities_for` in `crates/caliban-provider-openai/src/models.rs:81`
  returns `max_input_tokens = 128_000` for any unrecognized model,
  including all LM Studio models. Today this is latent (default
  compactor is `NoopCompactor`) but it will become a footgun the moment
  someone configures a non-noop compactor with a local model. Not
  added to the fix list as a "must do" but flagging it here for the
  follow-up pass.

---

## Status

| # | Finding                                                | Severity      | Suggested area         | Status |
|---|--------------------------------------------------------|---------------|------------------------|--------|
| **1** | **Streaming usage not requested (`stream_options.include_usage`)** | low-med  | `provider-openai` (ir_convert.rs:254) | **fixed** (jf/fix/openai-stream-and-completion-tokens — `stream_options: Some({include_usage: true})` when `stream=true`) |
| 2 | `reasoning_content` dropped from reasoning models      | med-high      | `provider-openai`      | open   |
| 3 | Leading-newline cosmetic noise in qwen3.5 responses    | cosmetic      | (model quirk; opt'l)   | noted  |
| 4 | Qwen-XML tool calls in follow-up turns aren't parsed   | medium (qwen) | `provider-openai`      | open (severity downgraded after Finding 5) |
| **5** | **HTTP errors from streaming providers swallowed silently** | **high** | **`caliban` bin / `agent-core` runloop** | **fixed** (jf/fix/surface-stopped-for — TUI surfaces `RunEnd.stopped_for` as `[caliban: …]` transcript line + red toast for `ProviderError`/`HookDenied`/`CompactionFailed`; neutral info line for `MaxTurnsReached`/`Cancelled`) |
| **6** | **GPT-5 / o-series reject `max_tokens` — caliban never sends `max_completion_tokens`** | **high** | **`provider-openai`** (ir_convert.rs:247-248) | **fixed** (jf/fix/openai-stream-and-completion-tokens — `uses_completion_tokens(model)` routes `gpt-5*`/`o1*`/`o3*`/`o4*` to `max_completion_tokens`) |
| **7** | **Streaming `usage` chunk dropped — token counts always show 0** | **medium** | **`provider-openai`** (stream_parse.rs:97-108) | **open** |
| **8** | **Duplicate `system/init` frame in stream-json output**         | **low-med**| **`caliban` bin / headless driver** (headless/mod.rs:301 + startup.rs:525) | **fixed** (jf/fix/headless-dedupe-init — dropped external `emit_init` in startup.rs; `HeadlessDriver::run` now drains hook buffer right after the canonical `emit_init` so frame order is preserved) |
| **9** | **Headless mode swallows provider errors identically to TUI**   | **high** (= Finding 5 in headless) | **`caliban` bin / headless driver** (headless/mod.rs:457-475) | **fixed** (jf/fix/surface-stopped-for — `RunEnd` arm maps `ProviderError`/`HookDenied`/`CompactionFailed` to `subtype:"error"` + populated `error` field + exit 1, mirroring the schema-validation path from H-9) |
| **10**| **`--input-format stream-json` consumes only the first user frame** | **medium** | **`caliban` bin** (startup.rs:374-383) | **open** |
| - | `capabilities_for` fallback is 128k for unknown models | latent footgun| `provider-openai`      | noted  |

Further findings from probes 2-4 (multi-turn history retention,
tool-use correctness, skill loading) will be appended below as they
emerge.

---

## Probe 2 — Multi-turn history retention (PASS)

Two-turn session probe via `--session probe-history`:

- Turn 1: `"My favorite color is teal. Remember this. Reply in one short sentence."`
  → `"I've noted that your favorite color is teal."`
- Turn 2: `"What is my favorite color?"` → `"Your favorite color is teal."`

Persisted session contains the full conversation (5 messages:
`[system, user, assistant, user, assistant]`). The earlier
"every message treated as the first" symptom does **not** reproduce
with `qwen3.5-9b-mlx`. The prior incident was most likely a different
LM Studio model overflowing its loaded context window, not a caliban
history-management bug.

Side observations from this probe:

- **TTFT cold vs warm**: turn 1 ≈ 18.7s (cold model load); turn 2 ≈ 4.3s.
  Big drop once the model is resident — useful baseline for interpreting
  future probe latencies.
- **Token counts persisted as zero in the session JSON**
  (`total_usage.input_tokens: 0`, `output_tokens: 0`). Direct
  corroboration of Finding 1.
- **Finding 3 (new, cosmetic)**: every assistant response from
  `qwen3.5-9b-mlx` starts with a literal `\n` (visible in the persisted
  session content). Causes a stray blank line above every assistant
  turn in both the CLI and the TUI. Likely a chat-template quirk of
  this specific Qwen variant; not strictly a caliban bug. Trivial
  mitigation (trim leading whitespace from streamed text once we know
  it's the model's own preamble, not part of the content) but worth
  surfacing here.
- Memory auto-index splicing is **working**: the persisted system
  message starts with `<auto-memory-index path="...">`, confirming
  `caliban-memory::load` is being invoked and spliced into the default
  prompt as designed.

---

## Probe 3 — Tool-use correctness (MIXED — first call OK, follow-up broken)

Prompt asked the model to locate the file defining default permission
rules using whatever tool(s) it picked. Two observations:

**Good:** The first tool call from `qwen3.5-9b-mlx` is **schema-correct**.
The model called `Grep` with a sensible `pattern` and no
`include`-as-path confusion (the earlier failure mode from the
prior probe session). The Grep dispatched, returned matches, and
the IR captured a proper `tool_use` block.

**Broken:** The **follow-up turn** (after the `tool_result` lands)
returns an empty `assistant_message.content: []`. Both `max_tokens=1024`
and `max_tokens=4096` reproduce this. The agent loop sees
`stop_reason == Stop` (not `ToolUse`), exits the loop, and the user
sees no output at all.

Replaying the exact conversation against LM Studio directly (both
non-streamed and streamed) revealed the cause:

### Finding 4 — Qwen-native `<tool_call>` XML in follow-up turns isn't parsed

**Severity:** high for Qwen-family reasoning models (qwen3.x reasoning
variants). Zero impact for non-Qwen OpenAI-shaped providers.

**Symptom.** After the first tool result message, qwen3.5-9b-mlx
serializes its next tool call as Qwen-native XML inside the OpenAI
`content` field:

```
<tool_call>
<function=Glob>
<parameter=path>/Users/johnford2002/dev/personal/caliban</parameter>
<parameter=pattern>**/*.yaml</parameter>
</function>
</tool_call>
```

It does NOT populate OpenAI's `tool_calls` array. caliban (correctly
following the OpenAI spec) parses the text as content, doesn't see a
`tool_calls` field, gets `finish_reason: "stop"`, and ends the turn
without dispatching anything.

This is the model's chat template handling tool-result follow-ups via
its native protocol rather than the OpenAI normalization path. LM
Studio passes the model's output through verbatim — it doesn't rewrite
`<tool_call>` XML into the OpenAI `tool_calls` array. So caliban gets
a text-only response that *looks* like a tool call to a human reader
but isn't one structurally.

**Evidence.** Non-streamed replay against `/v1/chat/completions` with
the same conversation returned:
- `finish_reason: "stop"`,
- `content` of 168 chars containing the `<tool_call>` XML,
- `reasoning_content` of 264 chars,
- no `tool_calls` field.

Streamed replay (SSE) showed the same pattern: many
`delta.reasoning_content` chunks first, then `delta.content` chunks
emitting the XML, then `finish_reason: "stop"` with no
`delta.tool_calls`.

**Mitigation options, ordered by effort:**

1. **Document as "model unsupported."** Tell users which LM Studio
   models are known to misbehave with caliban's OpenAI-spec dispatcher
   (qwen3 reasoning variants). Cheap and honest.
2. **Detect-and-rewrite at the stream layer.** Add a post-parser
   pass that scans accumulated text for `<tool_call>...</tool_call>`
   blocks and synthesizes a tool_use IR block from them. Adds a
   regex/parsing step but recovers multi-turn tool flow for Qwen
   models on LM Studio.
3. **Configure LM Studio's tool-call adapter.** LM Studio has
   per-model JSON config controlling how it normalizes tool calls.
   It may be possible to tell LM Studio to emit OpenAI-style
   `tool_calls` for qwen3.5; if so, this becomes a one-time setup
   step, not a caliban code change.

**Interaction with Finding 2.** Even with `reasoning_content`
handled, this Qwen-XML-in-content problem would still break
multi-turn tool use on qwen3.5. The two are independent — both need
addressing if we want first-class Qwen support, but they don't share
a fix.

**Refinement (from probe 4):** Finding 4 manifests specifically on
*follow-up tool calls* after a `tool_result`. Text-only responses
after a `tool_result` work fine — see probe 4 where the model
produced normal `delta.content` with a complete prose answer after
two `Skill` invocations. So the bug surface is narrower than first
suspected: it's the second-or-later tool dispatch in a chain that
breaks, not all post-tool-result content.

**Side note worth investigating.** In caliban's persisted session,
the failed follow-up assistant message was stored with
`content: []` — completely empty — even though the SSE stream
contained `content` deltas with the `<tool_call>` text. The
non-streamed replay populated content correctly. It's possible
caliban's streaming `MessageAccumulator` is dropping the text deltas
under some condition (perhaps because reasoning_content deltas
arrive first and the accumulator's content-block bookkeeping gets
confused). Worth a focused look during the fix pass; tentatively
folded into Finding 2 since the trigger is the same model.

---

## Probe 4 — Skill loading and invocation (PASS)

Setup: a clean workspace at `/tmp/probe-skill-workspace/` with a
single skill at `.caliban/skills/teal-poem/SKILL.md` declaring a
poem template with a distinctive completion marker.

Invocation: `caliban --workspace /tmp/probe-skill-workspace ...` to
re-root skill discovery. Prompt nudged the model toward the skill
without naming it directly.

**Results:**

- ✅ **Discovery works from a non-cwd workspace.** Setting
  `--workspace` correctly rooted `default_roots` (Layer 0 of
  `caliban-skills::default_roots`) at the probe workspace and the
  skill was found.
- ✅ **Model invoked the `Skill` tool with the correct exact
  name**: `{"name": "teal-poem"}`. No spelling/casing issues.
- ✅ **Skill body is delivered to the model in context.** The
  tool result included the full SKILL.md body verbatim, including
  the `[teal-poem complete]` marker.
- ✅ **Multi-turn after tool result works for text-only follow-up.**
  After the Skill tool result, the model emitted text content (the
  poem) and ended cleanly — no Finding 4-style XML wrapping. This
  is what refined Finding 4 above to "follow-up *tool calls*" rather
  than "follow-up *content*".

**Minor wart:** the model called `Skill(name="teal-poem")` *twice*
back-to-back, fetching the same skill body twice. The second call
was redundant since the body was already in context. Pure model
inefficiency, not a caliban issue, but it does mean a skill body
counts toward context twice when this happens.

**Skill fidelity is a model concern.** The skill body said "exactly
5 words per line"; the model produced 6-7 words per line. The
constraint was visible in context but not followed. This is a
qwen3.5-9b capability ceiling, not a caliban defect — but worth
remembering when authoring skills: stricter constraints will be
ignored by smaller local models more often than by frontier ones.

---

## Finding 5 — HTTP errors from streaming providers are silently swallowed

**Severity:** high. Cause of every "empty assistant turn" symptom seen
across probes. Affects all OpenAI-compatible providers.

**Where:**

- The provider error path returns `OpenAIError::BadStatus` from
  `crates/caliban-provider-openai/src/transport/direct.rs:79-85` when
  LM Studio returns a non-2xx status.
- The agent runloop in `crates/caliban-agent-core/src/stream.rs:640-650`
  catches this as `Err(e)`, sets
  `stopped_for = StopCondition::ProviderError(e.to_string())`, and
  `break 'outer;`. The loop then yields `TurnEvent::RunEnd` with the
  `stopped_for` field populated.
- **No visible surface in the binary.** `run_and_render` in
  `caliban/src/main.rs` consumes `RunEnd` but only prints the
  turns/tokens summary line; the `stopped_for` field is dropped on
  the floor for every variant. Exit code is `0`.

**Symptom (as a user).** A run finishes "successfully": exit 0, the
session JSON is saved with whatever messages preceded the failed turn,
and the CLI prints nothing. The user has no signal that anything
went wrong — they just get silence.

**Evidence.** Captured caliban's request via a proxy when probing
sub-agents against `qwen2.5-coder-7b-instruct-mlx`:

- Request: 13.3 KB body, 17 tool definitions, `stream_options.include_usage: true`.
- Replaying the request to LM Studio directly returned
  **HTTP 400** with body
  `{"error":"The number of tokens to keep from the initial prompt is greater than the context length. Try to load the model with a larger context length, or provide a shorter input"}`.
- The same request against `qwen3.5-9b-mlx` (loaded with a larger
  context window) returned HTTP 200 with a valid `tool_calls` response.
  So the request is well-formed; qwen2.5-coder's loaded context is
  simply smaller than caliban's request.

**Suggested fix.** In `caliban/src/main.rs` (and equivalent paths in
the TUI), surface non-`EndOfTurn` stop conditions explicitly:

```rust
match stopped_for {
    StopCondition::EndOfTurn => {}
    StopCondition::ProviderError(msg) =>
        eprintln!("[caliban: provider error: {msg}]"),
    StopCondition::HookDenied(msg) =>
        eprintln!("[caliban: hook denied: {msg}]"),
    StopCondition::CompactionFailed(msg) =>
        eprintln!("[caliban: compaction failed: {msg}]"),
    StopCondition::Cancelled =>
        eprintln!("[caliban: cancelled]"),
    StopCondition::MaxTurnsReached(n) =>
        eprintln!("[caliban: max-turns ({n}) reached]"),
}
```

And consider returning non-zero from `main()` when `stopped_for` is
an error variant (`ProviderError`, `HookDenied`, `CompactionFailed`)
so scripts can detect failure.

**Downstream effects this finding retroactively explains.**

- Probe 3 turn 1's "empty assistant turn" (originally folded into
  Finding 4 as a Qwen-XML mystery) is most likely just this: the
  request grew once the Grep result was appended, exceeded LM
  Studio's loaded context, returned 400, and got swallowed.
- All "0 turns / 0 tokens" outputs during sub-agent probing reduce
  to context-overflow + this silent-swallow combination.
- The earlier-session "model treats every message as the first"
  story we discussed before any of this probing started looks
  consistent with the same root cause + cumulative session growth
  pushing each turn over the LM Studio limit.

**Related downgrades:**

- **Finding 4 severity → medium.** The "follow-up tool call breaks"
  symptom was likely a mix of Finding 4 *and* Finding 5. The
  underlying Qwen-XML-tool-call shape is real (probe 3's
  non-streamed replay returned it), but the empty-turn surface area
  was probably mostly Finding 5. Re-test once Finding 5 is fixed
  to see how much of Finding 4 remains.

**Second reproduction (different provider, different trigger, same symptom).**
Later in the same probing session the user tried selecting an OpenAI
GPT-5 model via the TUI and saw "no response." The captured
`RunEnd` in the debug log shows exactly the same swallow pattern:

```
stopped_for: ProviderError("invalid request: { ... \"message\":
  \"Unsupported parameter: 'max_tokens' is not supported with this
  model. Use 'max_completion_tokens' instead.\", ... }")
```

The agent loop *captured* the OpenAI 400 response verbatim, populated
`stopped_for: ProviderError(...)`, and yielded RunEnd. The TUI showed
nothing. The user retried their prompt three times in a row before
realizing something was wrong — see the session messages in the
`RunEnd` payload: three back-to-back identical user messages with no
intervening assistant turns. That's the exact "model treats every
message as the first" symptom we hypothesized at the start of the
probing session — but the mechanism is "harness eats the error and
keeps the prompt buffer open," not "history is being lost." Fixing
Finding 5 makes this user-discoverable; the underlying parameter bug
is captured separately as Finding 6 below.

---

## Finding 6 — GPT-5 / o-series reject `max_tokens`; caliban never sends `max_completion_tokens`

**Severity:** high for anyone selecting `gpt-5*`, `o1*`, `o3*`, or
`o4*` via the OpenAI provider. Zero impact for other model families.

**Where:**

- `crates/caliban-provider-openai/src/ir_convert.rs` builds the
  `NativeRequest` with `max_tokens: Some(req.max_tokens),
  max_completion_tokens: None`. The schema carries both fields, but
  only `max_tokens` is ever populated.
- OpenAI's GPT-5 family (and the o-series of reasoning models that
  preceded it) reject `max_tokens` with HTTP 400 and the explicit
  error `"Unsupported parameter: 'max_tokens' is not supported with
  this model. Use 'max_completion_tokens' instead."`

**Symptom (as a user).** Selecting `gpt-5` in the TUI and sending a
prompt produces *no visible response* — because Finding 5 swallows
the 400 silently. The user retries, gets nothing again, repeats. The
debug log carries the underlying OpenAI error verbatim inside
`RunEnd.stopped_for`, but the TUI never surfaces it.

**Evidence.** Reproduced from the user's own debug log at
`~/Library/Caches/caliban/debug.log` line 73 onward (run started
2026-05-25T23:42:52Z). Full error string above. Session state at
the time showed three consecutive user turns with no assistant turn
between them, confirming the user kept retrying because they had no
feedback.

**Suggested fix.** Two natural options:

1. **Model-family branching at request-build time.** The OpenAI
   provider already does this for `system_role` (`"system"` vs
   `"developer"` for o1-series — see the `system_role: &str`
   parameter in `ir_to_native_request`). Add a parallel
   `uses_completion_tokens(model: &str) -> bool` helper that
   matches `gpt-5*`, `o1*`, `o3*`, `o4*` and decides which field
   to populate. The `models.rs` table already has a `caps_o1`
   variant — natural place to hang a `uses_completion_tokens` bool
   on the `Capabilities` struct (or a parallel `ModelInfo` flag).
2. **Always populate both.** OpenAI's current behavior on
   conventional models is to accept `max_tokens` and ignore
   `max_completion_tokens` (or vice versa). Sending both may be
   tolerated everywhere — needs a quick test against gpt-4o /
   gpt-4.1 first to confirm no spurious rejection. Slightly less
   clean than option 1 but doesn't require a model-family registry.

Option 1 is the more durable answer because it integrates with the
existing per-family branching already in `ir_convert.rs`. Option 2
is two lines and ships today if a more careful fix is too costly.

**Interaction with Finding 5.** Fixing Finding 5 (surface stop
conditions) would have made this discoverable on first try — the
user would have seen `[caliban: provider error: unsupported
parameter 'max_tokens', use 'max_completion_tokens']` and either
filed a bug or tried gpt-4o. Fixing only Finding 6 without Finding 5
leaves the *next* swallowed error class to ambush a different user.
Both should ship together.

**Affected models (non-exhaustive):**

- `gpt-5`, `gpt-5-mini`, `gpt-5-nano` (current OpenAI flagship family)
- `o1`, `o1-mini`, `o1-preview` (reasoning models pre-GPT-5)
- `o3`, `o3-mini`, `o4`, `o4-mini`

**Not affected:** `gpt-4o`, `gpt-4o-mini`, `gpt-4.1*`, `gpt-3.5-turbo`,
and anything via Anthropic / Gemini / Ollama (those providers use
their own native fields).

---

## Probe cluster: Permissions (ALL PASS)

Six discrete probes against the permission-rules hook. All passed.

| Probe | Setup                          | Expected                                 | Result |
|-------|--------------------------------|------------------------------------------|--------|
| P-1   | `--deny Bash`                  | Bash call returns "permission denied"    | ✅ pass |
| P-2   | `--deny Read` (overrides default) | Read denied; default Allow lost          | ✅ pass |
| P-3   | `--deny "Read:/etc/*"`         | Read /etc/hostname denied, Read /tmp/ ok | ✅ pass |
| P-4   | `<ws>/.caliban/permissions.toml` w/ `Read=deny` | Read denied in that workspace | ✅ pass |
| P-5   | `--no-permissions`             | Bash (default Ask) executes              | ✅ pass |
| P-6   | `--auto-allow`                 | Bash Ask→Allow non-interactively         | ✅ pass |

Side-evidence gathered from this cluster:

- **Non-interactive Ask handler.** Without `--auto-allow`, an Ask-tier
  rule converts to a denial with the explicit message
  `"permission denied: 'Bash' requires interactive approval (no TTY)"`.
  Useful: the user sees *why* it was blocked, not just that it was.
- **Parallel tool dispatch** works in P-3: the model emitted two
  `Read` tool_use blocks in a single assistant message and caliban
  dispatched them concurrently. Tool-use block #1 (denied) and #2
  (allowed) returned independent results.
- **Finding 4 refinement (important).** Several P-1/P-3/P-5/P-6
  probes featured a *second* tool call (or a follow-up text response)
  *after* a tool result — and worked cleanly, with proper
  `delta.content`. So Finding 4 is **not** "every follow-up turn
  breaks." It seems specific to a narrower combination of
  (large/formatted tool result + the model's decision to chain to
  another tool call), and possibly to the heaviness of reasoning the
  model performs before the chained call. Worth a separate
  reproduction during the fix pass with both small/short tool results
  and large/long ones to bracket the trigger more precisely.

---

## Probe cluster: Headless mode (`-p` / ADR 0025) — MIXED

Seven probes against the headless driver. Two pass cleanly; the rest
surfaced four new findings (7-10 above).

| Probe | Setup                                            | Expected                                  | Result |
|-------|--------------------------------------------------|-------------------------------------------|--------|
| H-1   | `-p` text output (default)                       | Plain text on stdout, exit 0              | ✅ pass |
| H-2   | `-p --output-format json`                        | Single JSON `type:result` object          | ✅ pass (Finding 7: tokens are 0) |
| H-3   | `-p --output-format stream-json`                 | NDJSON: init → message → result           | ⚠️ Finding 8 (duplicate init) + Finding 7 (zero tokens) |
| H-4   | `-p --output-format stream-json` + provider 400  | Error frame + non-zero exit               | ⚠️ Finding 9 (silently succeeds, exit 0, subtype "success") |
| H-5   | `--max-turns 1` with tool-heavy prompt           | Exit 130, `subtype:"max_turns"`           | ✅ pass — proves error-surface plumbing exists |
| H-6   | Auto-headless via piped stdin                    | Treats stdin as prompt, headless on       | ✅ pass |
| H-7   | `--input-format stream-json` w/ 2 user frames    | 2 agent turns, EOF drains                 | ⚠️ Finding 10 (only first frame consumed) |
| H-8   | `--bare` disables auto-discovery                 | `settingSources: ["builtin"]`, fewer tools | ✅ pass (Finding 8 still applies — duplicate init) |
| H-9   | `--json-schema` success + failure paths          | Success → `structured_output`; failure → `subtype:"error"`, exit 2 | ✅ pass — proves error-subtype plumbing exists end-to-end |
| H-10  | Auto-headless via piped stdout (no `-p`)         | JSON result frame on stdout               | ✅ pass |

### Finding 7 — Streaming usage chunk is dropped

**Severity:** medium. Token counts in `result` frames, in TUI usage
summaries, and in any compactor that consumes `Usage` from the
provider are silently zero on every streaming OpenAI-compatible run.

**Where:** `crates/caliban-provider-openai/src/stream_parse.rs:98-205`.
The code only reads `chunk.usage` inside
`if let Some(choice) = chunk.choices.first() { ... if let Some(reason) = choice.finish_reason { let usage_delta = chunk.usage.map(|u| Usage { .. }); .. } }`.
But the OpenAI streaming protocol emits the usage frame as a
**separate, terminal chunk with empty `choices: []` and `usage`
populated** (this is the documented behavior when
`stream_options.include_usage: true` is set, which caliban now does
set — see the corrected Finding 1 above).

Because the no-choices chunk falls out of the `if let Some(choice)`
guard, `chunk.usage` is never extracted. The parser yields no
`MessageDelta { usage_delta: Some(_) }` for the terminal usage frame.
Total usage stays at `Default::default()`.

**Evidence.** Captured SSE during H-3 / H-4:

```
data: {"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}
data: {"choices":[],"usage":{"prompt_tokens":362,"completion_tokens":57,...}}
data: [DONE]
```

Two chunks, two different shapes. The finish-reason chunk has no
usage; the usage chunk has no choice. caliban's parser handles the
first and ignores the second.

**Suggested fix.** Pull the `chunk.usage` extraction OUT of the
nested `if`s and emit a standalone `MessageDelta` (with no
`stop_reason` and `usage_delta: Some(_)`) whenever `chunk.usage` is
present, regardless of whether choices are non-empty. The agent
runloop already accumulates `usage_delta` via `acc.usage.merge(u)`
(`stream.rs:766`).

### Finding 8 — Duplicate `system/init` frame in stream-json output

**Severity:** low-medium. Doesn't break parsers that are
permissive, but any typed consumer expecting "exactly one init
frame" will trip.

**Where:** Double emission:

- `caliban/src/main.rs:910` calls `driver.emit_init()` explicitly
  before invoking `driver.run(...)`.
- `caliban/src/headless/mod.rs:301` (inside `HeadlessDriver::run`)
  unconditionally calls `self.emit_init()` again as the first
  action of the run.

**Evidence.** Bytes-identical first and second NDJSON lines in
every stream-json run (H-3, H-4, H-5, H-7).

**Suggested fix.** Drop one of the two emit sites. The
`driver.run` site is the more obvious owner; remove the
explicit call at `main.rs:910`. Verify there isn't a
`structured_output`-only path that depends on the external
emit (which the comment near main.rs:910 hints at).

### Finding 9 — Headless swallows provider errors (Finding 5 in headless dress)

**Severity:** high (= Finding 5).

**Symptom.** A provider 400 (e.g. LM Studio context overflow, GPT-5
`max_tokens` rejection) produces:

- `subtype: "success"` in the `result` frame.
- `result: ""` (empty).
- Exit code 0.
- No `error` frame, no stderr output.

This violates ADR 0025 which specifies exit code 1 for "generic
runtime error" and 2 for "tool/assistant error", and which mentions
an error frame in the stream-json shape.

**Evidence.** H-4 against qwen2.5-coder with 17 tool defs (LM Studio
returned HTTP 400 "context length exceeded"). caliban output:

```
{"type":"system","subtype":"init", ...}
{"type":"system","subtype":"init", ...}
{"type":"result","subtype":"success","result":"","turns":0, ...}
```

`turns: 0` was the only signal that anything was wrong.

**Suggested fix.** The runloop's `RunEnd.stopped_for` field is the
same one Finding 5 calls out. Per H-5, the headless driver
already handles `MaxTurnsReached` → exit 130, `subtype:"max_turns"`.
Add the parallel mapping:

- `StopCondition::ProviderError(msg)` → emit `error` frame with
  `subtype: "error_during_execution"` (matching the documented
  vocabulary) + exit code 1, and include `msg` in the frame.
- `StopCondition::HookDenied(msg)` → exit 1 + error frame.
- `StopCondition::CompactionFailed(msg)` → exit 1 + error frame.
- `StopCondition::Cancelled` → exit 124 (already specified).

Fix this in the same pass as Finding 5; they're the same plumbing
on different drivers.

### Finding 10 — `--input-format stream-json` consumes only the first user frame

**Severity:** medium. Documented multi-turn behavior is not
implemented; any consumer feeding a transcript NDJSON to caliban will
silently lose all but the first user message.

**Where:** `caliban/src/main.rs:759-781`. The implementation
explicitly `break`s out of the frame iteration after finding the
first `InputFrame::User`. Control/interrupt frames are also
silently dropped (the code comment promises a "best-effort warning"
to stderr; no such warning is emitted).

**Evidence.** H-7 with this NDJSON stdin:

```
{"type":"user","content":"My favorite letter is Q. Remember this."}
{"type":"user","content":"What is my favorite letter? One word answer."}
```

caliban output (with `max_tokens=1024` to rule out Finding 2):

```
{"type":"message","role":"assistant","content":[{"text":"\n\nI'll add that to your memory!\n\n```markdown\n# Memory index\n\n- [favorite-letter](favorite-letter.md) — Q is the user's favorite letter\n```","type":"text"}]}
{"type":"result","subtype":"success","result":"...","turns":1, ...}
```

`turns: 1` — only the first user line was processed. The second
("What is my favorite letter?") was discarded.

**Suggested fix.** Replace the single-shot `for ... break;` with a
real driver loop:

1. Push each `User` frame onto the conversation as a user turn.
2. Run the agent until `RunEnd`.
3. Continue to the next input frame (or `control/interrupt`).
4. EOF gracefully drains and emits the final `result`.

This is a bigger change than the other suggested fixes — it
requires reshaping `HeadlessDriver::run` to consume a stream of
inputs rather than a single `Vec<Message>`. Worth considering
alongside ADR 0025's `--include-partial-messages` and
`--replay-user-messages` flags, which both presume multi-turn
flow exists.

### H-8 / H-9 / H-10 additional evidence (no new findings)

- **H-8** confirmed `--bare` correctly drops setting sources
  `hooks.toml`, `skills`, `mcp.toml`, `memory` (init frame shows
  `settingSources: ["builtin"]`) and trims the tool count from 19
  to 16 — the three dropped tools are `Skill`, `ReadMemoryTopic`,
  `WriteMemoryTopic`. `bare_mode: true` is correctly surfaced.
  Finding 8's duplicate-init bug still applies under `--bare`.
- **H-9** is the most informative pass in this cluster. Both the
  success path (`subtype:"success"` + populated `structured_output`)
  and the failure path (`subtype:"error"`, exit 2, `error: "field
  'answer' expected type 'integer', got 'string'"`) work cleanly.
  This is the **clearest existence proof that the headless driver
  already has a working "non-success subtype + non-zero exit code"
  plumbing path** for the schema-validation case. Wiring
  Finding 9 (and Finding 5 in the runloop) into the same shape
  should be straightforward: add the missing
  `StopCondition::ProviderError` → `ResultSubtype::Error` + exit 1
  mapping, mirroring `SchemaValidation` → `Error` + exit 2.
- **H-10** confirmed auto-headless triggers correctly when stdout
  is piped (and stdin is non-TTY). Output is well-formed JSON
  on stdout, no TUI fallback. Matches ADR 0025.

---

## Probe cluster: Sub-agent (AgentTool) — BLOCKED on Finding 5

Sub-agent probes (S-1 basic delegation, S-2 no recursion, S-3 per-input
model override, S-4 tool_allowlist) could not be executed end-to-end
against LM Studio in this session. Two attempts on different models
both produced the silent empty-turn symptom; proxy capture confirmed
both were Finding 5 (HTTP 400, context overflow) — not a sub-agent
bug per se.

Quantified blocker:

- `qwen2.5-coder-7b-instruct-mlx` (no `--no-sub-agent`): request body
  ≈ 13.3 KB / 17 tools — overflows the model's loaded ctx, HTTP 400.
- `qwen3.5-9b-mlx` (with AgentTool in palette): 15.3 KB / 18 tools —
  also overflows even this model's larger loaded ctx, HTTP 400.
- Removing AgentTool (`--no-sub-agent`) brings the request down to
  13.3 KB and qwen3.5 accepts it (HTTP 200) — but that defeats the
  purpose of the sub-agent probe.

**To unblock these probes, one of the following needs to happen:**

1. **Raise LM Studio's loaded context length** on the target model
   (per-model setting in LM Studio's UI). The model files for both
   qwen3.5 and qwen2.5-coder support more than the default loaded
   ctx. This is the most direct unblock and the right configuration
   for tool-heavy harnesses like caliban.
2. **Add a CLI flag to restrict the tool palette** at startup (e.g.,
   `--tools Read,Grep,AgentTool`). Today the only knobs are
   `--no-tools` (all off), `--no-skills`, `--no-mcp`, `--no-sub-agent`
   — there's no positive allowlist. Would also help bake smaller
   request envelopes for resource-constrained local models.
3. **Run sub-agent probes against a hosted provider** (Anthropic /
   Gemini AI Studio / OpenAI) where context isn't a constraint.

**Implicit observation worth surfacing.** Caliban's default tool
palette has grown to **18 tools** (Read, Write, Edit, MultiEdit,
NotebookEdit, Bash, Glob, Grep, WebFetch, WebSearch, BashOutput,
KillShell, TodoWrite, EnterPlanMode, ExitPlanMode, ReadMemoryTopic,
WriteMemoryTopic, Skill, AgentTool). The combined JSON-schema for
those tools is around 8-10 KB inside the request. For LM Studio
operators running default-loaded local models (typically 4-8k ctx),
this is enough to push every request past the context limit *before
the user has typed anything*. Combined with Finding 5 (silent
swallow), the user-visible experience is "I ran caliban and it does
nothing." Worth at least a startup warning when tool-defs token
estimate exceeds, say, 50% of a small-ctx capability hint.

---

## Probe cluster: MCP (ALL PASS)

**Up-front correction.** Earlier sections of this doc were written
based on a stale read of `crates/caliban-mcp-client/src/manager.rs`
(commit `aaeddf8`, the v1 config-only stub that registered no tools
and didn't spawn). Two later commits shipped real spawn behavior:

- `c85da01 feat(mcp): Phase A — rmcp stdio wiring closes ADR 0017 deferral`
- `6e9c37b feat(mcp): Phase B — HTTP + SSE transports (ADR 0023)`

So caliban today *does* spawn MCP servers, perform the rmcp
handshake, and register the discovered tools into the parent
registry. The doc's "Status" table from earlier should be read with
this in mind; nothing in Findings 1-4 is changed by this correction,
but anywhere that said "MCP is config-only / spawn deferred" is now
outdated.

Five explicit probes against the current MCP plumbing. All passed.

| Probe | Setup                                       | Expected                                          | Result |
|-------|---------------------------------------------|---------------------------------------------------|--------|
| M-1   | No `mcp.toml` anywhere                      | Silent no-op (log gated on counts > 0)            | ✅ pass |
| M-2   | Enabled server (`command="echo"`)           | Spawn attempt; handshake fails; `failed=1`        | ✅ pass |
| M-3   | `disabled = true` server                    | No spawn attempt; counted as `disabled=N`         | ✅ pass |
| M-4a  | `env = { K = "${VAR}" }` w/ var set         | Spawn proceeds (failure is downstream, on handshake) | ✅ pass |
| M-4b  | Same config w/o var set                     | Clean config-time error: `references unset env var` | ✅ pass |
| M-5   | Server key `UPPERCASE_BAD`                  | Specific, regex-citing error                       | ✅ pass |

Observations worth carrying forward:

- **MCP startup is non-fatal.** Spawn errors, handshake failures, and
  config-load failures all log a `WARN` and let caliban continue
  without that server's tools. Good resilience for the local-first
  use case where servers may not be available every run.
- **Error messages are operator-grade.** The "invalid server name"
  error names the offender and cites the regex. The "missing env
  var" error names both the server and the offending env key. No
  guessing required.
- **`mcp manager started connected=N failed=M disabled=P`** is
  the one-line status; useful for grepping in long debug logs. The
  WARN line for a failed server includes the rmcp transport error
  verbatim, which was enough to diagnose every probe failure above
  from logs alone.
- **One latent quirk worth noting (not a finding).** When a server
  fails to start, the message is `mcp server failed to start;
  skipping server=...`. The phrase "skipping" reads as if the run
  *skipped* the server — but `connected_count` still ticks at zero
  for it. Slightly clearer might be "removed from registry" or
  "tools not registered." Pure prose nit, not worth a fix on its
  own.
