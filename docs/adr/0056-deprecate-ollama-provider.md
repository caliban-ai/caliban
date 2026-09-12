# ADR 0056 · Deprecate the bespoke Ollama provider — reach local models through the OpenAI-compatible surface

- **Status:** accepted
- **Date:** 2026-09-12
- **Source:** local-inference landscape review (this session). Community rationale drawn from [*Stop Using Ollama*](https://sleepingrobots.com/dreams/stop-using-ollama/) (sleepingrobots.com), folded into Context below as dependency-free historical reference. Engine choice is grounded in a head-to-head benchmark on Apple M5 Pro via `scripts/bench-local-inference.sh`, and validated for correctness via `scripts/conformance-local-inference.sh` (summarized under *Benchmark evidence* and *Conformance evidence*). Relates to [0007](0007-transport-trait-pattern.md) (schema-family provider crates — this drops one), [0022](0022-model-routing-architecture.md) / [0038](0038-model-router-v2.md) (routing + per-route `base_url`), [0033](0033-opentelemetry-and-cost.md) (provider list), and the operator runbook for standing up the replacement stack.

## Context

caliban treats **Ollama as a first-class, hard-wired provider**, not as one instance of a
pluggable local backend. Concretely (from a full-workspace survey):

- `caliban-provider-ollama` is an unconditional workspace member and dependency of the
  `caliban` binary — no feature gate.
- There is a `ProviderKind::Ollama` enum variant with four dispatch `match` arms
  (`args.rs`, `router.rs`, `startup/compose.rs`, `effective_model.rs`), plus trailing
  derivations: a default model, TUI status labels, a doctor probe, `OLLAMA_*` env
  plumbing (including the slow-local stream-watchdog override), a zero-cost `rates.yaml`
  card, and `scripts/ollama-probe.sh`.
- The crate itself speaks Ollama's **native `/api/chat`** wire format — NDJSON streaming,
  object-form tool arguments, bare-base64 images, a `thinking` message field, an
  `options` block — and carries a bespoke **discovery subsystem** over `/api/tags` +
  `/api/show` + `/api/ps` that enumerates pulled models and detects the live
  context window.

Structurally, ~two-thirds of the crate is a near-1:1 mirror of `caliban-provider-openai`
(config / error / transport / IR-conversion / `Provider` impl); the OpenAI crate's own
comments even cite the Ollama crate as their mirror source. The genuinely
Ollama-specific parts are the native wire format and the discovery subsystem.

Two independent forces now make this bespoke provider poor value to maintain.

**1 — Governance / community concerns with Ollama.** The reasons documented in
*Stop Using Ollama* apply directly to why caliban should not privilege it as a built-in.
The major ones, folded in so this ADR stands alone:

- **Thin attribution to llama.cpp.** Ollama's inference engine derives from llama.cpp,
  yet attribution was absent for a long period despite the MIT notice requirement — a
  compliance issue that went unanswered for 400+ days.
- **A forked, regressed backend.** Ollama moved off llama.cpp to a custom GGML build
  that reintroduced already-solved bugs (broken structured output, vision-model
  failures, newer-model incompatibilities); llama.cpp benchmarks ~1.8× faster on
  identical hardware.
- **Misleading model naming.** Stripping the "Distill" prefix from DeepSeek-R1 variants
  led users to believe they ran the full model, damaging its reputation.
- **Closed-source / unlicensed components.** The desktop GUI shipped without a license
  in a private repo with likely AGPL dependencies — at odds with an open-source posture.
- **Proprietary Modelfile + hashed storage lock-in.** Config layered on top of GGUF, and
  a proprietary hashed model store, make migration to llama.cpp / LM Studio deliberately
  harder; changing one parameter can rewrite tens of GB.
- **Restrictive registry.** Only Q4/Q8 quants are packaged; Q5/Q6/IQ users are sent
  elsewhere, and new models wait on Ollama packaging that HF GGUFs never require.
- **Cloud pivot with unclear privacy + a slow-fixed credential CVE (CVE-2025-51471)**,
  and a VC-driven build-on-open-source → lock-in → monetize trajectory whose incentives
  diverge from the open-source community.

These are ecosystem/governance arguments, not a runtime defect in caliban — but they are
exactly the reasons not to give one such vendor a privileged, hard-coded seat in the
binary.

**2 — The local-engine landscape shifted, and it is all OpenAI-compatible now.** As of
2026 the practical local engines caliban would target expose OpenAI-compatible
`/v1/chat/completions`:

- **`llama-server`** (llama.cpp) — Metal on Apple Silicon, single dependency-free binary,
  widest model coverage, mature GBNF/JSON-schema constrained decoding, prompt-cache slots.
- **MLX-native servers** — `mlx_lm.server`, `mlx-openai-server`, LM Studio's MLX runtime —
  the only path to Apple's MLX and (on M5) the GPU Neural Accelerators. (Ollama itself
  switched its Apple-Silicon engine to MLX in 0.19; llama.cpp has **no** MLX backend and,
  per its maintainers, won't — MLX is too high-level to be a ggml backend. "MLX" and
  "llama.cpp/Metal" are mutually exclusive engines, not a combined thing.)

