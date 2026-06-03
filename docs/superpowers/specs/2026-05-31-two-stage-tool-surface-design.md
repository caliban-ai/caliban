---
title: Two-stage tool surface — lazy MCP schema loading + ToolSearch
date: 2026-05-31
status: Proposed
author: john.ford2002@gmail.com
adr: adrs/0046-two-stage-tool-surface.md
---

# Two-stage tool surface — Design

**Date:** 2026-05-31
**Status:** Proposed
**Sub-project of:** caliban Rust agent harness
**Related ADRs:** [0017 — MCP client architecture](../../../adrs/0017-mcp-client-architecture.md),
[0021 — Sub-agent primitive](../../../adrs/0021-sub-agent-primitive.md),
[0023 — MCP v2 transports + OAuth](../../../adrs/0023-mcp-v2-transports-and-oauth.md),
[0026 — Settings layering](../../../adrs/0026-settings-layering.md),
[0037 — Sub-agent isolation + background fleet](../../../adrs/0037-subagent-isolation-and-background-fleet.md),
[0043 — `arc-swap` read-mostly shared state](../../../adrs/0043-arc-swap-shared-state.md),
[0045 — Permissions v2 + TOML primary config](../../../adrs/0045-permissions-v2-and-toml-primary-config.md)

## Goal

Stop sending every registered MCP tool's schema on every turn. Add a
single new built-in (`ToolSearch`) that lets the model discover MCP
tools on demand; activated tools persist for the rest of the session
in a sidecar set; the next turn's wire payload includes only the active
subset.

The TODO entry under *Performance & scaling (2026-05-31)* in
`docs/TODO.md` motivates this work: every turn snapshots
`self.tools.to_caliban_tools()` (`crates/caliban-agent-core/src/stream/mod.rs:497-523`),
inflating per-turn tokens, serialisation cost, and tool-selection
accuracy as MCP cardinality grows. This spec is the design half of the
multi-PR sequence the TODO calls for.

## Non-goals

- **Built-in tools laziness.** Built-ins (`Read`, `Grep`, `Glob`,
  `Edit`, `Bash`, `Write`, `WebFetch`, `WebSearch`, `TodoWrite`,
  `Skill`, `AgentTool`, `EnterPlanMode`/`ExitPlanMode`, memory tools)
  stay always-present in the wire payload. A future spec can lazy-fy
  them once the MCP-only path is shaken out.
- **Plugin tools.** Plugins contribute *skill roots* today, not direct
  registry entries (`crates/caliban-plugins/`, `caliban/src/main.rs:242`,
  `caliban/src/startup.rs:420-430`). Nothing to defer.
- **Activation persistence across session restart.** The activation
  set lives in-memory only in v1. Persisting it to the session JSONL
  is a v1.1 nice-to-have.
- **Auto-eviction on idle.** LRU eviction at the `max_active_schemas`
  cap is the only eviction trigger.
- **Telemetry counters.** `tools.activated.total` /
  `tools.evicted.total` ride with the OTel/cost layer (ADR 0033)
  in a follow-up.
- **`caliban tools` CLI.** Headless introspection beyond what
  `/context` already shows is deferred.
- **Schema-size estimator on `/usage`.** Counterfactual reporting
  ("would have saved ~8K tokens") is hard to characterise honestly.

## Problems this spec solves

Drawn from a fresh read of `caliban-agent-core::stream`,
`caliban-agent-core::registry`, `caliban-mcp-client::manager`,
`caliban-agent-core::startup::install_sub_agent`, and
`caliban-settings::schema`.

### Wire payload growth

`ToolRegistry::to_caliban_tools()`
(`crates/caliban-agent-core/src/registry.rs:59-69`) clones every
registered tool's name + description + `input_schema` into a
`Vec<caliban_provider::Tool>` on every turn. With one MCP server
(e.g. `mcp__github__*` advertising ~15 tools at ~500 schema tokens
each), per-turn overhead is ~7.5K tokens of dormant tool advertising.
Three such servers approaches ~22K tokens/turn before any history.

