//! caliban's MCP client consuming the gonzalo code-graph server (gonzalo EPIC
//! D / D2), via the same `McpClientManager` the agent uses.
//!
//! Two tests, deliberately split (see #344):
//!
//! * [`caliban_consumes_gonzalo_contract_via_mock`] — **hermetic, runs in CI**.
//!   Spawns the in-tree `gonzalo_test_server` fixture (a mock speaking the MCP
//!   protocol with gonzalo's documented tool/arg/result contract) and drives
//!   the real caliban client end to end. This pins the caliban→gonzalo contract
//!   — tool names, argument keys, and result shapes — so a caliban-side
//!   regression fails CI instead of shipping green.
//!
//! * [`caliban_client_queries_real_gonzalo_code_graph`] — **`#[ignore]`d**. The
//!   full round-trip against a *real* built `gonzalo-mcp` + a seeded store. It
//!   is honestly marked ignored (not a silent `return`) so it never masquerades
//!   as a passing CI check. Run it explicitly with the env set:
//!
//!   ```text
//!   # in the gonzalo repo:
//!   cargo build --release -p gonzalo-mcp -p gonzalo-cli
//!   gonzalo index crates/gonzalo-graph/src --root /tmp/gstore --repo gonzalo --view main
//!   GONZALO_MCP_BIN=.../target/release/gonzalo-mcp GONZALO_ROOT=/tmp/gstore \
//!     cargo test -p caliban-mcp-client --test gonzalo_integration -- --ignored --nocapture
//!   ```

#![allow(clippy::missing_panics_doc, clippy::pedantic)]

use std::collections::BTreeMap;

use caliban_agent_core::ToolContext;
use caliban_mcp_client::{
    ManualOauthConfig, McpClientManager, McpConfig, OauthMode, ServerConfig, ServerPermissions,
    TransportKind,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

/// The six code-graph tools gonzalo advertises, as caliban namespaces them.
const GONZALO_TOOLS: [&str; 6] = [
    "mcp__gonzalo__search",
    "mcp__gonzalo__node",
    "mcp__gonzalo__callers",
    "mcp__gonzalo__callees",
    "mcp__gonzalo__impact",
    "mcp__gonzalo__explore",
];

/// Build an `McpConfig` with a single stdio `gonzalo` server at `command`.
fn gonzalo_config(command: &str, root: &str) -> McpConfig {
    let mut env = BTreeMap::new();
    env.insert("GONZALO_ROOT".to_string(), root.to_string());
    let cfg = ServerConfig {
        transport: TransportKind::Stdio,
        command: command.to_string(),
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

/// Hermetic contract test — no real gonzalo, runs in CI.
///
/// Drives the real caliban client against the `gonzalo_test_server` fixture and
/// asserts the three things that can silently rot: the advertised tool names,
/// the argument keys caliban sends (`repo`/`view_id`/`name`), and the result
/// shapes caliban must be able to parse.
#[tokio::test]
async fn caliban_consumes_gonzalo_contract_via_mock() {
    let fixture = env!("CARGO_BIN_EXE_gonzalo_test_server");

    // 1. Caliban's manager spawns the fixture and discovers its tools.
    let mgr = McpClientManager::start(&gonzalo_config(fixture, "/unused"))
        .await
        .expect("gonzalo fixture manager start");
    assert_eq!(mgr.enabled_count(), 1, "gonzalo server should be enabled");
    assert_eq!(mgr.failed_count(), 0);

    let names: Vec<&str> = mgr.tool_names().collect();
    for tool in GONZALO_TOOLS {
        assert!(names.contains(&tool), "missing {tool} in {names:?}");
    }

    let mut registry = caliban_agent_core::ToolRegistry::new();
    mgr.register_all(&mut registry);

    // 2. `search` with the documented args round-trips, and the fixture echoes
    //    back the arg keys it actually received — so a caliban-side rename of
    //    repo/view_id/name would fail here.
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
        "expected at least one definition, got: {defs}"
    );
    assert_eq!(defs[0]["item"]["name"], "build_rust");
    let received = &defs[0]["received_args"];
    for key in ["repo", "view_id", "name"] {
        assert!(
            received
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v == key)),
            "fixture did not receive documented arg `{key}`: {received}"
        );
    }

    // 3. `impact` returns a JSON array over the (mocked) call graph.
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

/// The gonzalo-mcp binary + a seeded store, or `None` if the env is not set up.
fn gonzalo_env() -> Option<(String, String)> {
    let bin = std::env::var("GONZALO_MCP_BIN").ok()?;
    let root = std::env::var("GONZALO_ROOT").ok()?;
    if !std::path::Path::new(&bin).exists() {
        eprintln!("GONZALO_MCP_BIN does not exist: {bin}");
        return None;
    }
    Some((bin, root))
}

/// Full round-trip against a real built gonzalo-mcp + seeded store.
///
/// `#[ignore]`d on purpose: CI has no gonzalo build, so this would otherwise be
/// a silent no-op. The hermetic test above covers the contract in CI; this one
/// is the belt-and-suspenders live check you run by hand (see module docs).
#[tokio::test]
#[ignore = "requires a built gonzalo-mcp + seeded store (set GONZALO_MCP_BIN + GONZALO_ROOT); run with --ignored"]
async fn caliban_client_queries_real_gonzalo_code_graph() {
    let (bin, root) = gonzalo_env()
        .expect("set GONZALO_MCP_BIN + GONZALO_ROOT to run this ignored test (see module docs)");

    let mgr = McpClientManager::start(&gonzalo_config(&bin, &root))
        .await
        .expect("gonzalo-mcp manager start");
    assert_eq!(mgr.enabled_count(), 1, "gonzalo server should be enabled");
    assert_eq!(mgr.failed_count(), 0);

    let names: Vec<&str> = mgr.tool_names().collect();
    for tool in GONZALO_TOOLS {
        assert!(names.contains(&tool), "missing {tool} in {names:?}");
    }

    let mut registry = caliban_agent_core::ToolRegistry::new();
    mgr.register_all(&mut registry);

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