The decisive architectural fact: **caliban has no "local model" abstraction to preserve.**
The seam is the string provider id plus the `Provider` trait, and `caliban-provider-openai`
already reaches any OpenAI-compatible server via `OPENAI_BASE_URL` (single-provider) or the
router-v2 **per-route `base_url`** (0038). So local inference already works *without* the
Ollama crate — a fact the code comments and doctor already acknowledge (the OpenAI adapter
is the documented path to LM Studio / vLLM / `llama.cpp-server`).

**What is genuinely lost by removal.** Only the discovery subsystem
(`/api/tags` + `/api/show` + `/api/ps`): dynamic model enumeration, live per-model
capability flags, and **loaded-context-window detection**. An OpenAI-compatible surface
offers `/v1/models` (and llama.cpp a `/props` endpoint), but the current OpenAI adapter
consumes neither — its model list is a static table. This is the one capability with no
drop-in replacement, and the only real cost of the decision.

Alternatives weighed:

- **Keep Ollama as-is.** Rejected: privileges a vendor whose posture we specifically want
  not to endorse, and carries a bespoke native-`/api/chat` adapter that duplicates the
  OpenAI one for shrinking benefit.
- **Replace Ollama with a bespoke `llama-server` provider.** Rejected as unnecessary:
  llama-server is OpenAI-compatible, so a new native adapter would re-duplicate the OpenAI
  crate again — the exact mistake we're undoing.
- **Deprecate Ollama; reach every local engine through the OpenAI adapter + `base_url`.**
  Chosen (below).

## Benchmark evidence (Apple M5 Pro, 2026-09-12)