### Tool-selection accuracy drift

When the wire palette includes dozens of tools whose schemas the
model didn't need this turn, tool-selection accuracy on the relevant
subset degrades. This is well-characterised in the OpenAI / Anthropic
function-calling literature; the cure is to surface a smaller, more
relevant palette.

### Sub-agent inheritance hazard

Sub-agents snapshot the parent registry at install time
(`caliban/src/startup.rs:1161-1196`). If the parent has a giant MCP
surface, every sub-agent invocation pays that cost too. The allowlist
mechanism (`AgentTool` frontmatter `tools: [...]`) helps but is
opt-in per sub-agent definition.

### Discovery vs. eagerness

The model cannot search the registry for tools it doesn't already
see. The current design exposes everything so the model can pick;
the proposed design hides MCP tools but gives the model a discovery
primitive (`ToolSearch`) that surfaces matches with full schemas in
a single round-trip.

## Design

### Settings — new `[tools]` section

```toml
# caliban.toml / settings.toml
[tools]
lazy_mcp = false                # v1 default; opt-in
max_active_schemas = 24         # soft cap; LRU evict on overflow
```

Per-server override in `mcp.toml` (or `[mcp_servers.X]` under unified
settings):

```toml
[mcp_servers.silverbullet]
command = "..."
lazy = false                    # opt this server back to eager when lazy_mcp=true
```

`lazy: Option<bool>` on a server entry: `Some(false)` overrides the
global; `Some(true)` is the same as the global default; `None` (the
common case) follows the global.

Schema additions land in
`crates/caliban-settings/src/schema.json` (a `tools` object and a
`mcp_servers.<name>.lazy` boolean). Validation continues at WARN per
ADR 0026.

### Built-in: `ToolSearch`

A new built-in registered alongside `Read`, `Grep`, etc. in the
default palette built by `caliban::startup::build_registry`
(`caliban/src/startup.rs:374-433`).

**Input schema:**

```json
{
  "type": "object",
  "properties": {
    "query": {
      "type": "string",
      "description": "Substring or word-prefix query matched against MCP tool names and descriptions. Use 'select:name1,name2' to fetch specific tools by exact name."
    },
    "max_results": {
      "type": "integer",
      "minimum": 1,
      "maximum": 25,
      "default": 10
    }
  },
  "required": ["query"]
}
```

**Description** (sent to the model):

> Search for MCP tools by name or description. Matching tools are
> activated for the rest of this session — their full schemas appear
> in your tool list on subsequent turns and you can call them
> directly. Returns up to `max_results` matches with name,
> description, and JSON Schema for each. Use `select:foo,bar` to
> fetch specific tools by exact name. When MCP loading is disabled
> this tool returns a no-op message.

**Invocation:** ranks registry entries whose `is_mcp(name)` is true
(`name.starts_with("mcp__")`). Score order: exact-substring on name >
word-prefix on name > exact-substring on description > fuzzy on name.
Cap by `max_results`. For each match: call
`activation_set.rcu(|s| s.activate(name))`, then format a `TextBlock`
of the form:

```
Activated 3 tools for this session:

mcp__github__create_issue
  Create a new GitHub issue.
  Schema:
  {"type":"object","properties":{...}}

mcp__github__list_issues
  ...
```

Activations are idempotent (re-search of an already-active tool just
bumps LRU). Eviction on overflow is reported in the response:

```
Activated 3 tools. Evicted 1 to stay under cap=24:
  - mcp__postgres__describe_table (least recently used)
```

### Sidecar state: `McpActivationSet`

New module `crates/caliban-agent-core/src/mcp_activation.rs`.

