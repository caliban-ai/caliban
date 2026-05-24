# Token Accuracy Verification

**Date:** 2026-05-23
**Branch:** `jf/feat/perf/baseline`
**Scope:** Confirm that the four token counters (`input_tokens`, `output_tokens`, `cache_creation_input_tokens`, `cache_read_input_tokens`) flow correctly from provider response → IR → per-turn merge → per-run merge → `RunEnd` → `UsageSummary` transcript line → render.

## Pipeline trace

Hop-by-hop, for each counter:

### `input_tokens`

| Hop | Location | Notes |
|---|---|---|
| Wire-format → IR (Anthropic, non-stream) | `crates/caliban-provider-anthropic/src/ir_convert.rs:187` (response builder) | **Anthropic reports `input_tokens` as uncached only**, per API spec |
| Wire-format → IR (Anthropic, stream) | `crates/caliban-provider-anthropic/src/stream_parse.rs:99` | Same source field, same semantics |
| Wire-format → IR (OpenAI, non-stream) | `crates/caliban-provider-openai/src/ir_convert.rs:330` | `prompt_tokens` is **total including cached** (OpenAI semantics) |
| Wire-format → IR (OpenAI, stream) | `crates/caliban-provider-openai/src/stream_parse.rs:191` | Same source, same semantics |
| Per-event accumulation | `crates/caliban-agent-core/src/stream.rs:667` `acc.usage.merge(u)` | `Usage::merge` sums |
| Per-turn capture | `crates/caliban-agent-core/src/stream.rs:675` `turn_usage = acc.usage` | Copy |
| TurnEnd emission | `crates/caliban-agent-core/src/stream.rs:773` (normal) and `:753` (hook-denied) | Pass-through |
| Per-run aggregation | `crates/caliban-agent-core/src/stream.rs:799` `total_usage.merge(turn_usage)` | Sum |
| RunEnd emission | `crates/caliban-agent-core/src/stream.rs:818` | Pass-through |
| TUI `UsageSummary` build | `caliban/src/tui.rs:1303` | `total_usage.input_tokens` |
| TUI render | `caliban/src/tui.rs:744` | Displays as `X↑ (...) Y↓ tokens` |
| Session save footer | `caliban/src/main.rs:486` | Displays total = input + output, plus cache_extra suffix |

### `output_tokens`

Same hops as `input_tokens`. Both providers report this as "tokens generated this response," semantics consistent across providers. No discrepancy.

### `cache_creation_input_tokens`

| Hop | Location | Notes |
|---|---|---|
| Wire-format → IR (Anthropic, non-stream) | `crates/caliban-provider-anthropic/src/ir_convert.rs:189` | Pass-through |
| Wire-format → IR (Anthropic, stream) | `crates/caliban-provider-anthropic/src/stream_parse.rs:100` | Pass-through |
| Wire-format → IR (OpenAI) | hardcoded `None` (OpenAI does not differentiate creation from read) |
| Wire-format → IR (Gemini, Ollama) | hardcoded `None` (caching not implemented / not applicable) |
| Per-event accumulation | `Usage::merge` (response.rs:60-77) — `Some(a) + Some(b) = Some(a+b)`; `None + Some(x) = Some(x)` |
| Per-turn / per-run merge | Same `Usage::merge` — works correctly |
| RunEnd → UsageSummary build | `caliban/src/tui.rs:1306` | `total_usage.cache_creation_input_tokens` |
| TUI render | via `format_cache_suffix(_, cache_creation)` | Shows as `(C cache write)` when nonzero |

### `cache_read_input_tokens`

