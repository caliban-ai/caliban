# Two-stage tool surface implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add lazy MCP schema loading via a new `ToolSearch` built-in and a sidecar `McpActivationSet`, so each turn ships only the activated subset of MCP tools instead of the full registry.

**Architecture:** Sidecar `Arc<ArcSwap<McpActivationSet>>` lives on `Agent` (per ADR-0043). A new `WireFilter` applies at the per-turn request-build site (`stream/mod.rs:497-523`). `ToolSearch` lives in `caliban-tools-builtin` and holds an `Arc<McpClientManager>` to enumerate MCP tools at invoke time. Sub-agents snapshot the parent's activation set on install when frontmatter `inherit_active_mcp` is true (default).

**Tech stack:** Rust workspace, async-trait + tokio, `arc-swap` 1.7, serde, JSON schema (draft-07) at WARN-only validation.

**Spec:** `docs/superpowers/specs/2026-05-31-two-stage-tool-surface-design.md`
**ADR:** `adrs/0046-two-stage-tool-surface.md`
**Branch:** `strategic/two-stage-tool-surface`

---

## Phase 1 — Settings + activation-set foundation (no behavior change)

### Task 1: Add `[tools]` section to settings schema

**Files:**
- Modify: `crates/caliban-settings/src/schema.json` (add `tools` property under top-level `properties`)

- [ ] **Step 1: Add the schema fragment**

In `crates/caliban-settings/src/schema.json`, find the line `"effort": { "type": "string", "enum": ["low", "medium", "high", "max", "auto"] }` and add this property immediately before it:

```json
    "tools": {
      "type": "object",
      "properties": {
        "lazy_mcp": { "type": "boolean", "default": false },
        "max_active_schemas": { "type": "integer", "minimum": 0, "default": 24 }
      },
      "additionalProperties": false
    },
```

- [ ] **Step 2: Verify schema parses**

Run: `cargo test -p caliban-settings schema`
Expected: PASS — schema is loaded at test time via the embedded `include_str!` in `schema.rs`.

- [ ] **Step 3: Commit**

```bash
git add crates/caliban-settings/src/schema.json
git commit -m "feat(settings): add tools section to schema (lazy_mcp, max_active_schemas)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 2: Add `ToolsConfig` struct + `Settings.tools` field

**Files:**
- Modify: `crates/caliban-settings/src/settings.rs` (add `ToolsConfig` struct + `tools` field)
- Test: `crates/caliban-settings/tests/integration.rs` (or wherever the round-trip tests live; search for a recent settings-key test like `fallback_model` first)

- [ ] **Step 1: Add the struct**

In `crates/caliban-settings/src/settings.rs`, add immediately after the `StatuslineConfig` struct (or wherever sibling config structs live — look at the file for where `StatuslineConfig` is declared):

```rust
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ToolsConfig {
    #[serde(default)]
    pub lazy_mcp: Option<bool>,
    #[serde(default)]
    pub max_active_schemas: Option<usize>,
}
```

- [ ] **Step 2: Add the field to `Settings`**

In the `Settings` struct (around line 183-283), add this field immediately after `min_cache_block_tokens` (or in the same logical group of agent-related knobs):

```rust
    pub tools: Option<ToolsConfig>,
```

- [ ] **Step 3: Update `Default` impl and any explicit field-by-field constructors** in the same file so `tools` defaults to `None`. Look for `impl Default for Settings` (or `#[derive(Default)]`) — most fields use derive defaults; nothing extra to do if `Settings` uses `#[derive(Default)]`.

- [ ] **Step 4: Write a round-trip test**

In `crates/caliban-settings/tests/integration.rs` (or wherever similar `model`/`fallback_model` round-trip tests live), add:

```rust
#[test]
fn tools_config_roundtrip() {
    let toml = r#"
[tools]
lazy_mcp = true
max_active_schemas = 32
"#;
    let s: caliban_settings::Settings = toml::from_str(toml).unwrap();
    let tools = s.tools.expect("tools should parse");
    assert_eq!(tools.lazy_mcp, Some(true));
    assert_eq!(tools.max_active_schemas, Some(32));
}

#[test]
fn tools_config_absent_leaves_settings_tools_none() {
    let toml = "model = \"test\"\n";
    let s: caliban_settings::Settings = toml::from_str(toml).unwrap();
    assert!(s.tools.is_none());
}
```

- [ ] **Step 5: Run tests + commit**

Run: `cargo test -p caliban-settings tools_config`
Expected: PASS (both tests).

```bash
git add crates/caliban-settings/src/settings.rs crates/caliban-settings/tests/
git commit -m "feat(settings): add ToolsConfig (lazy_mcp + max_active_schemas)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 3: Add `lazy` field to MCP `ServerConfig`

**Files:**
- Modify: `crates/caliban-mcp-client/src/config.rs:93-123` (add `lazy: Option<bool>`)
- Test: `crates/caliban-mcp-client/tests/config.rs` (or wherever ServerConfig parsing tests live)

- [ ] **Step 1: Add the field**

In `crates/caliban-mcp-client/src/config.rs` `ServerConfig` struct (line ~93-123), add immediately after the `disabled: bool` field:

```rust
    /// When `tools.lazy_mcp = true` globally, individual servers can
    /// opt back to eager loading by setting `lazy = false`. `None`
    /// follows the global default.
    #[serde(default)]
    pub lazy: Option<bool>,
```

- [ ] **Step 2: Update `Default` impl** for `ServerConfig` (same file) to include `lazy: None`.

- [ ] **Step 3: Write a test**

Find the existing `ServerConfig` parsing test (search `cargo run -p caliban-mcp-client --` or grep for `ServerConfig` in `tests/`). Add:

```rust
#[test]
fn server_config_lazy_parses() {
    let toml = r#"
command = "my-mcp"
lazy = false
"#;
    let cfg: caliban_mcp_client::ServerConfig = toml::from_str(toml).unwrap();
    assert_eq!(cfg.lazy, Some(false));
}

#[test]
fn server_config_lazy_absent_is_none() {
    let toml = r#"command = "my-mcp""#;
    let cfg: caliban_mcp_client::ServerConfig = toml::from_str(toml).unwrap();
    assert_eq!(cfg.lazy, None);
}
```

- [ ] **Step 4: Run tests + commit**

Run: `cargo test -p caliban-mcp-client server_config_lazy`
Expected: PASS.

```bash
git add crates/caliban-mcp-client/src/config.rs crates/caliban-mcp-client/tests/
git commit -m "feat(mcp): add per-server lazy override to ServerConfig

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 4: Create `McpActivationSet` module

**Files:**
- Create: `crates/caliban-agent-core/src/mcp_activation.rs`
- Modify: `crates/caliban-agent-core/src/lib.rs` (add `pub mod mcp_activation;`)
- Test: `crates/caliban-agent-core/tests/mcp_activation.rs`

- [ ] **Step 1: Write the failing test first**

Create `crates/caliban-agent-core/tests/mcp_activation.rs`:

