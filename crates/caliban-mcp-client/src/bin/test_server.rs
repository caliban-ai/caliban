//! In-tree MCP test server used by `caliban-mcp-client` integration tests.
//!
//! Advertises four tools — `echo`, `fail`, `slow`, and `not_a_real_tool/with_slash`
//! — plus a `--hang-init` mode that never replies to `initialize`. Spawned by
//! `cargo test` via `CARGO_BIN_EXE_test_server`.

use std::sync::Arc;
use std::time::Duration;

use rmcp::ServiceExt;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, ListToolsResult, PaginatedRequestParams,
    ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::io::stdio;

#[derive(Debug, Clone, Default)]
struct TestServer;

impl ServerHandler for TestServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("caliban-mcp-client integration test server")
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        let object_schema = || {
            let mut obj = serde_json::Map::new();
            obj.insert(
                "type".to_string(),
                serde_json::Value::String("object".to_string()),
            );
            obj.insert(
                "additionalProperties".to_string(),
                serde_json::Value::Bool(true),
            );
            Arc::new(obj)
        };
        let tools = vec![
            Tool::new("echo", "Echo back the input as text", object_schema()),
            Tool::new(
                "fail",
                "Always returns isError=true with a fixed text message",
                object_schema(),
            ),
            Tool::new(
                "slow",
                "Sleeps for `ms` milliseconds (default 5000) before replying",
                object_schema(),
            ),
            // Intentionally contains a `/` so the client must normalize.
            Tool::new(
                "weird/name.tool",
                "Tool with chars that must be normalized by the client",
                object_schema(),
            ),
        ];
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        match request.name.as_ref() {
            "echo" => {
                let payload = request.arguments.map_or_else(
                    || "(no input)".to_string(),
                    |m| serde_json::Value::Object(m).to_string(),
                );
                Ok(CallToolResult::success(vec![Content::text(payload)]))
            }
            "fail" => Ok(CallToolResult::error(vec![Content::text(
                "tool fail: intentional failure",
            )])),
            "slow" => {
                let ms: u64 = request
                    .arguments
                    .as_ref()
                    .and_then(|m| m.get("ms"))
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(5_000);
                tokio::time::sleep(Duration::from_millis(ms)).await;
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "slept {ms}ms"
                ))]))
            }
            "weird/name.tool" => Ok(CallToolResult::success(vec![Content::text(
                "weird-tool ok",
            )])),
            _other => Err(rmcp::ErrorData::method_not_found::<
                rmcp::model::CallToolRequestMethod,
            >()),
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--hang-init") {
        // Read from stdin and produce nothing on stdout. This simulates a
        // server that never replies to `initialize` (HandshakeTimeout test).
        use tokio::io::{AsyncReadExt, BufReader};
        let mut stdin = BufReader::new(tokio::io::stdin());
        let mut buf = [0u8; 1024];
        loop {
            match stdin.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
        return Ok(());
    }

    // Optionally echo an env var on stderr so integration tests can assert
    // env-var pass-through. We never emit anything to stdout outside of
    // JSON-RPC framing — rmcp owns stdout.
    if let Ok(echo) = std::env::var("CALIBAN_TEST_ECHO_ENV") {
        eprintln!("CALIBAN_TEST_ECHO_ENV={echo}");
    }

    let (stdin, stdout) = stdio();
    let service = TestServer
        .serve((stdin, stdout))
        .await
        .map_err(|e| std::io::Error::other(format!("test_server: {e}")))?;
    let _quit = service.waiting().await;
    Ok(())
}