| Hop | Location | Notes |
|---|---|---|
| Wire-format → IR (Anthropic, non-stream) | `crates/caliban-provider-anthropic/src/ir_convert.rs:190` | Pass-through |
| Wire-format → IR (Anthropic, stream) | `crates/caliban-provider-anthropic/src/stream_parse.rs:101` | Pass-through |
| Wire-format → IR (OpenAI, non-stream) | `crates/caliban-provider-openai/src/ir_convert.rs:314-318, 333` | Extracted from `prompt_tokens_details.cached_tokens` |
| Wire-format → IR (OpenAI, stream) | `crates/caliban-provider-openai/src/stream_parse.rs:194-198` | Same |
| Wire-format → IR (Gemini, Ollama) | hardcoded `None` |
| Per-event / per-turn / per-run accumulation | `Usage::merge` |
| RunEnd → UsageSummary | `caliban/src/tui.rs:1305` |
| TUI render | via `format_cache_suffix(cache_read, _)` | Shows as `(R cached)` when nonzero |

## Aggregator invariant

A single function aggregates all four counters: `Usage::merge` in `crates/caliban-provider/src/response.rs:60-77`. Confirmed via grep that no other site adds to `input_tokens` or `output_tokens` outside of that impl:

```text
$ grep -rn "input_tokens\s*+=\|output_tokens\s*+=" crates/
crates/caliban-provider/src/response.rs:61:        self.input_tokens += other.input_tokens;
crates/caliban-provider/src/response.rs:62:        self.output_tokens += other.output_tokens;
```

The `merge` impl handles `Option` correctly:
- `Some(a) + Some(b) = Some(a + b)`
- `Some(a) + None = Some(a)` (and the symmetric case)
- `None + None = None`

So cache fields aggregate correctly across turns whether or not every turn populates them.

## Finding: cross-provider semantic inconsistency in `input_tokens`

The most important discovery from this trace.

**Anthropic** reports `usage.input_tokens` as the **uncached portion only**. Cached input is reported separately under `cache_creation_input_tokens` and `cache_read_input_tokens`. Total prompt size for billing = `input_tokens + cache_creation_input_tokens + cache_read_input_tokens` (with respective price multipliers).

**OpenAI** reports `usage.prompt_tokens` as the **total prompt size, including any cached portion**. The `prompt_tokens_details.cached_tokens` is a subset of `prompt_tokens`, not in addition to it.

caliban's IR currently carries these values unchanged from each provider, so the same struct field means different things depending on which provider produced the response. The TUI display reads:

```
[caliban: 1 turn · 50↑ (1000 cache write) 200↓ tokens]
```

For an **Anthropic** turn, this means: 50 fresh prompt tokens + 1000 written to cache → 1050 actual prompt tokens.

For an **OpenAI** turn with the same display:
```
[caliban: 1 turn · 1050↑ (1000 cached) 200↓ tokens]
```
1050 is already the total; the 1000 is informational.

A user comparing the two providers from caliban's output will not realize the `↑` number means different things.

## Fix

Normalize on **"input_tokens = total prompt size including cached portions"** (the OpenAI convention). It's more intuitive ("X tokens went in"), saves the user from doing addition, and matches what every other LLM-cost dashboard shows.

The fix is one-sided: in the Anthropic adapter, after reading `usage.input_tokens` from the wire, add `cache_creation_input_tokens` and `cache_read_input_tokens` so the IR's `input_tokens` becomes the total. The separated cache counters are preserved unchanged.

Two sites to fix:
- `crates/caliban-provider-anthropic/src/ir_convert.rs:185-191`
- `crates/caliban-provider-anthropic/src/stream_parse.rs:97-102`

After the fix, the display reads consistently:
- Anthropic turn 1: `[caliban: 1 turn · 1050↑ (1000 cache write) 200↓ tokens]`
- Anthropic turn 2: `[caliban: 1 turn · 1020↑ (1000 cached) 200↓ tokens]`
- OpenAI: `[caliban: 1 turn · 1050↑ (1000 cached) 200↓ tokens]`

All three mean "X total tokens went in, of which N were cached/written-to-cache."

## Regression test

A two-turn aggregation test is added in `caliban/tests/prompt_cache.rs` to lock in this behavior: each turn returns known counts via a mock Anthropic server, and the test asserts that all four counters in `total_usage` sum correctly.
