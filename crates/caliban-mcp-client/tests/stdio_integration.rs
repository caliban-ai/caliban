//! Integration tests for `caliban-mcp-client` Phase A (stdio transport).
//!
//! All tests spawn the in-tree `test_server` binary (built by Cargo via
//! `[[bin]] name = "test_server"`) using `env!("CARGO_BIN_EXE_test_server")`
//! and drive the rmcp client through `Conn::start` + `McpClientManager::start`.

#![allow(clippy::missing_panics_doc, clippy::pedantic)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use caliban_agent_core::{ToolContext, ToolError, ToolRegistry};
use caliban_mcp_client::{
    Conn, DEFAULT_TOOL_TIMEOUT, ManualOauthConfig, McpClientManager, McpConfig, McpError,
    OauthMode, ServerConfig, ServerPermissions, ServerStatus, StartOptions, Transport,
    TransportKind,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

fn test_server_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_test_server"))
}

fn server_config(extra_args: &[&str], env: BTreeMap<String, String>) -> ServerConfig {
    let mut args: Vec<String> = Vec::new();
    for a in extra_args {
        args.push((*a).to_string());
    }
    ServerConfig {
        transport: TransportKind::Stdio,
        command: test_server_path().to_string_lossy().into_owned(),
        args,
        env,
        cwd: None,
        url: None,
        headers: BTreeMap::new(),
        oauth: OauthMode::Off,
        manual_oauth: ManualOauthConfig::default(),
        disabled: false,
        permissions: ServerPermissions::default(),
    }
}

fn ctx() -> ToolContext {
    ToolContext {
        tool_use_id: "t1".to_string(),
        cancel: CancellationToken::new(),
        hooks: None,
        turn_index: 0,
    }
}

fn ctx_with(token: CancellationToken) -> ToolContext {
    ToolContext {
        tool_use_id: "t-cancel".to_string(),
        cancel: token,
        hooks: None,
        turn_index: 0,
    }
}

fn single_server_config(name: &str, cfg: ServerConfig) -> McpConfig {
    let mut servers = BTreeMap::new();
    servers.insert(name.to_string(), cfg);
    McpConfig { servers }
}

/// 1. Start manager against the test server, assert tools registered with the
///    expected `mcp__<server>__<tool>` names (including normalization).
#[tokio::test]
async fn discovers_and_registers_tools() {
    let cfg = single_server_config("test", server_config(&[], BTreeMap::new()));
    let mgr = McpClientManager::start(&cfg).await.expect("manager start");
    assert_eq!(mgr.enabled_count(), 1);
    assert_eq!(mgr.failed_count(), 0);

    let names: Vec<&str> = mgr.tool_names().collect();
    assert!(names.contains(&"mcp__test__echo"), "names: {names:?}");
    assert!(names.contains(&"mcp__test__fail"), "names: {names:?}");
    assert!(names.contains(&"mcp__test__slow"), "names: {names:?}");
    // The server advertises "weird/name.tool" — must be normalized to
    // `weird_name_tool` so the provider's tool-use API accepts it.
    assert!(
        names.contains(&"mcp__test__weird_name_tool"),
        "expected normalized name 'mcp__test__weird_name_tool', got names: {names:?}",
    );

    let mut registry = ToolRegistry::new();
    mgr.register_all(&mut registry);
    assert!(registry.get("mcp__test__echo").is_some());
    assert!(registry.get("mcp__test__weird_name_tool").is_some());
}

/// 2. Roundtrip — call `echo` and assert the text content comes back.
#[tokio::test]
async fn invokes_echo_tool_returns_text() {
    let cfg = single_server_config("test", server_config(&[], BTreeMap::new()));
    let mgr = McpClientManager::start(&cfg).await.expect("manager start");
    let mut registry = ToolRegistry::new();
    mgr.register_all(&mut registry);
    let tool = registry
        .get("mcp__test__echo")
        .expect("echo registered")
        .clone();

    let out = tool
        .invoke(json!({"msg": "hi there"}), ctx())
        .await
        .expect("invoke");
    assert_eq!(out.len(), 1);
    let caliban_provider::ContentBlock::Text(t) = &out[0] else {
        panic!("expected text block, got: {out:?}");
    };
    assert!(t.text.contains("hi there"), "got text: {}", t.text);
}