```rust
use caliban_agent_core::mcp_activation::McpActivationSet;

#[test]
fn activate_idempotent_bumps_lru() {
    let mut s = McpActivationSet::new(8);
    s.activate("mcp__a__one");
    s.activate("mcp__a__two");
    let evicted = s.activate("mcp__a__one");
    assert!(evicted.is_none(), "no eviction at len < cap");
    let order: Vec<&str> = s.iter_active().collect();
    assert_eq!(order, vec!["mcp__a__one", "mcp__a__two"], "re-activate moves to MRU");
}

#[test]
fn evicts_oldest_at_cap() {
    let mut s = McpActivationSet::new(2);
    assert!(s.activate("a").is_none());
    assert!(s.activate("b").is_none());
    let evicted = s.activate("c");
    assert_eq!(evicted, Some("a".to_string()));
    assert!(!s.is_active("a"));
    assert!(s.is_active("b"));
    assert!(s.is_active("c"));
}

#[test]
fn snapshot_independent_after_mutate() {
    let mut s = McpActivationSet::new(4);
    s.activate("a");
    let snap = s.snapshot();
    s.activate("b");
    assert!(s.is_active("b"));
    assert!(!snap.is_active("b"));
    assert!(snap.is_active("a"));
}

#[test]
fn iter_active_returns_mru_first() {
    let mut s = McpActivationSet::new(4);
    s.activate("a");
    s.activate("b");
    s.activate("c");
    let order: Vec<&str> = s.iter_active().collect();
    assert_eq!(order, vec!["c", "b", "a"], "front of LRU is MRU");
}

#[test]
fn cap_zero_disables_activation() {
    let mut s = McpActivationSet::new(0);
    let evicted = s.activate("a");
    // cap == 0 → activation is a no-op; nothing stored
    assert_eq!(evicted, None);
    assert!(!s.is_active("a"));
    assert_eq!(s.len(), 0);
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test -p caliban-agent-core --test mcp_activation`
Expected: FAIL with "could not find `mcp_activation` in `caliban_agent_core`".

- [ ] **Step 3: Implement the module**

Create `crates/caliban-agent-core/src/mcp_activation.rs`:

```rust
//! Sidecar activation state for lazy MCP tool loading (ADR-0046).
//!
//! `McpActivationSet` tracks which MCP tools are currently active for
//! the session. Sticky per session; LRU eviction at `cap`.

use std::collections::{BTreeSet, VecDeque};

#[derive(Debug, Clone)]
pub struct McpActivationSet {
    cap: usize,
    /// Newest at front, oldest at back. Eviction pops from back.
    lru: VecDeque<String>,
    active: BTreeSet<String>,
}

impl McpActivationSet {
    /// Create an activation set with the given soft cap. `cap == 0`
    /// disables activation (every `activate()` is a no-op).
    #[must_use]
    pub fn new(cap: usize) -> Self {
        Self {
            cap,
            lru: VecDeque::with_capacity(cap),
            active: BTreeSet::new(),
        }
    }

    /// Whether `name` is currently active.
    #[must_use]
    pub fn is_active(&self, name: &str) -> bool {
        self.active.contains(name)
    }

    /// Count of active entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.active.len()
    }

    /// Whether the set is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }

    /// Iterate active entries in MRU order (newest first).
    pub fn iter_active(&self) -> impl Iterator<Item = &str> {
        self.lru.iter().map(String::as_str)
    }

    /// Activate `name`. Returns the evicted name when overflow
    /// triggers an LRU drop. Idempotent: re-activating a current
    /// member bumps it to the front of the LRU.
    pub fn activate(&mut self, name: &str) -> Option<String> {
        if self.cap == 0 {
            return None;
        }
        // Re-activation: just bump LRU position.
        if self.active.contains(name) {
            if let Some(idx) = self.lru.iter().position(|n| n == name) {
                let _ = self.lru.remove(idx);
            }
            self.lru.push_front(name.to_string());
            return None;
        }
        self.active.insert(name.to_string());
        self.lru.push_front(name.to_string());
        if self.lru.len() > self.cap {
            if let Some(evicted) = self.lru.pop_back() {
                self.active.remove(&evicted);
                return Some(evicted);
            }
        }
        None
    }

    /// Snapshot for sub-agent inheritance.
    #[must_use]
    pub fn snapshot(&self) -> Self {
        self.clone()
    }
}
```

- [ ] **Step 4: Wire into lib.rs**

In `crates/caliban-agent-core/src/lib.rs`, add (in the module declarations near the top):

```rust
pub mod mcp_activation;
```

- [ ] **Step 5: Run tests to verify pass**

Run: `cargo test -p caliban-agent-core --test mcp_activation`
Expected: PASS (all 5 tests).

- [ ] **Step 6: Commit**

```bash
git add crates/caliban-agent-core/src/mcp_activation.rs \
        crates/caliban-agent-core/src/lib.rs \
        crates/caliban-agent-core/tests/mcp_activation.rs
git commit -m "feat(agent-core): McpActivationSet — LRU sidecar for lazy MCP tool loading

Tracks active MCP tools per session; soft cap with LRU eviction;
snapshot() for sub-agent inheritance. ADR-0046.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 5: Add `mcp_active` + `mcp_eager_servers` to `Agent`

**Files:**
- Modify: `crates/caliban-agent-core/src/agent.rs` (Agent struct fields + builder + Config)

- [ ] **Step 1: Extend `AgentConfig`**

In `crates/caliban-agent-core/src/agent.rs`, in `AgentConfig` (line ~34-78), add immediately after `min_cache_block_tokens`:

```rust
    // ADR-0046 — lazy MCP tool loading
    pub lazy_mcp: bool,
    pub max_active_schemas: usize,
```

In the `Default` impl (line ~80-111), add:

```rust
    lazy_mcp: false,
    max_active_schemas: 24,
```

- [ ] **Step 2: Add Agent fields**

In the `Agent` struct (line ~133-165), add at the bottom:

```rust
    pub(crate) mcp_active: std::sync::Arc<arc_swap::ArcSwap<crate::mcp_activation::McpActivationSet>>,
    pub(crate) mcp_eager_servers: std::sync::Arc<std::collections::HashSet<String>>,
```

- [ ] **Step 3: Wire in the builder / construction**

Find where `Agent` is constructed (likely `AgentBuilder::build` or similar — grep for `Agent {`). Add to the construction:

```rust
mcp_active: std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(
    crate::mcp_activation::McpActivationSet::new(config.max_active_schemas),
)),
mcp_eager_servers: std::sync::Arc::new(std::collections::HashSet::new()),
```

The builder probably has a `with_mcp_eager_servers(set: HashSet<String>)` setter to add. Look at how `with_*` builder methods look for `provider`, `tools`, etc. and add a matching one if appropriate (the populated set comes from the binary's startup wiring later).

- [ ] **Step 4: Add a helper accessor**

In the `impl Agent` block, add:

```rust
pub fn mcp_active(&self) -> std::sync::Arc<arc_swap::ArcSwap<crate::mcp_activation::McpActivationSet>> {
    std::sync::Arc::clone(&self.mcp_active)
}