To ground the engine choice rather than argue it from marketing, we benchmarked the three
engines head-to-head on the target hardware (Apple M5 Pro) with a **caliban-shaped prompt**
— a ~2.7k-token system+tools prefix and a short completion, the shape that dominates agent
latency — using the same Qwen3.6-27B weights at 4-bit in each engine's native format (GGUF
`Q4_K_M` for llama.cpp, `mlx-community/…-4bit` for the MLX engines). Harness:
`scripts/bench-local-inference.sh` (curl + jaq, 7 runs, medians; TTFT measured at the first
streamed token, not curl's proxy-faked TTFB). Both cache regimes were measured: **cold**
(unique prompt per run → prefix-cache miss) and **warm** (fixed prompt → cache hit).

| Backend            | cold TTFT | cold prefill | warm TTFT | warm prefill | decode    |
|--------------------|-----------|--------------|-----------|--------------|-----------|
| Metal (llama.cpp)  | 8.11 s    | 338 tok/s    | 0.19 s    | 14,620 tok/s | 15.5 tok/s|
| MLX (mlx-lm)       | 6.16 s    | 445 tok/s    | 0.30 s    | 9,102 tok/s  | 17.2 tok/s|
| Ollama (MLX)       | 6.47 s    | 423 tok/s    | 0.22 s    | 12,250 tok/s | 15.5 tok/s|

Findings:

- **The prefill winner flips with cache state.** Cold: mlx-lm wins (~1.3× Metal). Warm:
  Metal wins (~1.6× mlx-lm). There is no single prefill winner — it depends on cache-hit rate.
- **All three cache effectively**, collapsing 20–40× from cold to warm TTFT; mlx-lm caches
  too (disproving a concern that it might not), with the slowest warm lookup of the three.
- **Decode is engine-flat and cache-independent** (~15.5 tok/s; mlx-lm ~+11% at 17.2) and a
  small factor for short agent completions.
- **Ollama is dominated in every regime** — it never wins cold, warm, or decode. Dropping it
  costs nothing on performance.
- **Real agent turns lean warm**: a large stable system+tools prefix (cached after turn 1)
  plus a short growing tail, so steady-state multi-turn favors the warm path — where
  **llama.cpp/Metal is fastest** on both TTFT and prefill with equal decode.
- The M5 Neural-Accelerator advantage is real but ~1.3× cold prefill, **not** the "4×"
  marketing figure (which was M5-vs-M4 TTFT, not MLX-vs-Metal).

The MLX gains (cold prefill −25% vs Metal, decode +11%) are real but modest and situational;
llama.cpp/Metal wins the cache-friendly multi-turn path that dominates coding sessions. This
supports **llama.cpp as a single default engine even on Apple Silicon**, with MLX as an
*optional* per-host tune for cold- or decode-heavy workloads.

## Conformance evidence (same three engines, 2026-09-12)

Speed is moot if a backend can't drive the agent loop, so a companion harness
(`scripts/conformance-local-inference.sh`) asserts the capability surface caliban actually
parses — grounded in the prior Ollama/LM Studio probes (`docs/evaluation/probes/`). Results
(Qwen3.6-27B on each engine):

| Test (P0 unless noted)         | Metal | mlx-lm | Ollama |
|--------------------------------|-------|--------|--------|
| tool call (non-stream)         | pass  | pass   | pass   |
| arguments valid JSON           | pass  | pass   | pass   |
| tool call (streaming)          | pass  | pass   | pass   |
| no tool-XML leak               | pass  | pass   | pass   |
| finish_reason stop / length    | pass  | pass   | pass   |
| tool-result round-trip         | pass  | pass   | pass   |
| usage accounting (P1)          | pass  | pass   | pass   |
| reasoning field (P1)           | pass  | **FAIL** | **FAIL** |

Findings:

- **All three are P0-conformant.** Tool calling, valid-JSON arguments, streaming tool calls,
  correct stop reasons, cap adherence, and the tool round-trip all pass on every engine — so
  the agent loop works on all three. mlx-lm being viable is what the "optional per-host tune"
  recommendation rests on, and this confirms it.
- **The feared MLX tool-XML leak does not occur.** The LM Studio failure mode —
  `<tool_call>` markup leaking into the reasoning channel instead of parsing into
  `tool_calls` — is **absent** on both mlx-lm and Ollama-MLX with Qwen3.6 (NOLEAK passes). No
  `--tool-call-parser` workaround was needed on this stack.
- **The one real gap: reasoning field name.** Both MLX engines stream thinking as
  `reasoning`; caliban's OpenAI schema reads only `reasoning_content`
  (`crates/caliban-provider-openai/src/schema/events.rs:47`, `.../response.rs:45`), so their
  thinking is silently dropped. This is **not** mlx-lm-specific — Ollama-MLX has it too, so
  it is no reason to keep Ollama — and it is a one-line fix: add
  `#[serde(alias = "reasoning")]` to those two fields. Until then it is a P1 display gap, not
  a loop-breaker (tool calls and content are unaffected).

Net: correctness validation clears the migration — every engine drives the agent loop, and
the sole divergence is a trivial, uniform adapter fix.

## Engine-behavior findings the OpenAI adapter/router must handle

Standing up the three engines surfaced concrete portability gaps the OpenAI-compatible path
must absorb — each observed on the M5 Pro box, not hypothetical:

- **`model`-field semantics differ.** llama.cpp ignores the request's `model` field and
  serves whatever is loaded; **mlx-lm treats it as a model to load** (an unknown name is
  fetched from Hugging Face and 404s). So the router cannot assume a friendly alias works
  across backends — the name it sends must be one the target engine accepts (for mlx-lm, the
  exact repo id).
- **Reasoning output lands in different fields (confirmed by the conformance run).** Both MLX
  engines — mlx-lm *and* Ollama-MLX — stream thinking as `reasoning`; llama.cpp uses
  `reasoning_content`, the only field caliban's OpenAI schema reads, so caliban drops MLX
  thinking today. Fix: `#[serde(alias = "reasoning")]` on the two `reasoning_content` fields
  (`schema/events.rs:47`, `schema/response.rs:45`). A P1 display gap, not a loop-breaker —
  `content` and `tool_calls` are unaffected.
- **`/v1/models` identity is not the repo id** — mlx-lm reports `default_model`. The
  discovery shim (below) must tolerate a synthetic model id.
- **Prompt-cache maturity varies** (see benchmark): all three cache, but warm-path latency
  differs — not a correctness issue, but it shapes which engine is fastest per workload.

## Decision

We will **deprecate and remove the bespoke `caliban-provider-ollama` crate and the
first-class `ProviderKind::Ollama`, and reach local models exclusively through the
OpenAI-compatible provider** (`caliban-provider-openai`) pointed at a local engine's
`/v1` endpoint via `base_url` — `OPENAI_BASE_URL` for the single-provider path, or a
router-v2 per-route `[provider.openai].base_url` (0038) for per-model/per-host selection.

**Engine posture (grounded in the benchmark above).** The project default is
**llama.cpp / `llama serve`** everywhere, **including Apple Silicon** — portable, a single
dependency-free binary, widest model coverage, mature constrained decoding, and it wins the
cache-friendly warm path that dominates multi-turn agent sessions. It is also the honest
"not-Ollama," since Ollama is a wrapper over it. **MLX** (via `mlx_lm.server` / LM Studio) is
an **optional per-host tune**, not an automatic upgrade — worth selecting only for cold- or
decode-heavy workloads, and chosen purely by aiming `base_url` at that host (a config line,
not a code branch). `llama-swap` may front multiple engines behind one `/v1` endpoint to
recover Ollama's "just ask for a model" convenience. Standing up this stack is an operator
task, documented in the companion runbook, and is a **precondition** for removal on any box
that relied on Ollama.

**Phasing (soft-deprecate, then remove)** so no running setup breaks silently:

1. **Deprecate (this release).** Mark `caliban-provider-ollama` and `ProviderKind::Ollama`
   deprecated. On selecting the `ollama` provider (or an `ollama/<model>` selector), emit a
   one-time notice pointing at the OpenAI-`base_url` path and the runbook. Add a doctor
   hint. Update docs (guide providers pages, parity matrix, env-vars) to present the OpenAI
   adapter as the local-model path. No behavior change yet.
2. **Remove (a following release).** Delete the crate + workspace member, the four dispatch
   arms, the `ProviderKind::Ollama` variant and its derivations (default model, TUI labels,
   `OLLAMA_*` env plumbing, `rates.yaml` card, `scripts/ollama-probe.sh`), and the doctor
   Ollama probe. Amend the enumerations in [0007](0007-transport-trait-pattern.md) (drops a
   schema-family crate), [0033](0033-opentelemetry-and-cost.md) (provider list), and
   [0038](0038-model-router-v2.md) (the Ollama no-op mapping). When this removal lands,
   annotate those ADRs' index rows accordingly (tracked as Phase 2, ticket #635).

**Discovery replacement (follow-on, optional but recommended).** To recover the one lost
capability, add a small **`/v1/models` + `/props` discovery shim** to the OpenAI adapter so
a local OpenAI-compatible server can report its model list and loaded context window. If
this shim does not land, local context windows fall back to a configured value
(`num_ctx` / a settings field) — an acceptable degradation, not a blocker for removal.

**Migration for operators.** Point the OpenAI provider at the local engine:
`OPENAI_BASE_URL=http://<host>:<port>/v1` (any non-empty `OPENAI_API_KEY`), or a router
route with `[provider.openai].base_url` and `model` set to the engine's model name. The
runbook covers `llama-server`, `mlx_lm.server`, and `llama-swap` on Apple Silicon.

**Sub-tickets to spawn (epic):**

1. Deprecation notices + doctor hint on the `ollama` provider / `ollama/*` selector.
2. Docs: reframe the local-model path around the OpenAI adapter + `base_url`; add the
   engine-setup runbook; update the parity matrix.
3. `/v1/models` + `/props` discovery shim in `caliban-provider-openai` (recovers
   context-window detection; must tolerate a synthetic `model` id such as mlx-lm's
   `default_model`).
4. OpenAI-adapter robustness for local reasoning engines: add `#[serde(alias = "reasoning")]`
   to `reasoning_content` (`caliban-provider-openai` `schema/events.rs:47`,
   `schema/response.rs:45`) so MLX engines' thinking isn't dropped (confirmed against mlx-lm
   and Ollama-MLX); and ensure the router sends a model name the target backend accepts
   (mlx-lm requires the exact repo id; llama.cpp ignores the field). See *Conformance
   evidence* and *Engine-behavior findings*.
5. Removal: delete crate + member + four dispatch arms + `ProviderKind::Ollama` +
   derivations + probe script + `rates.yaml` card + doctor path.
6. Amend ADRs 0007 / 0033 / 0038 enumerations and annotate index rows.

## Consequences

- **Positive:** removes an entire bespoke native-`/api/chat` wire adapter (~a crate's
  worth of duplicated plumbing) in favor of the OpenAI adapter caliban already maintains;
  stops privileging a vendor whose governance posture we specifically decline to endorse;
  and turns engine choice into a per-host `base_url` decision, so the project baseline can
  be llama.cpp while any Apple-Silicon box runs MLX with no code fork. Local inference gains
  the full engine field (llama.cpp, MLX servers, vLLM, LM Studio, llama-swap) through one
  seam instead of one blessed vendor. The engine posture is empirically grounded (benchmark
  above): llama.cpp is competitive-to-better on the warm path that dominates multi-turn
  sessions, so a single default engine is viable and MLX is optional rather than required —
  and Ollama, dominated in every regime, is dropped with no performance regret. Correctness
  is validated too (conformance run above): all three engines are P0-conformant, so the
  agent loop works on each, and the only divergence — MLX's `reasoning` field name — is a
  one-line adapter alias.
- **Negative:** loses dynamic model discovery + live context-window detection unless the
  `/v1/models`+`/props` shim (sub-ticket 3) lands; without it, local context windows must be
  configured. Operators on `provider = "ollama"` / `ollama/*` must migrate (mitigated by the
  soft-deprecation window, notices, and the runbook). Standing up the replacement stack is
  more operator setup than `ollama run` — the convenience cost of dropping the wrapper,
  partly recovered by `llama-swap`.
- **Revisit if:** the `/v1/models`+`/props` discovery seam proves insufficient and
  loaded-context detection turns out load-bearing enough to justify a dedicated local-engine
  provider after all; or a single local engine gains a genuinely non-OpenAI capability we
  want first-class (at which point a *feature-gated* adapter — not a hard-wired provider — is
  the pattern to reach for); or Ollama's governance concerns are resolved and demand for it
  as an *optional, feature-gated* provider re-emerges; or the workload mix shifts markedly
  cold- or decode-heavy, where the benchmark shows MLX's advantage would justify making the
  per-host MLX tune the default on Apple-Silicon boxes rather than an option.
