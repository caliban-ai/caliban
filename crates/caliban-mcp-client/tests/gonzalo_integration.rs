//! End-to-end: caliban's MCP client consuming the gonzalo code-graph server
//! (gonzalo EPIC D / D2). Uses the same `McpClientManager` the agent uses to
//! spawn the real `gonzalo-mcp` stdio binary and query an indexed view.
//!
//! The test is **env-gated** so CI (which has no gonzalo build) skips it. It
//! runs the full round-trip only when both `GONZALO_MCP_BIN` (path to the built
//! `gonzalo-mcp` binary) and `GONZALO_ROOT` (a store populated via
//! `gonzalo index`) are set. Reproduce locally:
//!
//! ```text
//! # in the gonzalo repo:
//! cargo build --release -p gonzalo-mcp -p gonzalo-cli
//! gonzalo index crates/gonzalo-graph/src --root /tmp/gstore --repo gonzalo --view main
//! GONZALO_MCP_BIN=.../target/release/gonzalo-mcp GONZALO_ROOT=/tmp/gstore \
//!   cargo test -p caliban-mcp-client --test gonzalo_integration -- --nocapture
//! ```

#![allow(clippy::missing_panics_doc, clippy::pedantic)]

use std::collections::BTreeMap;

use caliban_agent_core::ToolContext;
use caliban_mcp_client::{
    ManualOauthConfig, McpClientManager, McpConfig, OauthMode, ServerConfig, ServerPermissions,
    TransportKind,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

/// The gonzalo-mcp binary + a seeded store, or `None` if the env is not set up
/// (CI skips; a local run with the two env vars exercises the real path).
fn gonzalo_env() -> Option<(String, String)> {
    let bin = std::env::var("GONZALO_MCP_BIN").ok()?;
    let root = std::env::var("GONZALO_ROOT").ok()?;
    if !std::path::Path::new(&bin).exists() {
        eprintln!("skip: GONZALO_MCP_BIN does not exist: {bin}");
        return None;
    }
    Some((bin, root))
}

fn gonzalo_config(bin: &str, root: &str) -> McpConfig {
    let mut env = BTreeMap::new();
    env.insert("GONZALO_ROOT".to_string(), root.to_string());
    let cfg = ServerConfig {
        transport: TransportKind::Stdio,
        command: bin.to_string(),
        args: Vec::new(),
        env,
        cwd: None,
        url: None,
        headers: BTreeMap::new(),
        oauth: OauthMode::Off,
        manual_oauth: ManualOauthConfig::default(),
        disabled: false,
        lazy: None,
        permissions: ServerPermissions::default(),
    };
    let mut servers = BTreeMap::new();
    servers.insert("gonzalo".to_string(), cfg);
    McpConfig { servers }
}

fn ctx() -> ToolContext {
    ToolContext {
        tool_use_id: "gonzalo-e2e".to_string(),
        cancel: CancellationToken::new(),
        hooks: None,
        turn_index: 0,
    }
}

/// The text of a tool result's single content block.
fn text_of(blocks: &[caliban_provider::ContentBlock]) -> String {
    match blocks.first() {
        Some(caliban_provider::ContentBlock::Text(t)) => t.text.clone(),
        other => panic!("expected a text block, got: {other:?}"),
    }
}

#[tokio::test]
async fn caliban_client_queries_gonzalo_code_graph() {
    let Some((bin, root)) = gonzalo_env() else {
        eprintln!("skipping gonzalo e2e: set GONZALO_MCP_BIN and GONZALO_ROOT to run");
        return;
    };

    // 1. Caliban's manager spawns gonzalo-mcp and discovers its tools.
    let mgr = McpClientManager::start(&gonzalo_config(&bin, &root))
        .await
        .expect("gonzalo-mcp manager start");
    assert_eq!(mgr.enabled_count(), 1, "gonzalo server should be enabled");
    assert_eq!(mgr.failed_count(), 0);

    let names: Vec<&str> = mgr.tool_names().collect();
    for tool in [
        "mcp__gonzalo__search",
        "mcp__gonzalo__node",
        "mcp__gonzalo__callers",
        "mcp__gonzalo__callees",
        "mcp__gonzalo__impact",
        "mcp__gonzalo__explore",
    ] {
        assert!(names.contains(&tool), "missing {tool} in {names:?}");
    }

    let mut registry = caliban_agent_core::ToolRegistry::new();
    mgr.register_all(&mut registry);

    // 2. Invoke `search` for a symbol we know exists in the indexed view.
    let search = registry
        .get("mcp__gonzalo__search")
        .expect("search registered")
        .clone();
    let out = search
        .invoke(
            json!({ "repo": "gonzalo", "view_id": "main", "name": "build_rust" }),
            ctx(),
        )
        .await
        .expect("search invoke");
    let defs: serde_json::Value = serde_json::from_str(&text_of(&out)).expect("search json");
    assert!(
        defs.as_array().is_some_and(|a| !a.is_empty()),
        "expected at least one definition of build_rust, got: {defs}"
    );
    assert_eq!(defs[0]["item"]["name"], "build_rust");

    // 3. Invoke `impact` — a name-list result over the real call graph.
    let impact = registry
        .get("mcp__gonzalo__impact")
        .expect("impact registered")
        .clone();
    let out = impact
        .invoke(
            json!({ "repo": "gonzalo", "view_id": "main", "name": "build_rust" }),
            ctx(),
        )
        .await
        .expect("impact invoke");
    let names: serde_json::Value = serde_json::from_str(&text_of(&out)).expect("impact json");
    assert!(
        names.is_array(),
        "impact should return a JSON array: {names}"
    );
}
