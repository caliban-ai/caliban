//! Tests for `ToolSearchTool` (ADR-0046).

use std::sync::Arc;

use arc_swap::ArcSwap;
use caliban_agent_core::mcp_activation::{McpActivationSet, McpToolInfo};
use caliban_agent_core::tool::{Tool, ToolContext};
use caliban_provider::ContentBlock;
use caliban_tools_builtin::tool_search::{DirectoryFn, ToolSearchTool};
use serde_json::json;
use tokio_util::sync::CancellationToken;

fn make_info(name: &str, desc: &str) -> McpToolInfo {
    McpToolInfo {
        full_name: name.to_string(),
        description: desc.to_string(),
        input_schema: json!({"type":"object"}),
    }
}

fn make_search_tool(
    infos: Vec<McpToolInfo>,
    cap: usize,
) -> (ToolSearchTool, Arc<ArcSwap<McpActivationSet>>) {
    let active = Arc::new(ArcSwap::from_pointee(McpActivationSet::new(cap)));
    let directory: DirectoryFn = Arc::new(move || infos.clone());
    let tool = ToolSearchTool::new(directory, Arc::clone(&active));
    (tool, active)
}

fn cx() -> ToolContext {
    ToolContext {
        tool_use_id: "t".to_string(),
        cancel: CancellationToken::new(),
        hooks: None,
        turn_index: 0,
    }
}

fn text_block(blocks: &[ContentBlock]) -> &str {
    for b in blocks {
        if let ContentBlock::Text(t) = b {
            return t.text.as_str();
        }
    }
    panic!("expected a TextBlock in response");
}

#[tokio::test]
async fn returns_no_matches_message_when_empty() {
    let (tool, _) = make_search_tool(vec![make_info("mcp__github__one", "anything")], 8);
    let blocks = tool
        .invoke(json!({"query":"completely-unrelated"}), cx())
        .await
        .unwrap();
    assert!(text_block(&blocks).contains("No MCP tools matched"));
}

#[tokio::test]
async fn activates_matches_on_substring_query() {
    let (tool, active) = make_search_tool(
        vec![
            make_info("mcp__github__create_issue", "open a github issue"),
            make_info("mcp__github__list_issues", "list github issues"),
            make_info("mcp__postgres__query", "run a sql query"),
        ],
        8,
    );
    let _ = tool.invoke(json!({"query":"github"}), cx()).await.unwrap();
    let snap = active.load();
    assert!(snap.is_active("mcp__github__create_issue"));
    assert!(snap.is_active("mcp__github__list_issues"));
    assert!(!snap.is_active("mcp__postgres__query"));
}

#[tokio::test]
async fn select_form_targets_exact_names() {
    let (tool, active) = make_search_tool(
        vec![
            make_info("mcp__a__one", ""),
            make_info("mcp__a__two", ""),
            make_info("mcp__b__one", ""),
        ],
        8,
    );
    let _ = tool
        .invoke(json!({"query":"select:mcp__a__one,mcp__b__one"}), cx())
        .await
        .unwrap();
    let snap = active.load();
    assert!(snap.is_active("mcp__a__one"));
    assert!(!snap.is_active("mcp__a__two"));
    assert!(snap.is_active("mcp__b__one"));
}

#[tokio::test]
async fn select_missing_names_reported() {
    let (tool, _active) = make_search_tool(vec![make_info("mcp__a__one", "")], 8);
    let blocks = tool
        .invoke(json!({"query":"select:mcp__a__one,mcp__missing__x"}), cx())
        .await
        .unwrap();
    assert!(text_block(&blocks).contains("Unknown names"));
    assert!(text_block(&blocks).contains("mcp__missing__x"));
}

#[tokio::test]
async fn respects_max_results() {
    let infos: Vec<McpToolInfo> = (0..20)
        .map(|i| make_info(&format!("mcp__a__t{i}"), "test"))
        .collect();
    let (tool, active) = make_search_tool(infos, 32);
    let _ = tool
        .invoke(json!({"query":"mcp__a","max_results":3}), cx())
        .await
        .unwrap();
    assert_eq!(active.load().len(), 3);
}

#[tokio::test]
async fn reports_evictions_when_cap_exceeded() {
    // cap=2, activate three distinct → first evicted.
    let (tool, active) = make_search_tool(
        vec![
            make_info("mcp__a__one", ""),
            make_info("mcp__a__two", ""),
            make_info("mcp__a__three", ""),
        ],
        2,
    );
    let blocks = tool.invoke(json!({"query":"mcp__a"}), cx()).await.unwrap();
    let body = text_block(&blocks);
    // Eviction text should mention the evicted name.
    assert!(
        body.contains("Evicted") || body.contains("evicted"),
        "body: {body}"
    );
    assert_eq!(active.load().len(), 2);
}

#[tokio::test]
async fn empty_directory_returns_no_servers_message() {
    let (tool, _) = make_search_tool(vec![], 8);
    let blocks = tool.invoke(json!({"query":"any"}), cx()).await.unwrap();
    assert!(text_block(&blocks).contains("No MCP servers"));
}
