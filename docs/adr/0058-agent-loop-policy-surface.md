# ADR 0058 · Agent-loop policy surface — model-adaptive budgets, containment, and verification

- **Status:** accepted
- **Date:** 2026-09-16
- **Source:** epic [#259](https://github.com/caliban-ai/caliban/issues/259); bug [#239](https://github.com/caliban-ai/caliban/issues/239). Builds on [0009](0009-agent-core-design.md) (agent-core design).

## Context

Caliban exposes **model-inference** knobs — `effort` and `thinking` control how hard
the model works inside a single call. It does **not** expose **agent-loop policy** —
the turn budget, any time or cost budget, whether the agent verifies its own work,
and how a run is contained when it stops making progress. Those are hard-coded or
under-exposed in `crates/caliban-agent-core`. Expert operators cannot tune the loop
to their situation, and there is no context-adaptive default: a cloud run should stay
cost-conservative (provider tokens are the scarce resource), while a local run should
prioritize result quality (generation is effectively free, so time and correctness are
the constraints, not cost).

**What already exists in the loop.** Several spiral-containment guards ship today:
`max_turns` (default 50, a CLI flag); a no-edit nudge (`no_edit_nudge_threshold`,
default 10 — this is the first half of #239); an empty-turn guard
(`empty_turn_nudge_max`, #249); a thinking-spiral guard (`max_turn_thinking_chars`,
#62); and stream watchdogs. **What is missing:** any wall-clock **time budget** (the
code explicitly notes "no total deadline"), any **cost budget**, a **wrong-path /
churn guard**, a **verification-policy** knob (verification behavior is entirely
emergent from the system prompt), and any **adaptive or profile-based default
selection**. The existing guards that do exist are mostly hard-coded rather than
configurable.

**The originating measurement (#239).** On build-heavy repositories (matplotlib,
sphinx, pylint), the agent locates and understands the fix, then spends its entire turn
budget trying to stand up a local build/test loop and never makes a single edit — an
empty patch (measured: one instance ran 40 turns = 34 Bash + 7 Read + 0 Edit). A
prompt patch that forbade building merely *moved* the failure: the agent then shipped
confident-wrong fixes. The mis-calibration runs in **both** directions.

**The decisive experiment (eval "sub-project A", complete).** Before exposing any knob,
we ran an eval experiment: let the model write and run its own reproduction to verify a
fix (a "verify when cheap" prompt, executed inside each SWE-bench instance's own
dependency-provisioned container), and measure the resolve rate against the
forbid-verification baseline, on the same 25-instance set, for two model arms. The
result reframes the whole design:

- A strong cloud model went **52% → 60% resolved (+8 points)**.
- A weaker local model went **72% → 56% resolved (−16 points)**.

The verification lever's **sign flips with model strength**. Crucially, **adoption was
100% for both models** — the weak model fully took the verification offer; adoption is
not the differentiator. What differs is what each does with a failing test: the strong
model *converges* toward the fix; the weak model *churns* — it edits, tests, and
re-edits in the wrong direction, grinding to the turn cap (mean turns tripled to 41;
eight of twenty-five hit the cap). Verification **cured** the original #239 no-edit
spiral but **replaced** it with wrong-path churn — a failure that is invisible to every
guard the loop has today, because churn is full of real edits and real tool calls. The
dollar cost cap, previously theoretical, became an active constraint on hard cloud
instances (~1.8× cost).

Three conclusions follow directly and force this ADR:

1. **A single global default is provably wrong for someone** — model-adaptive defaults
   are mandatory, not a nicety.
2. **The load-bearing new mechanism is a no-progress / wrong-path guard**, not a bigger
   turn budget. For a weak model the correct posture is "verify, *plus* a tight
   divergence guard"; the naive "quality-first = big turn budget + verify-on" actively
   backfires, because more turns just buy more churn.
3. **The budget model must be time- and divergence-aware**, not merely a larger turn
   count.

## Decision

We will introduce an **agent-loop policy surface**: a coherent configuration group
(working name `[agent_loop]`) that gathers the existing hard-coded guards and a set of
new knobs, exposed consistently through settings (and the most useful ones as CLI
flags), with **defaults that adapt to execution context**. Three knob families plus
adaptive defaults:

1. **Budgets (bounds).** The existing turn budget; a new wall-clock **time budget**
   (`StopCondition::TimeBudgetExceeded`); and an optional **cost budget** in dollars
   (`StopCondition::CostBudgetExceeded`). Time-boxing environment/build setup is the
   second half of #239.

2. **Containment (guards).** Surface the existing no-edit, empty-turn, and
   thinking-spiral guards as configuration, and add the **wrong-path / divergence
   guard** — the lever the experiment proved load-bearing. It watches the
   edit → test-result signal (an edit that fails verification followed by re-editing
   without net progress), not mere inactivity, because churn is invisible to the
   inactivity-based guards.

3. **Verification policy.** A guidance knob — `off` / `verify-when-cheap` / `full` —
   that shapes the verification guidance injected into the system prompt. This is
   precisely what the experiment's baseline and treatment prompts controlled. Caliban
   does not fabricate or run tests on the model's behalf; it shapes when the model is
   *encouraged* to verify.

4. **Adaptive defaults, keyed on cloud-vs-local execution context, with an explicit
   override.** Because the experiment's finding is about model *strength* — which
   caliban cannot reliably introspect — we use the **execution context (local provider
   vs. cloud provider) as the proxy** for strength, which is exactly the axis the two
   experiment arms measured: a local run defaults to quality-first *paired with a tight
   divergence guard*; a cloud run defaults to cost-conservative. This selection is
   always overridable by an explicit **named profile** (`cost-optimized`,
   `quality-first`, `local-guarded`), so the imperfect cases (a strong model behind a
   local gateway; a weak model in the cloud) are handled by the user, not mis-served by
   the heuristic. We deliberately reject (a) a `model_tier` map as the *default* axis —
   more honest to strength but requiring a maintained model→tier table and a fragile
   fallback for unknown models (it may return as an optional refinement), and (b)
   profiles-only with no auto-detection — simplest, but it abandons the "sane adaptive
   default" the epic exists to provide.

5. **The divergence guard is eval-gated.** We will not ship a guessed churn-detection
   threshold. A follow-up eval sub-project ("A2", in the local-only evals repository,
   mirroring how "A" de-risked verification) validates the detection signal and its
   thresholds before the guard is enabled by default. Until then it may land behind an
   off-by-default flag.

This ADR records the **surface and the default policy**; it does not implement them.
The work is decomposed into child tickets under epic #259 (B1: the `[agent_loop]`
config surface; B2: time budget; B3: cost budget; B4: the wrong-path/churn guard,
eval-gated; B5: the verification-guidance knob; B6: adaptive defaults + named profiles),
plus the A2 eval sub-project. The agent-loop policy surface sits *parallel* to the
existing model-inference knobs (`effort`, `thinking`) established under
[0009](0009-agent-core-design.md): the model knobs govern each call; these govern the
loop around the calls.

## Consequences

- **Positive.** Expert operators can tune loop shape and bounds to their situation.
  Defaults become context-appropriate instead of one-size-fits-all, directly correcting
  the both-directions mis-calibration in #239: the time budget and divergence guard stop
  the build-trap and the churn spiral, while the verification knob lets strong models
  self-correct (worth ~+8 points where it helps) without forcing it on models it hurts.
  The existing hard-coded guards become discoverable and adjustable. caliban-operator
  and other embedders get a real control surface instead of a hard-coded loop.

- **Negative.** The cloud-vs-local proxy for model strength is imperfect and leans on
  the override for the exceptions — an operator who runs a strong model locally must
  select a profile. The divergence guard is genuinely novel and carries the epic's main
  implementation and validation risk; it must not ship on a guessed threshold, which is
  why B4 is gated on the A2 eval. Verification, where enabled, costs roughly 1.8× in
  turns and dollars, so the cost budget must be real, not decorative. A larger
  configuration surface is more to document, test, and keep coherent across settings and
  CLI flags.

- **Revisit if.** The divergence guard cannot be made to catch wrong-path churn without
  also tripping on legitimate iterative work (A2 fails to find a robust signal) — then
  reconsider whether verification should default on for local models at all. Or if
  model-strength detection becomes reliable (a capability hint the provider surfaces, or
  a stable model→tier map), promote the `model_tier` axis from optional refinement to
  the primary default selector, since it is more honest to the finding than the
  cloud/local proxy.

_Historical note: the empirical basis is the 2026-06 SWE-bench eval study (harness at
`~/dev/caliban-ai/qa/evals`); the cross-model A/B is written up at length in the
project's decision log. This ADR states the finding directly and does not depend on
those documents._