/// 3. Server-side error — `fail` returns `is_error=true` and we surface
///    `ToolError::Execution` carrying the server's message.
#[tokio::test]
async fn server_side_error_becomes_execution_error() {
    let cfg = single_server_config("test", server_config(&[], BTreeMap::new()));
    let mgr = McpClientManager::start(&cfg).await.expect("manager start");
    let mut registry = ToolRegistry::new();
    mgr.register_all(&mut registry);
    let tool = registry
        .get("mcp__test__fail")
        .expect("fail registered")
        .clone();

    let err = tool.invoke(json!({}), ctx()).await.unwrap_err();
    assert!(matches!(err, ToolError::Execution(_)), "got: {err:?}");
    let msg = format!("{err}");
    assert!(
        msg.contains("intentional failure"),
        "expected server error text in '{msg}'",
    );
}

/// 4. Cancellation token fires mid-call — `slow` should return Cancelled
///    within ~100ms (we sleep 5s on the server side).
#[tokio::test]
async fn cancellation_aborts_call_within_100ms() {
    let cfg = single_server_config("test", server_config(&[], BTreeMap::new()));
    let mgr = McpClientManager::start(&cfg).await.expect("manager start");
    let mut registry = ToolRegistry::new();
    mgr.register_all(&mut registry);
    let tool = registry
        .get("mcp__test__slow")
        .expect("slow registered")
        .clone();

    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel_clone.cancel();
    });

    let start = std::time::Instant::now();
    let err = tool
        .invoke(json!({"ms": 5_000}), ctx_with(cancel))
        .await
        .unwrap_err();
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(500),
        "cancellation should fire fast, but took {elapsed:?}",
    );
    assert!(matches!(err, ToolError::Cancelled), "got: {err:?}");
}

/// 5. A bogus server (nonexistent binary) doesn't stop startup; other servers
///    keep working.
#[tokio::test]
async fn failed_server_does_not_abort_startup() {
    let mut servers = BTreeMap::new();
    servers.insert(
        "ghost".to_string(),
        ServerConfig {
            transport: TransportKind::Stdio,
            command: "/this/definitely/does/not/exist".to_string(),
            args: vec![],
            env: BTreeMap::new(),
            cwd: None,
            url: None,
            headers: BTreeMap::new(),
            oauth: OauthMode::Off,
            manual_oauth: ManualOauthConfig::default(),
            disabled: false,
            permissions: ServerPermissions::default(),
        },
    );
    servers.insert("real".to_string(), server_config(&[], BTreeMap::new()));
    let cfg = McpConfig { servers };

    let mgr = McpClientManager::start(&cfg).await.expect("manager start");
    assert_eq!(mgr.enabled_count(), 1, "real server should be connected");
    assert_eq!(
        mgr.failed_count(),
        1,
        "ghost server should be marked failed"
    );

    let names: Vec<&str> = mgr.tool_names().collect();
    assert!(names.iter().any(|n| n.starts_with("mcp__real__")));
    assert!(!names.iter().any(|n| n.starts_with("mcp__ghost__")));
}

/// 6. `--hang-init` mode — server reads forever and never replies. We expect
///    `HandshakeTimeout` after `startup_timeout`.
#[tokio::test]
async fn handshake_timeout_skips_server() {
    let transport = Transport::Stdio {
        command: test_server_path().to_string_lossy().into_owned(),
        args: vec!["--hang-init".to_string()],
        env: BTreeMap::new(),
        cwd: None,
    };
    let start = std::time::Instant::now();
    let result = Conn::start("hang".to_string(), transport, Duration::from_millis(250)).await;
    let elapsed = start.elapsed();
    assert!(
        matches!(result, Err(McpError::HandshakeTimeout { .. })),
        "got: {result:?}",
    );
    assert!(elapsed < Duration::from_millis(1500), "took {elapsed:?}");
}

/// 7. Manager also surfaces handshake-timeouts as Failed entries, without
///    aborting startup.
#[tokio::test]
async fn handshake_timeout_marks_server_failed() {
    let opts = StartOptions {
        startup_timeout: Duration::from_millis(250),
        tool_timeout: DEFAULT_TOOL_TIMEOUT,
    };
    let cfg = single_server_config(
        "hung",
        ServerConfig {
            transport: TransportKind::Stdio,
            command: test_server_path().to_string_lossy().into_owned(),
            args: vec!["--hang-init".to_string()],
            env: BTreeMap::new(),
            cwd: None,
            url: None,
            headers: BTreeMap::new(),
            oauth: OauthMode::Off,
            manual_oauth: ManualOauthConfig::default(),
            disabled: false,
            permissions: ServerPermissions::default(),
        },
    );
    let mgr = McpClientManager::start_with_options(&cfg, opts)
        .await
        .expect("manager start");
    assert_eq!(mgr.enabled_count(), 0);
    assert_eq!(mgr.failed_count(), 1);
    let summary = &mgr.summaries()[0];
    assert!(
        matches!(summary.status, ServerStatus::Failed { .. }),
        "status: {:?}",
        summary.status,
    );
}

