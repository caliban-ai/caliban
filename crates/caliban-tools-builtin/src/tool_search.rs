//! `ToolSearch` built-in tool (ADR-0046).
//!
//! Lets the model discover MCP tools by substring query or by exact
//! `select:` form. Matches are added to the sidecar
//! [`McpActivationSet`] and ride the wire payload on subsequent turns
//! via [`caliban_agent_core::wire_filter::WireFilter`].
//!
//! ## Input
//!
//! ```text
//! { "query": "github", "max_results": 10 }
//! { "query": "select:mcp__github__one,mcp__github__two" }
//! ```
//!
//! ## Output
//!
//! A single [`caliban_provider::TextBlock`] listing activated tools
//! (name + description + JSON Schema), any LRU evictions, and any
//! `select:` names that were not found.

use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::OnceLock;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use caliban_agent_core::mcp_activation::{McpActivationSet, McpToolInfo};
use caliban_agent_core::tool::{Tool, ToolContext, ToolError};
use caliban_provider::{ContentBlock, TextBlock};
use serde::Deserialize;
use serde_json::{Value, json};

const MAX_DEFAULT_RESULTS: usize = 10;
const MAX_RESULTS_CAP: usize = 25;

/// Closure that returns the current set of MCP tools at invoke time.
/// Resolved lazily so the wiring at startup can hand off a closure
/// that closes over an `Arc<McpClientManager>`.
pub type DirectoryFn = Arc<dyn Fn() -> Vec<McpToolInfo> + Send + Sync>;

/// `ToolSearch` discovery tool.
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
    /// Construct a new `ToolSearchTool`.
    ///
    /// The `directory` closure is invoked at every `invoke()` call so
    /// late-registered MCP servers (currently not a feature, but easy
    /// to support later) surface without restart.
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
    fn name(&self) -> &'static str {
        "ToolSearch"
    }

    fn description(&self) -> &'static str {
        "Search for MCP tools by name or description. Matching tools are \
         activated for the rest of this session — their full schemas appear \
         in your tool list on subsequent turns and you can call them directly. \
         Returns up to `max_results` matches with name, description, and JSON \
         Schema for each. Use `select:foo,bar` (comma-separated full names) to \
         fetch specific tools by exact name. When MCP loading is disabled this \
         tool returns a no-op message."
    }

    fn input_schema(&self) -> &Value {
        self.schema.get_or_init(|| {
            json!({
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
            })
        })
    }

    async fn invoke(&self, input: Value, _cx: ToolContext) -> Result<Vec<ContentBlock>, ToolError> {
        let parsed: Input = crate::parse_input(input)?;
        let max = parsed
            .max_results
            .unwrap_or(MAX_DEFAULT_RESULTS)
            .min(MAX_RESULTS_CAP);
        let directory = (self.directory)();

        if directory.is_empty() {
            return Ok(vec![ContentBlock::Text(TextBlock {
                text: "No MCP servers are configured.".to_string(),
                cache_control: None,
            })]);
        }

        // Either `select:foo,bar` (exact-name dispatch) or a ranked search.
        let (found, missing): (Vec<McpToolInfo>, Vec<String>) =
            if let Some(rest) = parsed.query.strip_prefix("select:") {
                let wanted: Vec<&str> = rest
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect();
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
                        let score = if n == q {
                            1000
                        } else if n.contains(&q) {
                            800
                        } else if d.contains(&q) {
                            400
                        } else {
                            0
                        };
                        if score > 0 { Some((score, i)) } else { None }
                    })
                    .collect();
                ranked.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
                (
                    ranked
                        .into_iter()
                        .take(max)
                        .map(|(_, i)| i.clone())
                        .collect(),
                    Vec::new(),
                )
            };

        if found.is_empty() {
            let mut msg = format!("No MCP tools matched '{}'.", parsed.query);
            if !missing.is_empty() {
                let _ = write!(msg, " Unknown names: {}", missing.join(", "));
            }
            return Ok(vec![ContentBlock::Text(TextBlock {
                text: msg,
                cache_control: None,
            })]);
        }

        // Activate each match; collect any LRU evictions for reporting.
        let mut evictions: Vec<String> = Vec::new();
        self.active.rcu(|s| {
            let mut new = (**s).clone();
            for info in &found {
                if let Some(evicted) = new.activate(&info.full_name) {
                    evictions.push(evicted);
                }
            }
            Arc::new(new)
        });

        Ok(vec![ContentBlock::Text(TextBlock {
            text: format_response(&found, &evictions, &missing),
            cache_control: None,
        })])
    }
}

fn format_response(found: &[McpToolInfo], evictions: &[String], missing: &[String]) -> String {
    let mut text = format!("Activated {} tool(s) for this session:\n\n", found.len());
    for info in found {
        let _ = writeln!(
            text,
            "{}\n  {}\n  Schema:\n  {}\n",
            info.full_name,
            info.description,
            serde_json::to_string(&info.input_schema).unwrap_or_default()
        );
    }
    if !evictions.is_empty() {
        let _ = writeln!(text, "Evicted {} to stay under cap:", evictions.len());
        for e in evictions {
            let _ = writeln!(text, "  - {e} (least recently used)");
        }
    }
    if !missing.is_empty() {
        let _ = writeln!(text, "Unknown names ignored: {}", missing.join(", "));
    }
    text
}
