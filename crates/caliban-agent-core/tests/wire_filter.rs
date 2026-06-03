//! Tests for [`caliban_agent_core::wire_filter`] + the filtered
//! `ToolRegistry::to_caliban_tools_filtered` method (ADR-0046).

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use caliban_agent_core::mcp_activation::McpActivationSet;
use caliban_agent_core::registry::ToolRegistry;
use caliban_agent_core::tool::{Tool, ToolContext, ToolError};
use caliban_agent_core::wire_filter::{WireFilter, is_mcp, mcp_server_of};
use caliban_provider::ContentBlock;

/// Minimal Tool impl for tests — name + a single stub schema.
struct StubTool {
    name: String,
    schema: serde_json::Value,
}

impl StubTool {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            schema: serde_json::json!({"type":"object"}),
        }
    }
}

#[async_trait]
impl Tool for StubTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &'static str {
        "stub"
    }
    fn input_schema(&self) -> &serde_json::Value {
        &self.schema
    }
    async fn invoke(
        &self,
        _input: serde_json::Value,
        _cx: ToolContext,
    ) -> Result<Vec<ContentBlock>, ToolError> {
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
    let filter = WireFilter {
        lazy_mcp: false,
        active: &active,
        eager_servers: &eager,
    };
    let result = r.to_caliban_tools_filtered(&filter);
    assert_eq!(result.tools.len(), 4);
    assert_eq!(result.dropped_mcp_count, 0);
}

#[test]
fn drops_inactive_mcp_when_lazy_mcp_true() {
    let r = make_registry();
    let active = McpActivationSet::new(8);
    let eager: HashSet<String> = HashSet::new();
    let filter = WireFilter {
        lazy_mcp: true,
        active: &active,
        eager_servers: &eager,
    };
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
    let filter = WireFilter {
        lazy_mcp: true,
        active: &active,
        eager_servers: &eager,
    };
    let result = r.to_caliban_tools_filtered(&filter);
    let mut names: Vec<String> = result.tools.iter().map(|t| t.name.clone()).collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "Read".to_string(),
            "mcp__server_a__one".to_string(),
            "mcp__server_a__two".to_string(),
        ]
    );
    assert_eq!(result.dropped_mcp_count, 1, "server_b dropped");
}

#[test]
fn passes_active_mcp_regardless_of_server() {
    let r = make_registry();
    let mut active = McpActivationSet::new(8);
    active.activate("mcp__server_b__one");
    let eager: HashSet<String> = HashSet::new();
    let filter = WireFilter {
        lazy_mcp: true,
        active: &active,
        eager_servers: &eager,
    };
    let result = r.to_caliban_tools_filtered(&filter);
    let mut names: Vec<String> = result.tools.iter().map(|t| t.name.clone()).collect();
    names.sort();
    assert_eq!(
        names,
        vec!["Read".to_string(), "mcp__server_b__one".to_string()]
    );
    assert_eq!(result.dropped_mcp_count, 2);
}

#[test]
fn non_mcp_tools_always_pass() {
    let r = make_registry();
    let active = McpActivationSet::new(8);
    let eager: HashSet<String> = HashSet::new();
    let filter = WireFilter {
        lazy_mcp: true,
        active: &active,
        eager_servers: &eager,
    };
    let result = r.to_caliban_tools_filtered(&filter);
    assert!(result.tools.iter().any(|t| t.name == "Read"));
}

#[test]
fn is_mcp_recognises_prefix() {
    assert!(is_mcp("mcp__server__tool"));
    assert!(!is_mcp("Read"));
    assert!(!is_mcp("Mcp__case"));
}

#[test]
fn mcp_server_of_extracts_segment() {
    assert_eq!(mcp_server_of("mcp__github__list_issues"), Some("github"));
    assert_eq!(mcp_server_of("mcp__a-b__t"), Some("a-b"));
    assert_eq!(mcp_server_of("Read"), None);
    assert_eq!(mcp_server_of("mcp__incomplete"), None);
}