```rust
pub struct McpActivationSet {
    cap: usize,
    lru: VecDeque<String>,   // newest at front; oldest at back
    active: BTreeSet<String>,
}

impl McpActivationSet {
    pub fn new(cap: usize) -> Self { ... }
    pub fn is_active(&self, name: &str) -> bool { ... }
    pub fn iter_active(&self) -> impl Iterator<Item = &str> { ... }
    pub fn len(&self) -> usize { ... }

    /// Activate `name`. Returns the evicted name when overflow
    /// triggers an LRU drop. Idempotent: re-activating a current
    /// member bumps it to the front of the LRU.
    pub fn activate(&mut self, name: &str) -> Option<String> { ... }

    /// Snapshot for sub-agent inheritance.
    pub fn snapshot(&self) -> Self { ... }
}
```

`Agent` (`crates/caliban-agent-core/src/agent.rs`) gains:

```rust
mcp_active: Arc<ArcSwap<McpActivationSet>>,
mcp_eager_servers: Arc<HashSet<String>>,  // resolved at startup from per-server lazy=false
```

The `ArcSwap` follows ADR-0043. Reads (every turn) are cheap; writes
(only when `ToolSearch` activates) go through `rcu`.

### Wire-payload filter

New module `crates/caliban-agent-core/src/wire_filter.rs`:

```rust
pub struct WireFilter<'a> {
    pub lazy_mcp: bool,
    pub active: &'a McpActivationSet,
    pub eager_servers: &'a HashSet<String>,
}

pub struct WireFilterResult {
    pub tools: Vec<caliban_provider::Tool>,
    pub dropped_mcp_count: usize,
}

impl ToolRegistry {
    pub fn to_caliban_tools_filtered(&self, f: &WireFilter<'_>) -> WireFilterResult { ... }
}
```

Filter rules per entry:

1. Non-MCP tool (name does not start with `mcp__`) → include.
2. `lazy_mcp == false` → include all.
3. MCP tool whose server segment is in `eager_servers` → include.
4. MCP tool whose name is in `active.iter_active()` → include.
5. Otherwise → drop; bump `dropped_mcp_count`.

The MCP "server segment" is the second `__`-delimited field:
`mcp__<server>__<tool>` → `<server>`.

### Stream integration

Two changes at the request-build site
(`crates/caliban-agent-core/src/stream/mod.rs:497-523`):

```rust
let active = self.mcp_active.load();
let filter = WireFilter {
    lazy_mcp: self.config.tools.lazy_mcp,
    active: &active,
    eager_servers: &self.mcp_eager_servers,
};
let WireFilterResult { tools, dropped_mcp_count } =
    self.tools.to_caliban_tools_filtered(&filter);
let mut req_tools = tools;

// existing prompt-cache call
if self.prompt_cache { /* unchanged */ }

// new: splice the deferred-block into the system prompt when relevant
let system_prompt = if filter.lazy_mcp && dropped_mcp_count > 0 {
    splice_deferred_block(&self.system_prompt, dropped_mcp_count)
} else {
    self.system_prompt.clone()
};
```

`splice_deferred_block` adds a fixed paragraph immediately before the
existing tool list:

> *Some MCP tools are deferred to keep your tool palette lean. Use
> the `ToolSearch` tool with a substring query to discover and
> activate them when needed; activated tools persist for the rest of
> the session. `N` MCP tools are currently deferred.*

`N` is `dropped_mcp_count`.

### Sub-agent inheritance

`AgentTool` frontmatter gains:

```yaml
---
name: research
description: ...
tools: [Grep, Read, mcp__github__list_issues]   # existing allowlist
inherit_active_mcp: true                          # new; default true
---
```

`caliban/src/startup.rs:install_sub_agent` (around lines 1161-1196)
gains a snapshot step:

```rust
let parent_active_snapshot = if frontmatter.inherit_active_mcp.unwrap_or(true) {
    parent.mcp_active.load().snapshot()
} else {
    McpActivationSet::new(child_cap)
};
let child_active = Arc::new(ArcSwap::from_pointee(parent_active_snapshot));
```

The allowlist mechanism still applies: if a sub-agent declares
`tools: [...]`, only those names pass the existing filter, regardless
of whether they were active in the parent.

