//! In-tree mock of the **gonzalo code-graph MCP server**, used by the hermetic
//! contract test in `tests/gonzalo_integration.rs` (#344). It speaks the MCP
//! protocol over stdio and reproduces gonzalo's documented *contract* — the six
//! code-graph tool names, their required arguments (`repo`/`view_id`/`name`),
//! and the JSON result shapes caliban parses — with canned data instead of a
//! real indexed store.
//!
//! It is intentionally strict: `search`/`impact` return an error result if any
//! documented argument is missing, so a caliban-side change to the argument
//! contract fails the test rather than silently passing. Spawned by `cargo
//! test` via `CARGO_BIN_EXE_gonzalo_test_server`.

use std::sync::Arc;

use rmcp::ServiceExt;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, ListToolsResult, PaginatedRequestParams,
    ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::io::stdio;
use serde_json::{Value, json};

/// The six code-graph tools gonzalo advertises (bare names; caliban namespaces
/// them to `mcp__gonzalo__<tool>`).
const TOOLS: [(&str, &str); 6] = [
    ("search", "Find definitions of a symbol by name in a view"),
    ("node", "Fetch a single graph node by id"),
    ("callers", "List callers of a symbol"),
    ("callees", "List callees of a symbol"),
    ("impact", "Names transitively impacted by a symbol"),
    ("explore", "Explore the neighborhood of a symbol"),
];

/// The arguments every code-graph query documents.
const REQUIRED_ARGS: [&str; 3] = ["repo", "view_id", "name"];

#[derive(Debug, Clone, Default)]
struct GonzaloFixture;

/// A permissive `{ "type": "object" }` input schema.
fn object_schema() -> Arc<serde_json::Map<String, Value>> {
    let mut obj = serde_json::Map::new();
    obj.insert("type".to_string(), Value::String("object".to_string()));
    obj.insert("additionalProperties".to_string(), Value::Bool(true));
    Arc::new(obj)
}

/// Sorted list of argument keys actually received, so the test can assert the
/// documented names arrived intact.
fn received_arg_keys(args: &serde_json::Map<String, Value>) -> Vec<String> {
    let mut keys: Vec<String> = args.keys().cloned().collect();
    keys.sort();
    keys
}

/// Check the three documented args are present and non-empty strings; return the
/// missing one's name otherwise.
fn missing_required(args: &serde_json::Map<String, Value>) -> Option<&'static str> {
    REQUIRED_ARGS.into_iter().find(|k| {
        args.get(*k)
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    })
}

impl ServerHandler for GonzaloFixture {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("gonzalo code-graph fixture (contract mock)")
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        let tools = TOOLS
            .into_iter()
            .map(|(name, desc)| Tool::new(name, desc, object_schema()))
            .collect();
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let args = request.arguments.unwrap_or_default();

        // Every code-graph tool requires the documented args.
        if let Some(missing) = missing_required(&args) {
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "missing or empty required argument `{missing}`"
            ))]));
        }
        let name = args["name"].as_str().unwrap_or_default();
        let repo = args["repo"].as_str().unwrap_or_default();
        let view_id = args["view_id"].as_str().unwrap_or_default();
        let received = received_arg_keys(&args);

        let body: Value = match request.name.as_ref() {
            // A definition list: one hit echoing the requested symbol + the arg
            // keys the fixture saw.
            "search" | "node" | "explore" => json!([{
                "item": { "name": name, "repo": repo, "view_id": view_id, "kind": "function" },
                "received_args": received,
            }]),
            // Name lists over the (mocked) call graph.
            "impact" | "callers" | "callees" => {
                json!([format!("{name}_edge_a"), format!("{name}_edge_b"),])
            }
            other => {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "unknown tool `{other}`"
                ))]));
            }
        };

        Ok(CallToolResult::success(vec![Content::text(
            body.to_string(),
        )]))
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let (stdin, stdout) = stdio();
    let service = GonzaloFixture
        .serve((stdin, stdout))
        .await
        .map_err(|e| std::io::Error::other(format!("gonzalo_test_server: {e}")))?;
    let _quit = service.waiting().await;
    Ok(())
}