pub fn mcp_eager_servers(&self) -> std::sync::Arc<std::collections::HashSet<String>> {
    std::sync::Arc::clone(&self.mcp_eager_servers)
}
```

These are needed by ToolSearch (via the builder) and `install_sub_agent`.

- [ ] **Step 5: Run cargo check**

Run: `cargo check -p caliban-agent-core`
Expected: clean — type wires up. If there's any constructor calling `Agent {` directly without the new fields, fix.

- [ ] **Step 6: Commit**

```bash
git add crates/caliban-agent-core/src/agent.rs
git commit -m "feat(agent-core): wire mcp_active + mcp_eager_servers into Agent

AgentConfig gains lazy_mcp + max_active_schemas. Agent holds
Arc<ArcSwap<McpActivationSet>> per ADR-0043 and Arc<HashSet<String>>
of eager-flagged server names. Accessors added for ToolSearch and
sub-agent install_sub_agent.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Phase 2 — Wire filter (no integration yet)

### Task 6: Create `WireFilter` module + filtered registry method

**Files:**
- Create: `crates/caliban-agent-core/src/wire_filter.rs`
- Modify: `crates/caliban-agent-core/src/registry.rs` (add `to_caliban_tools_filtered`)
- Modify: `crates/caliban-agent-core/src/lib.rs` (`pub mod wire_filter;`)
- Test: `crates/caliban-agent-core/tests/wire_filter.rs`

- [ ] **Step 1: Write the failing test first**

Create `crates/caliban-agent-core/tests/wire_filter.rs`:

```rust
use std::collections::HashSet;
use std::sync::Arc;

use caliban_agent_core::mcp_activation::McpActivationSet;
use caliban_agent_core::registry::ToolRegistry;
use caliban_agent_core::wire_filter::WireFilter;
use caliban_agent_core::tool::Tool;
use caliban_provider::ContentBlock;
use async_trait::async_trait;

/// Minimal Tool impl for tests.
struct StubTool {
    name: String,
    schema: serde_json::Value,
}

impl StubTool {
    fn new(name: &str) -> Self {
        Self { name: name.to_string(), schema: serde_json::json!({"type":"object"}) }
    }
}

#[async_trait]
impl Tool for StubTool {
    fn name(&self) -> &str { &self.name }
    fn description(&self) -> &str { "stub" }
    fn input_schema(&self) -> &serde_json::Value { &self.schema }
    async fn invoke(
        &self,
        _input: serde_json::Value,
        _cx: caliban_agent_core::tool::ToolContext,
    ) -> Result<Vec<ContentBlock>, caliban_agent_core::tool::ToolError> {
        Ok(vec![])
    }
}

fn make_registry() -> ToolRegistry {
    let mut r = ToolRegistry::new();
    r.register(Arc::new(StubTool::new("Read")));
    r.register(Arc::new(StubTool::new("mcp__server_a__one")));
    r.register(Arc::new(StubTool::new("mcp__server_a__two")));
    r.register(Arc::new(StubTool::new("mcp__server_b__one")));
    r
}

#[test]
fn passes_through_when_lazy_mcp_false() {
    let r = make_registry();
    let active = McpActivationSet::new(8);
    let eager: HashSet<String> = HashSet::new();
    let filter = WireFilter { lazy_mcp: false, active: &active, eager_servers: &eager };
    let result = r.to_caliban_tools_filtered(&filter);
    assert_eq!(result.tools.len(), 4);
    assert_eq!(result.dropped_mcp_count, 0);
}

#[test]
fn drops_inactive_mcp_when_lazy_mcp_true() {
    let r = make_registry();
    let active = McpActivationSet::new(8);
    let eager: HashSet<String> = HashSet::new();
    let filter = WireFilter { lazy_mcp: true, active: &active, eager_servers: &eager };
    let result = r.to_caliban_tools_filtered(&filter);
    let names: Vec<&str> = result.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, vec!["Read"]);
    assert_eq!(result.dropped_mcp_count, 3);
}

#[test]
fn passes_inactive_mcp_when_server_in_eager_list() {
    let r = make_registry();
    let active = McpActivationSet::new(8);
    let mut eager: HashSet<String> = HashSet::new();
    eager.insert("server_a".to_string());
    let filter = WireFilter { lazy_mcp: true, active: &active, eager_servers: &eager };
    let result = r.to_caliban_tools_filtered(&filter);
    let mut names: Vec<&str> = result.tools.iter().map(|t| t.name.as_str()).collect();
    names.sort();
    assert_eq!(names, vec!["Read", "mcp__server_a__one", "mcp__server_a__two"]);
    assert_eq!(result.dropped_mcp_count, 1); // only server_b dropped
}

#[test]
fn passes_active_mcp_regardless_of_server() {
    let r = make_registry();
    let mut active = McpActivationSet::new(8);
    active.activate("mcp__server_b__one");
    let eager: HashSet<String> = HashSet::new();
    let filter = WireFilter { lazy_mcp: true, active: &active, eager_servers: &eager };
    let result = r.to_caliban_tools_filtered(&filter);
    let mut names: Vec<&str> = result.tools.iter().map(|t| t.name.as_str()).collect();
    names.sort();
    assert_eq!(names, vec!["Read", "mcp__server_b__one"]);
    assert_eq!(result.dropped_mcp_count, 2);
}

#[test]
fn non_mcp_tools_always_pass() {
    let r = make_registry();
    let active = McpActivationSet::new(8);
    let eager: HashSet<String> = HashSet::new();
    let filter = WireFilter { lazy_mcp: true, active: &active, eager_servers: &eager };
    let result = r.to_caliban_tools_filtered(&filter);
    assert!(result.tools.iter().any(|t| t.name == "Read"));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p caliban-agent-core --test wire_filter`
Expected: FAIL — `wire_filter` module does not exist.

- [ ] **Step 3: Create the module**

Create `crates/caliban-agent-core/src/wire_filter.rs`:

```rust
//! Per-turn wire-payload filter for lazy MCP tool loading (ADR-0046).
//!
//! Filters [`ToolRegistry`]'s entries down to:
//! - all non-MCP tools, always; plus
//! - all MCP tools when `lazy_mcp == false`; otherwise
//! - MCP tools whose server segment is in `eager_servers`; plus
//! - MCP tools whose name is in the active set.

use std::collections::HashSet;

use crate::mcp_activation::McpActivationSet;

const MCP_PREFIX: &str = "mcp__";

/// Inputs to the per-turn filter. Borrowed; cheap to construct.
pub struct WireFilter<'a> {
    pub lazy_mcp: bool,
    pub active: &'a McpActivationSet,
    pub eager_servers: &'a HashSet<String>,
}

/// Output of [`ToolRegistry::to_caliban_tools_filtered`].
pub struct WireFilterResult {
    pub tools: Vec<caliban_provider::Tool>,
    pub dropped_mcp_count: usize,
}

/// Whether `name` follows the MCP `mcp__<server>__<tool>` convention.
#[must_use]
pub fn is_mcp(name: &str) -> bool {
    name.starts_with(MCP_PREFIX)
}

/// Extract the `<server>` segment from `mcp__<server>__<tool>`. Returns
/// `None` if `name` is not an MCP tool.
#[must_use]
pub fn mcp_server_of(name: &str) -> Option<&str> {
    let rest = name.strip_prefix(MCP_PREFIX)?;
    let end = rest.find("__")?;
    Some(&rest[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_mcp_recognises_prefix() {
        assert!(is_mcp("mcp__server__tool"));
        assert!(!is_mcp("Read"));
    }

    #[test]
    fn mcp_server_of_returns_segment() {
        assert_eq!(mcp_server_of("mcp__github__list_issues"), Some("github"));
        assert_eq!(mcp_server_of("mcp__bad-but-still-parses__t"), Some("bad-but-still-parses"));
        assert_eq!(mcp_server_of("Read"), None);
        assert_eq!(mcp_server_of("mcp__incomplete"), None);
    }
}
```

- [ ] **Step 4: Add `to_caliban_tools_filtered` on `ToolRegistry`**

In `crates/caliban-agent-core/src/registry.rs`, add immediately after `to_caliban_tools`:

```rust
    /// Variant of `to_caliban_tools` that applies a [`WireFilter`].
    /// Returns the filtered set plus the count of MCP tools that were
    /// dropped (used by the stream layer to splice a deferred-block
    /// paragraph into the system prompt).
    #[must_use]
    pub fn to_caliban_tools_filtered(
        &self,
        f: &crate::wire_filter::WireFilter<'_>,
    ) -> crate::wire_filter::WireFilterResult {
        let mut tools = Vec::with_capacity(self.tools.len());
        let mut dropped = 0_usize;

        for t in self.tools.values() {
            let name = t.name();
            if !crate::wire_filter::is_mcp(name) {
                tools.push(caliban_provider::Tool {
                    name: name.to_string(),
                    description: t.description().to_string(),
                    input_schema: t.input_schema().clone(),
                    cache_control: None,
                });
                continue;
            }
            if !f.lazy_mcp {
                tools.push(caliban_provider::Tool {
                    name: name.to_string(),
                    description: t.description().to_string(),
                    input_schema: t.input_schema().clone(),
                    cache_control: None,
                });
                continue;
            }
            let server_match = crate::wire_filter::mcp_server_of(name)
                .is_some_and(|s| f.eager_servers.contains(s));
            if server_match || f.active.is_active(name) {
                tools.push(caliban_provider::Tool {
                    name: name.to_string(),
                    description: t.description().to_string(),
                    input_schema: t.input_schema().clone(),
                    cache_control: None,
                });
                continue;
            }
            dropped += 1;
        }
        crate::wire_filter::WireFilterResult { tools, dropped_mcp_count: dropped }
    }
```

- [ ] **Step 5: Wire into lib.rs**

In `crates/caliban-agent-core/src/lib.rs`, add (near the existing module declarations):

```rust
pub mod wire_filter;
```

Also confirm `pub mod registry;` already exists — it does.

- [ ] **Step 6: Run tests**

Run: `cargo test -p caliban-agent-core --test wire_filter`
Expected: PASS (5 integration tests + 2 unit tests inside `wire_filter`).

- [ ] **Step 7: Commit**

```bash
git add crates/caliban-agent-core/src/wire_filter.rs \
        crates/caliban-agent-core/src/registry.rs \
        crates/caliban-agent-core/src/lib.rs \
        crates/caliban-agent-core/tests/wire_filter.rs
git commit -m "feat(agent-core): WireFilter + ToolRegistry::to_caliban_tools_filtered

Filters MCP tools at request-build time based on lazy_mcp setting,
per-server eager flags, and the active set. Returns dropped count so
the stream layer can splice the deferred-block paragraph. ADR-0046.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Phase 3 — ToolSearch built-in

### Task 7: Add `list_mcp_tools` accessor to `McpClientManager`

**Files:**
- Modify: `crates/caliban-mcp-client/src/manager.rs` (add `list_mcp_tools` method + `McpToolInfo` type)
- Test: `crates/caliban-mcp-client/tests/manager_list.rs` (or inline)

- [ ] **Step 1: Add the type**

In `crates/caliban-mcp-client/src/manager.rs` (top of file, near other public types):

```rust
/// Minimal descriptor of a registered MCP tool, used by ToolSearch.
#[derive(Debug, Clone)]
pub struct McpToolInfo {
    /// Full registry name, `mcp__<server>__<tool>`.
    pub full_name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}
```

- [ ] **Step 2: Add the accessor**

In `impl McpClientManager`, add:

```rust
    /// Snapshot the currently-registered MCP tools for use by ToolSearch.
    /// Includes only servers whose handshake succeeded (i.e., whatever
    /// `register_all` would surface to the registry).
    #[must_use]
    pub fn list_mcp_tools(&self) -> Vec<McpToolInfo> {
        // The manager already keeps `pending: Vec<Arc<McpTool>>` after
        // start(); we expose it as Info structs.
        self.pending
            .iter()
            .map(|t| McpToolInfo {
                full_name: t.full_name(),
                description: t.description().to_string(),
                input_schema: t.input_schema().clone(),
            })
            .collect()
    }
```

(If `self.pending` is private or doesn't exist by that name post-`register_all`, look for the internal map of registered MCP tools; the manager already iterates them during `register_all` at `caliban-mcp-client::manager:246`.)

- [ ] **Step 3: Write a test**

Add to existing manager tests or create `crates/caliban-mcp-client/tests/manager_list.rs`:

```rust
// Test that list_mcp_tools returns full_name in mcp__<server>__<tool> format.
// Implementation depends on existing test scaffolding for fake MCP servers.
// If no in-memory fake exists, this test can be #[ignore]'d for now and
// the integration test in Phase 3 Task 9 covers the round-trip.
#[test]
#[ignore = "depends on fake-MCP test scaffolding; covered by integration"]
fn list_mcp_tools_returns_prefixed_names() {
    // ...
}
```

- [ ] **Step 4: Commit**

```bash
git add crates/caliban-mcp-client/src/manager.rs crates/caliban-mcp-client/tests/
git commit -m "feat(mcp): McpClientManager::list_mcp_tools accessor

Returns a Vec<McpToolInfo> snapshot of registered MCP tools so
ToolSearch can enumerate without coupling to the agent's
ToolRegistry. ADR-0046.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 8: Create `ToolSearchTool` with query/select parsing + ranking

**Files:**
- Create: `crates/caliban-tools-builtin/src/tool_search.rs`
- Modify: `crates/caliban-tools-builtin/src/lib.rs` (export `ToolSearchTool`)
- Test: `crates/caliban-tools-builtin/tests/tool_search.rs`

- [ ] **Step 1: Write the failing test first**

Create `crates/caliban-tools-builtin/tests/tool_search.rs`:

```rust
use std::collections::HashSet;
use std::sync::Arc;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use caliban_agent_core::mcp_activation::McpActivationSet;
use caliban_agent_core::tool::{Tool, ToolContext};
use caliban_mcp_client::McpToolInfo;
use caliban_tools_builtin::tool_search::ToolSearchTool;
use serde_json::json;
use tokio_util::sync::CancellationToken;

fn make_info(name: &str, desc: &str) -> McpToolInfo {
    McpToolInfo {
        full_name: name.to_string(),
        description: desc.to_string(),
        input_schema: json!({"type":"object"}),
    }
}

fn make_search_tool(infos: Vec<McpToolInfo>) -> (ToolSearchTool, Arc<ArcSwap<McpActivationSet>>) {
    let active = Arc::new(ArcSwap::from_pointee(McpActivationSet::new(8)));
    let directory: Arc<dyn Fn() -> Vec<McpToolInfo> + Send + Sync> =
        Arc::new(move || infos.clone());
    let tool = ToolSearchTool::new(directory, Arc::clone(&active));
    (tool, active)
}

#[tokio::test]
async fn returns_no_matches_message_when_empty() {
    let (tool, _) = make_search_tool(vec![make_info("mcp__github__one", "anything")]);
    let cx = ToolContext {
        tool_use_id: "x".into(),
        cancel: CancellationToken::new(),
        hooks: None,
        turn_index: 0,
    };
    let blocks = tool.invoke(json!({"query":"completely-unrelated"}), cx).await.unwrap();
    let text = blocks.iter().filter_map(|b| match b {
        caliban_provider::ContentBlock::Text(t) => Some(t.text.as_str()),
        _ => None,
    }).next().unwrap();
    assert!(text.contains("No MCP tools matched"));
}

#[tokio::test]
async fn activates_matches_on_substring_query() {
    let (tool, active) = make_search_tool(vec![
        make_info("mcp__github__create_issue", "open a github issue"),
        make_info("mcp__github__list_issues", "list github issues"),
        make_info("mcp__postgres__query", "run a sql query"),
    ]);
    let cx = ToolContext {
        tool_use_id: "x".into(),
        cancel: CancellationToken::new(),
        hooks: None,
        turn_index: 0,
    };
    let _ = tool.invoke(json!({"query":"github"}), cx).await.unwrap();
    let snap = active.load();
    assert!(snap.is_active("mcp__github__create_issue"));
    assert!(snap.is_active("mcp__github__list_issues"));
    assert!(!snap.is_active("mcp__postgres__query"));
}

#[tokio::test]
async fn select_form_targets_exact_names() {
    let (tool, active) = make_search_tool(vec![
        make_info("mcp__a__one", ""),
        make_info("mcp__a__two", ""),
        make_info("mcp__b__one", ""),
    ]);
    let cx = ToolContext {
        tool_use_id: "x".into(),
        cancel: CancellationToken::new(),
        hooks: None,
        turn_index: 0,
    };
    let _ = tool.invoke(json!({"query":"select:mcp__a__one,mcp__b__one"}), cx).await.unwrap();
    let snap = active.load();
    assert!(snap.is_active("mcp__a__one"));
    assert!(!snap.is_active("mcp__a__two"));
    assert!(snap.is_active("mcp__b__one"));
}

#[tokio::test]
async fn respects_max_results() {
    let infos: Vec<McpToolInfo> = (0..20)
        .map(|i| make_info(&format!("mcp__a__t{i}"), "test"))
        .collect();
    let (tool, active) = make_search_tool(infos);
    let cx = ToolContext {
        tool_use_id: "x".into(),
        cancel: CancellationToken::new(),
        hooks: None,
        turn_index: 0,
    };
    let _ = tool.invoke(json!({"query":"mcp__a","max_results":3}), cx).await.unwrap();
    assert_eq!(active.load().len(), 3);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p caliban-tools-builtin --test tool_search`
Expected: FAIL — `tool_search` module does not exist.

- [ ] **Step 3: Create the module**

Create `crates/caliban-tools-builtin/src/tool_search.rs`:

```rust
//! `ToolSearch` built-in (ADR-0046). Lets the model discover MCP tools
//! by substring query or exact `select:` form. Activates matches in
//! the sidecar `McpActivationSet`; subsequent turns include them in
//! the wire payload via `WireFilter`.

use std::sync::Arc;
use std::sync::OnceLock;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use caliban_agent_core::mcp_activation::McpActivationSet;
use caliban_agent_core::tool::{Tool, ToolContext, ToolError};
use caliban_mcp_client::McpToolInfo;
use caliban_provider::ContentBlock;
use serde::Deserialize;
use serde_json::{json, Value};

const MAX_DEFAULT_RESULTS: usize = 10;
const MAX_RESULTS_CAP: usize = 25;

/// Resolves the current set of MCP tools at invoke time.
pub type DirectoryFn = Arc<dyn Fn() -> Vec<McpToolInfo> + Send + Sync>;

pub struct ToolSearchTool {
    directory: DirectoryFn,
    active: Arc<ArcSwap<McpActivationSet>>,
    schema: OnceLock<Value>,
}

impl std::fmt::Debug for ToolSearchTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolSearchTool").finish_non_exhaustive()
    }
}

impl ToolSearchTool {
    pub fn new(directory: DirectoryFn, active: Arc<ArcSwap<McpActivationSet>>) -> Self {
        Self {
            directory,
            active,
            schema: OnceLock::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct Input {
    query: String,
    #[serde(default)]
    max_results: Option<usize>,
}

#[async_trait]
impl Tool for ToolSearchTool {
    fn name(&self) -> &str {
        "ToolSearch"
    }

    fn description(&self) -> &str {
        "Search for MCP tools by name or description. Matching tools are \
         activated for the rest of this session — their full schemas appear \
         in your tool list on subsequent turns and you can call them directly. \
         Returns up to `max_results` matches with name, description, and JSON \
         Schema for each. Use `select:foo,bar` (comma-separated full names) to \
         fetch specific tools by exact name. When MCP loading is disabled this \
         tool returns a no-op message."
    }

    fn input_schema(&self) -> &Value {
        self.schema.get_or_init(|| json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Substring/word-prefix query. Use 'select:name1,name2' for exact names."
                },
                "max_results": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_RESULTS_CAP,
                    "default": MAX_DEFAULT_RESULTS
                }
            },
            "required": ["query"]
        }))
    }

    async fn invoke(
        &self,
        input: Value,
        _cx: ToolContext,
    ) -> Result<Vec<ContentBlock>, ToolError> {
        let parsed: Input =
            serde_json::from_value(input).map_err(|e| ToolError::invalid_input(e.to_string()))?;
        let max = parsed.max_results.unwrap_or(MAX_DEFAULT_RESULTS).min(MAX_RESULTS_CAP);
        let directory = (self.directory)();

        if directory.is_empty() {
            return Ok(vec![ContentBlock::Text(caliban_provider::TextBlock {
                text: "No MCP servers are configured.".to_string(),
            })]);
        }

        let matches = if let Some(rest) = parsed.query.strip_prefix("select:") {
            let wanted: Vec<&str> = rest.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
            let mut found = Vec::new();
            let mut missing = Vec::new();
            for name in &wanted {
                if let Some(info) = directory.iter().find(|i| i.full_name == *name) {
                    found.push(info.clone());
                } else {
                    missing.push((*name).to_string());
                }
            }
            (found, missing)
        } else {
            let q = parsed.query.to_lowercase();
            let mut ranked: Vec<(u32, &McpToolInfo)> = directory
                .iter()
                .filter_map(|i| {
                    let n = i.full_name.to_lowercase();
                    let d = i.description.to_lowercase();
                    let score = if n == q { 1000 }
                        else if n.contains(&q) { 800 }
                        else if d.contains(&q) { 400 }
                        else { 0 };
                    if score > 0 { Some((score, i)) } else { None }
                })
                .collect();
            ranked.sort_by(|a, b| b.0.cmp(&a.0));
            (
                ranked.into_iter().take(max).map(|(_, i)| i.clone()).collect(),
                Vec::new(),
            )
        };

        let (found, missing) = matches;

        if found.is_empty() {
            let mut msg = format!("No MCP tools matched '{}'.", parsed.query);
            if !missing.is_empty() {
                msg.push_str(&format!(" Unknown names: {}", missing.join(", ")));
            }
            return Ok(vec![ContentBlock::Text(caliban_provider::TextBlock { text: msg })]);
        }

        // Activate each match; collect evictions.
        let mut evictions = Vec::new();
        self.active.rcu(|s| {
            let mut new = (**s).clone();
            for info in &found {
                if let Some(evicted) = new.activate(&info.full_name) {
                    evictions.push(evicted);
                }
            }
            Arc::new(new)
        });

        // Format the response.
        let mut text = format!("Activated {} tool(s) for this session:\n\n", found.len());
        for info in &found {
            text.push_str(&format!(
                "{}\n  {}\n  Schema:\n  {}\n\n",
                info.full_name,
                info.description,
                serde_json::to_string(&info.input_schema).unwrap_or_default()
            ));
        }
        if !evictions.is_empty() {
            text.push_str(&format!(
                "Evicted {} to stay under cap:\n",
                evictions.len()
            ));
            for e in &evictions {
                text.push_str(&format!("  - {e} (least recently used)\n"));
            }
        }
        if !missing.is_empty() {
            text.push_str(&format!(
                "Unknown names ignored: {}\n",
                missing.join(", ")
            ));
        }

        Ok(vec![ContentBlock::Text(caliban_provider::TextBlock { text })])
    }
}
```

- [ ] **Step 4: Export from lib.rs**

In `crates/caliban-tools-builtin/src/lib.rs`, add:

```rust
pub mod tool_search;
```

(If the project re-exports specific tool types at the lib root, mirror the pattern.)

- [ ] **Step 5: Run tests**

Run: `cargo test -p caliban-tools-builtin --test tool_search`
Expected: PASS (4 tests).

- [ ] **Step 6: Commit**

```bash
git add crates/caliban-tools-builtin/src/tool_search.rs \
        crates/caliban-tools-builtin/src/lib.rs \
        crates/caliban-tools-builtin/tests/tool_search.rs
git commit -m "feat(tools): ToolSearch built-in for lazy MCP discovery

Substring/word-prefix query ranking + select:foo,bar exact form;
activates matches in the sidecar McpActivationSet; reports
evictions when LRU cap is hit; graceful no-op when no MCP servers
configured. ADR-0046.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 9: Register `ToolSearchTool` + wire eager-server set in binary startup

**Files:**
- Modify: `caliban/src/startup.rs` (insert ToolSearch registration; build eager-server set)
- Modify: `caliban/src/main.rs` (pass eager-server set to Agent builder)

- [ ] **Step 1: Build the eager-server set**

In `caliban/src/startup.rs::start_mcp` (or wherever MCP config is loaded — find it via grep `lazy.*server` or by following `mcp.toml` parsing), after configs are loaded, build:

```rust
let mut eager_servers = std::collections::HashSet::new();
for (name, cfg) in mcp_configs.iter() {
    if cfg.lazy == Some(false) {
        eager_servers.insert(name.clone());
    }
}
```

Return it from the function (extend the function signature to return both manager and eager set).

- [ ] **Step 2: Register `ToolSearchTool` after MCP startup**

In `caliban/src/main.rs`, find the lines around `start_mcp` (~line 271). The order is:
1. `build_registry()` — creates the registry with built-ins.
2. `start_mcp()` — registers MCP tools in the registry.
3. `install_sub_agent()` — installs `AgentTool` with a snapshot of the registry.

Between steps 2 and 3 (so ToolSearch sees MCP tools but sub-agents see ToolSearch in their snapshot), add:

```rust
{
    let mgr_for_search = Arc::clone(&mgr);
    let directory: caliban_tools_builtin::tool_search::DirectoryFn =
        Arc::new(move || mgr_for_search.list_mcp_tools());
    let active = Arc::clone(&agent.mcp_active()); // see Task 5 accessor
    registry.register(Arc::new(
        caliban_tools_builtin::tool_search::ToolSearchTool::new(directory, active),
    ));
}
```

The `agent` value here is the `Agent` that was built. If `Agent` is constructed AFTER the registry is finalized (typical pattern), then either:
- Build a placeholder `Arc<ArcSwap<McpActivationSet>>` first, share it with both ToolSearch and the Agent builder.
- Or restructure: construct the activation set first, pass to both.

The cleaner answer is: construct the activation set early. In `startup.rs::build_registry` (or in main.rs before `build_registry`):

```rust
let mcp_active = Arc::new(arc_swap::ArcSwap::from_pointee(
    caliban_agent_core::mcp_activation::McpActivationSet::new(
        settings.tools.as_ref().and_then(|t| t.max_active_schemas).unwrap_or(24)
    )
));
```

Pass `mcp_active` into both:
- `Agent` builder (via `.with_mcp_active(Arc::clone(&mcp_active))`).
- `ToolSearchTool::new(directory, Arc::clone(&mcp_active))`.

Add `with_mcp_active` and `with_mcp_eager_servers` setters on the `Agent` builder (in `agent.rs`).

- [ ] **Step 3: Wire `lazy_mcp` and `max_active_schemas` from settings into `AgentConfig`**

In `caliban/src/startup.rs::build_agent_config` (or wherever `AgentConfig` is assembled from `Settings` — look at how `auto_compact_threshold` flows for the same pattern), add:

```rust
config.lazy_mcp = settings.tools.as_ref().and_then(|t| t.lazy_mcp).unwrap_or(false);
config.max_active_schemas = settings.tools.as_ref().and_then(|t| t.max_active_schemas).unwrap_or(24);
```

- [ ] **Step 4: Build + run a smoke test**

Run: `cargo build --release` to confirm everything compiles.

Run: `cargo run --release -- --bare -p "say hello" --output-format json` and verify the binary starts and exits cleanly.

- [ ] **Step 5: Commit**

```bash
git add caliban/src/startup.rs caliban/src/main.rs crates/caliban-agent-core/src/agent.rs
git commit -m "feat(startup): register ToolSearch + wire lazy MCP eager-server set

ToolSearch is constructed after MCP manager startup so its directory
closure can enumerate the registered MCP tools. The shared
McpActivationSet is constructed early and passed into both the
Agent builder and ToolSearchTool. eager_servers is derived from
mcp.toml per-server lazy=false. ADR-0046.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Phase 4 — Stream integration

### Task 10: Wire `WireFilter` at the per-turn request build site

**Files:**
- Modify: `crates/caliban-agent-core/src/stream/mod.rs:497-523`

- [ ] **Step 1: Modify the request build**

In `crates/caliban-agent-core/src/stream/mod.rs` around line 497-523, replace:

```rust
let mut req_tools = self.tools.to_caliban_tools();
```

with:

```rust
let active_guard = self.mcp_active.load();
let filter = crate::wire_filter::WireFilter {
    lazy_mcp: self.config.lazy_mcp,
    active: &active_guard,
    eager_servers: &self.mcp_eager_servers,
};
let crate::wire_filter::WireFilterResult { tools: mut req_tools, dropped_mcp_count } =
    self.tools.to_caliban_tools_filtered(&filter);
```

- [ ] **Step 2: Make `dropped_mcp_count` available downstream**

Bind it so Step 3 / Task 11 can splice the system prompt:

```rust
let dropped_for_system_prompt = dropped_mcp_count;
let lazy_mcp_active_for_system_prompt = self.config.lazy_mcp;
```

- [ ] **Step 3: Run cargo check + existing tests**

Run: `cargo check -p caliban-agent-core && cargo test -p caliban-agent-core`
Expected: clean — default behavior is unchanged because `lazy_mcp` defaults to `false`.

- [ ] **Step 4: Commit**

```bash
git add crates/caliban-agent-core/src/stream/mod.rs
git commit -m "feat(stream): wire WireFilter at per-turn request build

When lazy_mcp=true, the wire payload is filtered to built-ins plus
active or eager-flagged MCP tools. Default is unchanged. ADR-0046.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 11: Splice deferred-block paragraph into the system message

**Files:**
- Create: `crates/caliban-agent-core/src/deferred_block.rs`
- Modify: `crates/caliban-agent-core/src/stream/mod.rs` (call the splice helper)
- Modify: `crates/caliban-agent-core/src/lib.rs` (`pub mod deferred_block;`)
- Test: `crates/caliban-agent-core/tests/deferred_block.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/caliban-agent-core/tests/deferred_block.rs`:

```rust
use caliban_agent_core::deferred_block::splice_into_messages;
use caliban_provider::{ContentBlock, Message, Role, TextBlock};

fn sys(text: &str) -> Message {
    Message {
        role: Role::System,
        content: vec![ContentBlock::Text(TextBlock { text: text.to_string() })],
    }
}

#[test]
fn appends_to_existing_system_message_when_dropped_gt_zero() {
    let mut msgs = vec![
        sys("you are an agent."),
        Message { role: Role::User, content: vec![] },
    ];
    splice_into_messages(&mut msgs, true, 5);
    let leading = match &msgs[0].content[0] {
        ContentBlock::Text(t) => &t.text,
        _ => panic!("expected text block"),
    };
    assert!(leading.contains("you are an agent."));
    assert!(leading.contains("Some MCP tools are deferred"));
    assert!(leading.contains("5 MCP tools"));
}

#[test]
fn noop_when_lazy_mcp_false() {
    let mut msgs = vec![sys("foo")];
    let before = msgs[0].clone();
    splice_into_messages(&mut msgs, false, 100);
    assert_eq!(msgs[0].content.len(), before.content.len());
    let leading = match &msgs[0].content[0] {
        ContentBlock::Text(t) => &t.text,
        _ => panic!(),
    };
    assert_eq!(leading, "foo");
}

#[test]
fn noop_when_dropped_zero() {
    let mut msgs = vec![sys("foo")];
    splice_into_messages(&mut msgs, true, 0);
    let leading = match &msgs[0].content[0] {
        ContentBlock::Text(t) => &t.text,
        _ => panic!(),
    };
    assert_eq!(leading, "foo");
}

#[test]
fn inserts_system_message_if_none_present() {
    let mut msgs = vec![Message { role: Role::User, content: vec![] }];
    splice_into_messages(&mut msgs, true, 3);
    assert_eq!(msgs[0].role, Role::System);
    let leading = match &msgs[0].content[0] {
        ContentBlock::Text(t) => &t.text,
        _ => panic!(),
    };
    assert!(leading.contains("3 MCP tools"));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p caliban-agent-core --test deferred_block`
Expected: FAIL — module does not exist.

- [ ] **Step 3: Create the helper**

Create `crates/caliban-agent-core/src/deferred_block.rs`:

```rust
//! Spliced paragraph that teaches the model about deferred MCP tools
//! when `tools.lazy_mcp` is on (ADR-0046).

use caliban_provider::{ContentBlock, Message, Role, TextBlock};

const DEFERRED_BLOCK_TEMPLATE: &str =
    "Some MCP tools are deferred to keep your tool palette lean. \
     Use the `ToolSearch` tool with a substring query to discover \
     and activate them when needed; activated tools persist for the \
     rest of the session. {N} MCP tools are currently deferred.";

fn format_block(dropped: usize) -> String {
    DEFERRED_BLOCK_TEMPLATE.replace("{N}", &dropped.to_string())
}

/// Splice the deferred-block paragraph into the leading system message
/// of `messages`. No-op when `lazy_mcp` is false or `dropped` is 0.
///
/// If no system message exists, insert one at index 0.
pub fn splice_into_messages(messages: &mut Vec<Message>, lazy_mcp: bool, dropped: usize) {
    if !lazy_mcp || dropped == 0 {
        return;
    }
    let block = format_block(dropped);

    if let Some(first) = messages.first_mut() {
        if matches!(first.role, Role::System) {
            if let Some(ContentBlock::Text(t)) = first.content.first_mut() {
                t.text.push_str("\n\n");
                t.text.push_str(&block);
            } else {
                first.content.insert(
                    0,
                    ContentBlock::Text(TextBlock { text: block.clone() }),
                );
            }
            return;
        }
    }
    messages.insert(0, Message {
        role: Role::System,
        content: vec![ContentBlock::Text(TextBlock { text: block })],
    });
}
```

- [ ] **Step 4: Wire into lib.rs**

In `crates/caliban-agent-core/src/lib.rs`, add:

```rust
pub mod deferred_block;
```

- [ ] **Step 5: Call from the stream**

In `crates/caliban-agent-core/src/stream/mod.rs`, immediately after `let mut req_messages = history.clone();` (line ~498) — i.e., before `req_tools` is built — add (after the `WireFilter` block from Task 10):

```rust
crate::deferred_block::splice_into_messages(
    &mut req_messages,
    self.config.lazy_mcp,
    dropped_mcp_count,
);
```

(`dropped_mcp_count` was bound in Task 10 already.)

- [ ] **Step 6: Run tests**

Run: `cargo test -p caliban-agent-core --test deferred_block`
Expected: PASS (4 tests).
Also: `cargo test -p caliban-agent-core` — full suite.

- [ ] **Step 7: Commit**

```bash
git add crates/caliban-agent-core/src/deferred_block.rs \
        crates/caliban-agent-core/src/lib.rs \
        crates/caliban-agent-core/src/stream/mod.rs \
        crates/caliban-agent-core/tests/deferred_block.rs
git commit -m "feat(stream): splice deferred-block paragraph when lazy MCP drops tools

Idempotent splice into the leading system message; inserts one if
absent. Counts the dropped MCP tools so the model sees how many are
hidden. No-op when lazy_mcp=false. ADR-0046.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Phase 5 — Sub-agent inheritance

### Task 12: Add `inherit_active_mcp` to `AgentToolInput`

**Files:**
- Modify: `crates/caliban-tools-builtin/src/agent/agent_tool.rs:52-81`

- [ ] **Step 1: Add the field**

In `AgentToolInput`, immediately after `inherit_hooks`:

```rust
    #[serde(default = "default_inherit_active_mcp")]
    pub inherit_active_mcp: bool,
```

At the bottom of the file, alongside `default_inherit_hooks`:

```rust
fn default_inherit_active_mcp() -> bool { true }
```

- [ ] **Step 2: Run cargo check**

Run: `cargo check -p caliban-tools-builtin`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/caliban-tools-builtin/src/agent/agent_tool.rs
git commit -m "feat(agent-tool): inherit_active_mcp frontmatter field (default true)

When true (default), sub-agents inherit the parent's
McpActivationSet. When false, they start with an empty set. ADR-0046.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 13: Snapshot parent's activation set in `install_sub_agent`

**Files:**
- Modify: `caliban/src/startup.rs:1152-1196` (install_sub_agent)
- Test: integration test (see Step 3)

- [ ] **Step 1: Extend the snapshot block**

In `install_sub_agent` (around `caliban/src/startup.rs:1161-1196`), the existing block snapshots the registry. After that snapshot:

```rust
// Capture the parent's activation state so child agents can inherit
// per frontmatter (ADR-0046).
let parent_mcp_active = parent_agent.mcp_active();
let parent_mcp_eager = parent_agent.mcp_eager_servers();
```

Then in the factory closure that builds a child `Agent`, take the frontmatter `inherit_active_mcp` flag (default true) and compute:

```rust
let child_active_set = if input.inherit_active_mcp {
    parent_mcp_active.load().snapshot()
} else {
    caliban_agent_core::mcp_activation::McpActivationSet::new(parent_mcp_cap)
};
let child_active = std::sync::Arc::new(
    arc_swap::ArcSwap::from_pointee(child_active_set),
);
```

And pass it into the child `Agent::builder()`:

```rust
.with_mcp_active(child_active)
.with_mcp_eager_servers(std::sync::Arc::clone(&parent_mcp_eager))
```

- [ ] **Step 2: Write an integration test**

Create `crates/caliban-tools-builtin/tests/agent_tool_inheritance.rs` (or extend the existing AgentTool integration test). Pseudocode:

```rust
// Build a parent Agent with a registry containing Read + mcp__a__one.
// Activate mcp__a__one in the parent's McpActivationSet.
// Invoke AgentTool with frontmatter inherit_active_mcp: true:
//   - child agent's mcp_active should is_active("mcp__a__one") == true.
// Invoke AgentTool with frontmatter inherit_active_mcp: false:
//   - child agent's mcp_active should is_active("mcp__a__one") == false.
```

The existing AgentTool tests show the scaffolding pattern for spawning an in-process child agent and observing its state. If the test infrastructure makes the child's `mcp_active` inaccessible, hook the assertion via the child's response (e.g. have the child immediately emit a tool_use that ToolSearch wouldn't activate if not already present).

If full integration coverage is awkward in v1, leave a `#[ignore]` placeholder and rely on the unit tests from Task 12 + manual smoke test in Task 16.

- [ ] **Step 3: Run tests + commit**

Run: `cargo test -p caliban-tools-builtin agent_tool_inheritance`
Expected: PASS.

```bash
git add caliban/src/startup.rs crates/caliban-tools-builtin/tests/agent_tool_inheritance.rs
git commit -m "feat(sub-agent): snapshot parent McpActivationSet on install per frontmatter

inherit_active_mcp: true (default) clones the parent's active set
into the child; false gives the child a fresh empty set. eager_servers
is always shared from parent (configuration, not state). ADR-0046.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Phase 6 — `/context` integration

### Task 14: Show MCP active line in `/context`

**Files:**
- Modify: `caliban/src/tui/slash/observe.rs:36-73` (ContextCommand::execute)

- [ ] **Step 1: Find access to the agent's `mcp_active`**

The slash command receives `ctx: &mut SlashCtx<'_>`. Look at `SlashCtx` for an `agent` accessor (or follow how other slash commands like `/cost` access agent state). The activation set is on `Agent`; the slash needs an Arc clone via `agent.mcp_active()`.

- [ ] **Step 2: Add the lines**

In `execute`, after the existing top-N blocks rendering:

```rust
if ctx.app.lazy_mcp_enabled() {
    let active = ctx.app.mcp_active().load();
    let cap = ctx.app.max_active_schemas();
    let line = format!("MCP active: {}/{}", active.len(), cap);
    ctx.app.transcript.push(TranscriptLine::Info(line));
    for name in active.iter_active() {
        ctx.app.transcript.push(TranscriptLine::Info(format!("  {name}")));
    }
}
```

You will likely need to add `lazy_mcp_enabled()`, `mcp_active()`, and `max_active_schemas()` accessors to whatever struct backs `ctx.app` (probably `caliban::tui::app::App` or similar) — wire them from the `Agent`/settings already present on `App`.

- [ ] **Step 3: Run tests + smoke test**

Run: `cargo test -p caliban` (binary tests).
Run: `cargo run --release -- --bare -p "/context" 2>&1 | head -40` — verify the MCP active line appears when lazy_mcp is on.

- [ ] **Step 4: Commit**

```bash
git add caliban/src/tui/slash/observe.rs caliban/src/tui/
git commit -m "feat(tui): /context shows MCP active set when lazy_mcp is on

Lists active count / cap plus each active name in MRU order. When
lazy_mcp=false the line is omitted (no information to surface).
ADR-0046.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Phase 7 — Docs + matrix updates

### Task 15: Update TODO + parity matrix

**Files:**
- Modify: `docs/TODO.md` (move Performance & scaling entry to closed)
- Modify: `docs/parity-gap-matrix.md` (F.ToolSearch + F.WaitForMcpServers → 🟡; refresh date line)
- Modify: `adrs/0046-two-stage-tool-surface.md` (status: proposed → accepted)

- [ ] **Step 1: TODO update**

In `docs/TODO.md`, the section "## Performance & scaling (2026-05-31)" — remove the bullet item itself (no longer outstanding) and add a one-line note under a new closing summary:

```markdown
## Performance & scaling (2026-05-31)

Closed: the two-stage tool surface design (ADR-0046, spec
`docs/superpowers/specs/2026-05-31-two-stage-tool-surface-design.md`)
landed in PR #<N>. Built-in tool laziness is not in scope yet — see
the spec's "Non-goals" section.
```

- [ ] **Step 2: Parity matrix update**

In `docs/parity-gap-matrix.md`, find rows:

```
| `ToolSearch` (lazy MCP schema loading) | 🔴 | only matters once MCP is real |
| `WaitForMcpServers` | 🔴 | same |
```

Replace with:

```
| `ToolSearch` (lazy MCP schema loading) | 🟡 | ADR-0046; v1 ships machinery (tools.lazy_mcp=true opt-in); v1.1 flips default to on |
| `WaitForMcpServers` | 🟡 | ADR-0046 covers the design space; standalone tool deferred to v1.1 |
```

Update the **Last refreshed** line at the top to add a new note about ADR-0046.

- [ ] **Step 3: ADR status flip**

In `adrs/0046-two-stage-tool-surface.md` change `**Status:** proposed` → `**Status:** accepted`. Update the ADR index in `adrs/README.md` so row 0046 says `accepted` instead of `proposed`.

- [ ] **Step 4: Commit**

```bash
git add docs/TODO.md docs/parity-gap-matrix.md adrs/0046-two-stage-tool-surface.md adrs/README.md
git commit -m "docs: close two-stage tool surface TODO; matrix F.ToolSearch \U0001f534→\U0001f7e1; ADR-0046 accepted

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

### Task 16: Final verification + release build

**Files:** (no source changes)

- [ ] **Step 1: Workspace tests**

Run: `cargo test --workspace`
Expected: all green.

- [ ] **Step 2: Clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 3: Release build**

Run: `cargo build --release`
Expected: builds; `target/release/caliban --version` reports successfully.

- [ ] **Step 4: Manual smoke (when an MCP server is available)**

In a directory with an `mcp.toml` configured for at least one server (e.g. silverbullet), with `settings.toml` containing:

```toml
[tools]
lazy_mcp = true
max_active_schemas = 6
```

Launch `cargo run --release` and:
1. Observe the TUI starts cleanly.
2. Run `/context` — confirm the "MCP active: 0/6" line is present.
3. Issue a prompt like "use ToolSearch to find tools for working with my notes."
4. Verify the model calls ToolSearch, the response shows activations, and the subsequent turn's tool list grows.
5. `/context` now shows "MCP active: N/6" with the activated names.

If anything is off, file a follow-up TODO entry instead of patching in this PR.

- [ ] **Step 5: Push the branch (do NOT open PR yet — user reviews first)**

```bash
git push -u origin strategic/two-stage-tool-surface
```

Tell the user: branch pushed; ready for PR review.

---

## Self-review checklist (for the human or executing agent)

- [ ] **Spec coverage:** every section in `docs/superpowers/specs/2026-05-31-two-stage-tool-surface-design.md` has a corresponding task above. Specifically:
  - Settings — Task 2
  - MCP per-server `lazy` — Task 3
  - `McpActivationSet` — Task 4
  - `Agent` field additions — Task 5
  - `WireFilter` — Task 6
  - `ToolSearch` mechanism — Tasks 7/8
  - Stream integration — Tasks 10/11
  - Sub-agent inheritance — Tasks 12/13
  - `/context` — Task 14
  - TODO / matrix / ADR housekeeping — Task 15
- [ ] **Type consistency:** `McpActivationSet::activate -> Option<String>` matches its usage in `ToolSearchTool::invoke` and tests; `WireFilter` field names match between definition (Task 6) and consumer (Task 10); `inherit_active_mcp: bool` matches in frontmatter struct (Task 12) and `install_sub_agent` (Task 13).
- [ ] **Placeholders fixed:** no "TBD" / "handle edge cases" / "add appropriate validation" lurking; the only `#[ignore]` is the optional integration test in Task 7 with a reasoned justification.
- [ ] **Default off:** all defaults preserve current behavior (Tasks 2, 5, 12). The TUI changes in Task 14 only render when `lazy_mcp=true`.

---

## Execution

Plan complete. Per sprint mode (consolidated proposal + spec + plan + impl in one pass), proceeding directly to inline execution via `superpowers:executing-plans`. Phases 1–7 in order; each task ends in a commit. Final task pushes the branch and yields to the user for review.