### `/context` integration

`caliban/src/tui/slash/context.rs` (or wherever `/context` aggregates
its breakdown) gains one line when `tools.lazy_mcp = true`:

```
MCP active: 3/24
  mcp__github__create_issue (12 turns ago)
  mcp__github__list_issues  (3 turns ago)
  mcp__postgres__query       (1 turn ago)
```

When `lazy_mcp = false` the line is omitted (everything is always
active; no information to surface).

### Default-off + opt-in path

`tools.lazy_mcp` defaults to `false` in v1. Users opt in explicitly
in their `settings.toml` or `caliban.toml`. The TODO entry, the spec,
and a one-line note in the README + parity matrix describe the
opt-in.

After 2-4 weeks of validation on real MCP-heavy workloads, a v1.1
follow-up flips the default to `true` and adds a `tools.lazy_mcp =
false` migration line to the deprecation log. That flip is **not**
in v1 scope.

## Components in detail

### `McpActivationSet` semantics

- `activate(name)` — if already in `active`, find it in `lru`, move
  to front, return `None`. Otherwise: insert in `active`; push to
  front of `lru`; if `lru.len() > cap`, pop back and remove from
  `active`, returning the evicted name.
- `iter_active()` — iterates in **MRU order** (front of `lru`
  first) so `/context` lists newest activations on top.
- `snapshot()` — `Self { cap, lru: self.lru.clone(), active:
  self.active.clone() }`. Used by sub-agent inheritance.
- `cap == 0` — special case; `lazy_mcp` is effectively disabled; a
  WARN is logged at settings load and the agent treats it as
  `lazy_mcp = false`.

### `ToolSearch` interaction with `caliban-mcp-client`

`ToolSearch` does **not** call back into the MCP server. It searches
the *already-registered* tools in `ToolRegistry` (everything
`manager.register_all(...)` placed there at startup,
`caliban-mcp-client::manager:246`). The schemas are already
materialised — what changes is whether they ride the wire each turn,
not whether they exist in the registry.

This means servers that fail to start, or tools that fail to register
during the `list_tools` handshake, are still invisible to
`ToolSearch`. That's correct: the user already gets a WARN at startup
about the failed server.

### Naming convention dependency

The filter relies on MCP tools being prefixed `mcp__<server>__<tool>`.
This is the existing convention (ADR 0017,
`caliban-mcp-client::manager::full_name`), so the filter doesn't
introduce a new dependency. A regression test asserts the prefix
convention in `caliban-agent-core/tests/wire_filter.rs` so a future
naming change is forced to update the filter.

### Interaction with caching

`apply_prompt_cache` (`crates/caliban-agent-core/src/cache.rs`) marks
the last user message and tool list with `cache_control:
Ephemeral` when over `min_cache_block_tokens`. Lazy MCP changes the
tool list across turns when activations happen — every activation
invalidates the tool-list cache prefix. This is acceptable because
(a) activations are uncommon (a few per session), (b) the post-
invalidation tool list is smaller than the eager equivalent, so even
unaccelerated turns are cheaper.

A future optimisation: split the cache marker so the eager-built-in
portion of the tool list (which never changes) gets its own cache
block. Out of scope for v1.

## Data flow