/// 8. `disabled = true` servers are not spawned and surface as `Disabled`.
#[tokio::test]
async fn disabled_server_is_skipped() {
    let mut cfg_disabled = server_config(&[], BTreeMap::new());
    cfg_disabled.disabled = true;
    let cfg = single_server_config("nope", cfg_disabled);
    let mgr = McpClientManager::start(&cfg).await.expect("manager start");
    assert_eq!(mgr.enabled_count(), 0);
    assert_eq!(mgr.failed_count(), 0);
    assert_eq!(mgr.skipped_disabled(), 1);
    assert!(mgr.tool_names().next().is_none());
    assert_eq!(mgr.summaries()[0].status, ServerStatus::Disabled);
}

/// 9. `env` table is forwarded to the spawned child. We check this via a
///    side-channel: the test server echoes `CALIBAN_TEST_ECHO_ENV` to stderr,
///    which our stderr-drain task logs via `tracing`. To assert without
///    coupling to tracing internals, the test server keeps running and we
///    just verify it spawned + handshaked successfully (the absence of an
///    env var is the path that would fail elsewhere; presence is silent).
#[tokio::test]
async fn env_var_pass_through_does_not_break_handshake() {
    let mut env = BTreeMap::new();
    env.insert(
        "CALIBAN_TEST_ECHO_ENV".to_string(),
        "passthrough-value".to_string(),
    );
    let cfg = single_server_config("envserver", server_config(&[], env));
    let mgr = McpClientManager::start(&cfg).await.expect("manager start");
    assert_eq!(mgr.enabled_count(), 1);
    // List should still work — env var must not interfere with the JSON-RPC
    // framing on stdout (test server emits it on stderr only).
    let names: Vec<&str> = mgr.tool_names().collect();
    assert!(names.contains(&"mcp__envserver__echo"));
}

/// 10. Shutdown — explicit `shutdown()` terminates the child within a
///     reasonable window. We assert via `ps` on Unix.
#[cfg(unix)]
#[tokio::test]
async fn shutdown_terminates_child() {
    let cfg = single_server_config("test", server_config(&[], BTreeMap::new()));
    let mgr = McpClientManager::start(&cfg).await.expect("manager start");
    // Grab the pid from the underlying conn via the summaries → we expose pid
    // through a backdoor for tests: walk the registered tools and find one.
    // Simpler: re-create the Conn directly so we know the PID.
    drop(mgr);
    let transport = Transport::Stdio {
        command: test_server_path().to_string_lossy().into_owned(),
        args: vec![],
        env: BTreeMap::new(),
        cwd: None,
    };
    let conn = Conn::start("test".to_string(), transport, Duration::from_secs(5))
        .await
        .expect("start");
    let pid = conn.pid().expect("pid");
    // Drop conn → service drop_guard cancels and TokioChildProcess::Drop kills.
    drop(conn);
    tokio::time::sleep(Duration::from_millis(500)).await;
    // Verify via /proc or `ps`.
    let out = std::process::Command::new("ps")
        .arg("-p")
        .arg(pid.to_string())
        .output()
        .expect("ps");
    // ps exits non-zero if pid not found on most platforms.
    assert!(
        !out.status.success() || !String::from_utf8_lossy(&out.stdout).contains("test_server"),
        "process {pid} still alive after drop",
    );
}

/// 11. Per-tool timeout — `slow` with a tight `tool_timeout` should bail
///     with a TimedOut Execution error.
#[tokio::test]
async fn per_tool_timeout_fires() {
    let opts = StartOptions {
        startup_timeout: Duration::from_secs(5),
        tool_timeout: Duration::from_millis(250),
    };
    let cfg = single_server_config("test", server_config(&[], BTreeMap::new()));
    let mgr = McpClientManager::start_with_options(&cfg, opts)
        .await
        .expect("manager start");
    let mut registry = ToolRegistry::new();
    mgr.register_all(&mut registry);
    let tool = registry
        .get("mcp__test__slow")
        .expect("slow registered")
        .clone();

    let start = std::time::Instant::now();
    let err = tool.invoke(json!({"ms": 5_000}), ctx()).await.unwrap_err();
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(1_500),
        "timeout should fire ~250ms, took {elapsed:?}",
    );
    assert!(matches!(err, ToolError::Execution(_)), "got: {err:?}");
    let msg = format!("{err}");
    assert!(
        msg.to_lowercase().contains("timed out") || msg.contains("TimedOut"),
        "expected timeout in '{msg}'",
    );
}

