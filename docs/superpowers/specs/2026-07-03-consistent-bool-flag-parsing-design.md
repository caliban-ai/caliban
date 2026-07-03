# Consistent `=BOOL` parsing for boolean CLI flags (#223)

**Date:** 2026-07-03
**Ticket:** [#223](https://github.com/caliban-ai/caliban/issues/223)
**Status:** accepted

## Problem

Boolean CLI flags parse inconsistently. `--include-partial-messages=false`
hard-errors:

```
error: unexpected value 'false' for '--include-partial-messages' found; no more were expected
```

It is a bare on/off flag (clap `SetTrue`), but the sibling `--max-tokens-recovery
[<BOOL>]` — in the *same* `--help` section — *does* accept `=true`/`=false`. There
is no signal which is which until the error fires. This cost a headless launch
(QA finding A1).

## Decision

Make every plain boolean flag on the top-level `Args` accept an **optional
`=BOOL`** value, uniformly. `--flag`, `--flag=true`, and `--flag=false` all work;
absence keeps the current default (`false`, or the flag's default state).

### Mechanism

Each converted flag gets the same attribute set:

```rust
#[arg(
    long,
    require_equals = true,
    num_args = 0..=1,
    default_value_t = false,
    default_missing_value = "true",
    value_parser = parse_bool_flag,
)]
pub(crate) some_flag: bool,
```

Two non-obvious pieces:

- **`require_equals = true`** is load-bearing. `Args` has a positional `PROMPT`
  (`caliban "do the thing"`). Without `require_equals`, `num_args = 0..=1` would
  let `caliban --include-partial-messages "do the thing"` swallow the prompt as
  the flag's value. Requiring `=` means a value can *only* attach via
  `--flag=VALUE`; a bare `--flag` followed by a token leaves that token for the
  positional. This also removes the identical latent footgun on the existing
  `--max-tokens-recovery` (whose documented form is already `=false`), which we
  align to `require_equals` as well.

- **`parse_bool_flag`** — a small custom parser accepting
  `true/false/1/0/yes/no/on/off` (case-insensitive). clap's built-in `bool`
  parser accepts only `true`/`false`; several of these flags read from an env var
  (`CALIBAN_NO_MCP`, `CALIBAN_VERBOSE`, …) where operators commonly set `=1`.
  Using the flexible parser **preserves existing env-var truthiness** rather than
  turning `CALIBAN_NO_MCP=1` into a startup error. Invalid values (`=maybe`) are
  rejected with a clear message.

### Scope

All plain `bool` fields on `Args` (the positive toggles *and* the `--no-*`
negation flags). Uniformity is the whole point of the ticket, so we do not carve
out a subset. `--no-mcp=false` ("don't disable MCP") is a harmless, readable
bonus, not a footgun — nobody is forced to write it.

Out of scope: value-bearing flags that already work (`--max-tokens`,
`--temperature`, `--model`, …) and subcommand-local bools (`force`, `dry_run`) —
those take no positional-PROMPT and are not part of the reported inconsistency.
`--max-tokens-recovery` stays `Option<bool>` but gains `require_equals`.

## Consequences

- `--include-partial-messages=false` and every sibling now behave like
  `--max-tokens-recovery=false`. The reported failure is fixed and the whole
  boolean-flag surface is internally consistent.
- `--help` now shows `[=BOOL]` on these flags, signalling they take an optional
  value — satisfying the acceptance criterion's documentation branch too.
- Env-var flags keep working with `=1`/`=0` (and now also `=yes`/`=true`).
- The positional `PROMPT` is never swallowed, on any flag.

## Testing

Table-driven parse tests over the converted flags asserting the matrix:
`--flag` → true, `--flag=false` → false, `--flag=true` → true. Plus:

- **Positional-not-swallowed:** `--include-partial-messages <prompt>` parses
  `<prompt>` as the positional and the flag as `true`.
- **Env preservation:** `CALIBAN_NO_MCP=1` → `no_mcp == true`;
  `CALIBAN_NO_MCP=0` → `false`.
- **Rejection:** `--include-partial-messages=maybe` errors.
- **Regression:** existing bare-flag usages still parse to `true`.