```
turn N (lazy_mcp=true, 0 MCP tools active):
  stream/mod.rs:497 builds req
    filter drops all MCP tools; dropped_mcp_count = 23
    req.tools = [Read, Grep, Glob, ..., ToolSearch, AgentTool]    (14 entries)
    system_prompt += DEFERRED_BLOCK("23 MCP tools currently deferred")
  model decides it needs GitHub work; emits:
    tool_use { name: "ToolSearch", input: { query: "github" } }
  dispatch invokes ToolSearch.invoke:
    matches = registry.iter()
        .filter(|t| is_mcp(t.name()) && matches_query(t, "github"))
        .take(10)
        .collect()      // -> 4 matches
    for m in &matches {
        agent.mcp_active.rcu(|s| { let mut s2 = (**s).clone();
                                   s2.activate(m.name()); Arc::new(s2) });
    }
    return TextBlock { "Activated 4 tools: ..." }

turn N+1 (4 MCP tools active):
  stream/mod.rs:497 builds req
    filter passes through built-ins + the 4 active MCP tools
    req.tools = [Read, Grep, Glob, ..., ToolSearch, AgentTool,
                 mcp__github__create_issue, mcp__github__list_issues,
                 mcp__github__add_comment, mcp__github__close_issue]   (18 entries)
    system_prompt += DEFERRED_BLOCK("19 MCP tools currently deferred")
  model calls one of the active tools; normal dispatch path runs.

sub-agent spawn from turn N+1:
  install_sub_agent reads frontmatter.inherit_active_mcp (default true)
    child_active = parent.mcp_active.load().snapshot()    // 4 tools
  child runs with its own ArcSwap<McpActivationSet>
    can search/activate independently; parent unaffected
```

## Error handling

| Condition | Behaviour |
|---|---|
| `ToolSearch` invoked with no matches | Returns `TextBlock { "No MCP tools matched 'X'. Available servers: github, postgres." }`. Not an error. |
| `ToolSearch` invoked when `lazy_mcp=false` | Returns `TextBlock { "Lazy MCP loading is disabled; all MCP tools are already in your palette." }`. Not an error — graceful no-op. |
| `ToolSearch` invoked when `lazy_mcp=true` but no MCP servers configured | Returns `TextBlock { "No MCP servers are configured." }`. Not an error. |
| LRU eviction during activation | Evicted name reported in the response text so the model sees what dropped. The evicted tool is **still in the registry** (dispatch works if the model issues a tool_use by name), but absent from subsequent wire payloads until re-activated. |
| Model issues `tool_use` for a deactivated/never-activated MCP tool | Normal registry dispatch runs (the entry exists). On success, the dispatch path **re-activates** the tool — same effect as if the model had called `ToolSearch` first. This means the model can short-circuit search-then-call if it remembered a tool name from prior history. |
| `tools.max_active_schemas = 0` | Treated as `lazy_mcp = false`; WARN at settings load. |
| Per-server `lazy = false` on a non-existent server | WARN at settings load; entry ignored at filter time. |
| MCP server registers no tools | Server is irrelevant to the filter (no entries to drop); no special handling. |
| Sub-agent has `inherit_active_mcp: true` but parent's set is empty | Child starts with empty active set. Normal case at session start. |
| Sub-agent allowlist excludes an active MCP tool | The allowlist filter (existing) drops it before the wire filter runs. No conflict. |

## Testing

Each test below corresponds to one of the components above. All in
the `caliban-agent-core` crate's `tests/` directory.

### Unit tests

- `mcp_activation.rs`
  - `activate_idempotent_bumps_lru`
  - `evicts_oldest_at_cap`
  - `snapshot_independent_after_mutate`
  - `cap_zero_treated_as_disabled` (paired with settings load WARN)
  - `iter_active_returns_mru_first`
- `wire_filter.rs`
  - `passes_through_when_lazy_mcp_false`
  - `drops_inactive_mcp_when_lazy_mcp_true`
  - `passes_inactive_mcp_when_server_in_eager_list`
  - `counts_dropped_for_system_prompt_block`
  - `non_mcp_tools_always_pass`
  - `prefix_regression: mcp__server__tool name shape required`

### Integration tests

- `tool_search_integration.rs` — fake registry with 1 built-in
  (`Read`) + 5 MCP tools across 2 servers. Verify:
  - lazy_mcp=true, no activations → wire has 1 built-in + ToolSearch.
  - ToolSearch with query `"server_a"` → activates 2 tools; next
    wire has 1 built-in + ToolSearch + 2 MCP tools.
  - Cap=2 + activating a third → first evicted, response text names
    the eviction.
  - lazy_mcp=false → wire always has 1 built-in + 5 MCP tools.
  - `select:mcp__server_a__one,mcp__server_b__two` → activates exactly
    those two by exact name, ignoring query ranking; missing names
    listed in the response.