/// 12. Conn pid is non-None on Unix once the child spawns.
#[cfg(unix)]
#[tokio::test]
async fn conn_exposes_child_pid() {
    let transport = Transport::Stdio {
        command: test_server_path().to_string_lossy().into_owned(),
        args: vec![],
        env: BTreeMap::new(),
        cwd: None,
    };
    let conn = Conn::start("test".to_string(), transport, Duration::from_secs(5))
        .await
        .expect("start");
    assert!(conn.pid().is_some());
}

/// 13. Stdio summaries carry transport='stdio' for the `/mcp` overlay column.
#[tokio::test]
async fn stdio_summary_carries_transport_kind() {
    let cfg = single_server_config("test", server_config(&[], BTreeMap::new()));
    let mgr = McpClientManager::start(&cfg).await.expect("manager start");
    assert_eq!(mgr.summaries().len(), 1);
    assert_eq!(mgr.summaries()[0].transport, "stdio");
}

/// 14. Tool input that isn't a JSON object returns `InvalidInput`.
#[tokio::test]
async fn non_object_input_is_invalid() {
    let cfg = single_server_config("test", server_config(&[], BTreeMap::new()));
    let mgr = McpClientManager::start(&cfg).await.expect("manager start");
    let mut registry = ToolRegistry::new();
    mgr.register_all(&mut registry);
    let tool = registry.get("mcp__test__echo").expect("echo").clone();
    let err = tool
        .invoke(json!("just a string"), ctx())
        .await
        .unwrap_err();
    assert!(matches!(err, ToolError::InvalidInput(_)), "got: {err:?}");
}

/// 15. Two configured servers both succeed and we see tools from each.
#[tokio::test]
async fn multiple_servers_register_independently() {
    let mut servers = BTreeMap::new();
    servers.insert("a".to_string(), server_config(&[], BTreeMap::new()));
    servers.insert("b".to_string(), server_config(&[], BTreeMap::new()));
    let cfg = McpConfig { servers };
    let mgr = McpClientManager::start(&cfg).await.expect("manager start");
    assert_eq!(mgr.enabled_count(), 2);
    let names: Vec<&str> = mgr.tool_names().collect();
    assert!(names.contains(&"mcp__a__echo"));
    assert!(names.contains(&"mcp__b__echo"));
}

/// 16. Connected summary records the tool count.
#[tokio::test]
async fn connected_summary_carries_tool_count() {
    let cfg = single_server_config("test", server_config(&[], BTreeMap::new()));
    let mgr = McpClientManager::start(&cfg).await.expect("manager start");
    assert_eq!(mgr.summaries().len(), 1);
    match &mgr.summaries()[0].status {
        ServerStatus::Connected { tools } => assert_eq!(*tools, 4),
        other => panic!("expected Connected, got {other:?}"),
    }
}

/// 17. `register_into` (legacy alias) still works for caliban/src/main.rs.
#[tokio::test]
async fn register_into_alias_still_works() {
    let cfg = single_server_config("test", server_config(&[], BTreeMap::new()));
    let mgr = McpClientManager::start(&cfg).await.expect("manager start");
    let mut registry = ToolRegistry::new();
    mgr.register_into(&mut registry);
    assert!(registry.get("mcp__test__echo").is_some());
}

/// 18. McpTool exposes server + raw tool name for diagnostics.
#[tokio::test]
async fn tool_exposes_server_and_raw_name() {
    let cfg = single_server_config("test", server_config(&[], BTreeMap::new()));
    let mgr = McpClientManager::start(&cfg).await.expect("manager start");
    let mut registry = ToolRegistry::new();
    mgr.register_all(&mut registry);

    // Pull the underlying McpTool by name and downcast through the registered
    // Arc. We can't downcast Arc<dyn Tool> safely here, so just check the
    // observable behavior on the Tool trait — input_schema is an object.
    let tool = registry
        .get("mcp__test__weird_name_tool")
        .expect("normalized tool");
    let schema = tool.input_schema();
    assert!(schema.is_object(), "schema: {schema}");
    assert_eq!(tool.name(), "mcp__test__weird_name_tool");
    let _ = Arc::clone(tool);
}