- `agent_tool_inheritance.rs` — parent activates 2 MCP tools; spawn
  child with `inherit_active_mcp: true` → child wire has the 2 + its
  own built-ins. Spawn child with `inherit_active_mcp: false` → child
  wire has only its built-ins. Allowlist `tools: [mcp__a__one]` →
  child wire has only the one allowed MCP tool, even if parent had
  more active.
- `system_prompt_deferred_block.rs` — snapshot the spliced system
  prompt when lazy_mcp=true and dropped_mcp_count > 0; assert
  absence when lazy_mcp=false or dropped_mcp_count = 0.

### Regression / golden-path tests

- Existing tool-registry tests (`caliban-agent-core/tests/registry*.rs`)
  must pass unchanged with `tools.lazy_mcp` defaulting to false.
- Existing sub-agent tests must pass unchanged; the new
  `inherit_active_mcp` field defaults to true and is a no-op when no
  activations exist.

### Manual / observational

- Launch caliban against a project with `mcp.toml` configured for
  one MCP server (e.g. `silverbullet`) and `tools.lazy_mcp = true`.
  Observe that the system prompt contains the deferred block, the
  tool list omits `mcp__silverbullet__*`, and a ToolSearch call
  activates them.
- `/context` shows the active set after activation.
- `tracing` at `caliban::wire_filter` debug shows
  `dropped=N kept=M` per turn.

## Acceptance criteria

The spec is implemented when:

1. `tools.lazy_mcp = false` (default) → behaviour is byte-identical
   to today; all existing tests pass; no system-prompt change.
2. `tools.lazy_mcp = true` with no MCP servers → behaviour is
   identical to default; `ToolSearch` is registered but inert.
3. `tools.lazy_mcp = true` with N MCP tools across M servers and
   none eager-flagged → wire payload contains 0 MCP tools by
   default; system prompt contains the deferred block; `/context`
   shows `MCP active: 0/cap`.
4. After a `ToolSearch` call that matches `k` tools (k ≤ max_results
   ≤ cap), wire payload on the next turn includes those `k` tools;
   `/context` shows `MCP active: k/cap`.
5. After `cap + 1` distinct activations, the first activation is
   evicted (LRU), reported in the ToolSearch response text, and the
   wire payload on the next turn omits it.
6. Sub-agent spawned with `inherit_active_mcp: true` (default) sees
   the parent's active set in its initial wire payload; with `false`,
   sees only its built-ins + ToolSearch.
7. Per-server `lazy = false` override pins that server's tools as
   always-included regardless of activation.
8. All assertion tests under "Unit", "Integration", and "Regression"
   above pass.

## Migration

- `settings.toml` schema additions: backward-compatible; absent keys
  default per spec.
- `mcp.toml` per-server `lazy` field: backward-compatible; absent
  field follows the global.
- `AgentTool` frontmatter `inherit_active_mcp`: backward-compatible;
  absent field defaults to `true`. Existing sub-agents that didn't
  declare it get the most-conservative-but-most-useful behavior.
- No CLI breaking changes.
- Parity matrix rows F.ToolSearch and F.WaitForMcpServers move from
  🔴 → 🟡 in v1 (machinery shipped; default off). They move to ✅ in
  v1.1 when the default flips.

## Open questions for v1.1

- Should activation persist across session restart? Easy to add by
  serialising `McpActivationSet` into the session JSONL (one extra
  field), but not in v1.
- Should `/tools` overlay surface the dormant set and allow manual
  activation/deactivation from the TUI?
- Should `tools.max_active_schemas` default be sized by token budget
  rather than a count? Probably out of scope for v1.
- A `caliban tools list/search/activate/deactivate` CLI for headless
  workflows. Currently the headless path can only activate via the
  model calling ToolSearch — fine for most v1 usage.
