//! `McpTool` — wraps one server-advertised tool as a caliban `Tool`.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use caliban_agent_core::{Tool, ToolContext, ToolError};
use caliban_provider::{ContentBlock, ImageBlock, ImageSource, TextBlock};
use rmcp::model::{CallToolRequestParams, CallToolResult, Content, RawContent};
use serde_json::Value;

use crate::client::Conn;

/// Normalize a server-advertised tool name to `[a-zA-Z0-9_-]` by replacing
/// anything else with `_`. Empty names become `_`.
#[must_use]
pub fn normalize_tool_name(raw: &str) -> String {
    if raw.is_empty() {
        return "_".to_string();
    }
    raw.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Build the registry name for a `(server, tool)` pair:
/// `mcp__<server>__<normalized_tool>`.
#[must_use]
pub fn full_tool_name(server: &str, raw_tool: &str) -> String {
    format!("mcp__{server}__{}", normalize_tool_name(raw_tool))
}

/// One MCP server-advertised tool, surfaced as a caliban `Tool`.
#[derive(Debug)]
pub struct McpTool {
    /// Server name (matches the `mcp.toml` table key).
    server_name: String,
    /// Original tool name as advertised by the server (passed through to
    /// `call_tool`).
    tool_name: String,
    /// Registry name: `mcp__<server>__<normalized_tool>`.
    full_name: String,
    /// Description for the provider's tool-use API.
    description: String,
    /// Tool input schema as advertised by the server.
    input_schema: Value,
    /// Shared connection to the owning server.
    conn: Arc<Conn>,
    /// Per-tool timeout. Inherits from server config; default 60s.
    timeout: Duration,
}

impl McpTool {
    /// Construct from an rmcp `Tool` plus the live connection.
    pub fn new(
        server: &str,
        conn: Arc<Conn>,
        advertised: &rmcp::model::Tool,
        timeout: Duration,
    ) -> Self {
        let tool_name = advertised.name.to_string();
        let full_name = full_tool_name(server, &tool_name);
        let description = advertised
            .description
            .as_ref()
            .map_or_else(String::new, std::string::ToString::to_string);
        // input_schema is Arc<JsonObject>; clone the object into a Value.
        let input_schema = Value::Object((*advertised.input_schema).clone());
        Self {
            server_name: server.to_string(),
            tool_name,
            full_name,
            description,
            input_schema,
            conn,
            timeout,
        }
    }

    /// Registry name (`mcp__<server>__<tool>`).
    #[must_use]
    pub fn full_name(&self) -> &str {
        &self.full_name
    }

    /// Server name as written in `mcp.toml`.
    #[must_use]
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Raw tool name as advertised by the server (pre-normalization).
    #[must_use]
    pub fn raw_tool_name(&self) -> &str {
        &self.tool_name
    }
}

/// Translate rmcp content into caliban `ContentBlock`s. Text + image map
/// directly; audio falls back to a text notice; embedded resources and resource
/// links are surfaced as text descriptions of their URI.
fn translate_content(blocks: Vec<Content>) -> Vec<ContentBlock> {
    let mut out: Vec<ContentBlock> = Vec::with_capacity(blocks.len());
    for block in blocks {
        // `Content` is `Annotated<RawContent>`; deref to the raw payload.
        let raw: RawContent = block.raw;
        match raw {
            RawContent::Text(t) => out.push(ContentBlock::Text(TextBlock {
                text: t.text,
                cache_control: None,
            })),
            RawContent::Image(i) => out.push(ContentBlock::Image(ImageBlock {
                source: ImageSource::Base64 {
                    media_type: i.mime_type,
                    data: i.data,
                },
                cache_control: None,
                sha256: None,
                dims: None,
            })),
            RawContent::Audio(a) => out.push(ContentBlock::Text(TextBlock {
                text: format!(
                    "[audio content not yet supported by caliban; {} bytes of base64 audio/{}]",
                    a.data.len(),
                    a.mime_type,
                ),
                cache_control: None,
            })),
            RawContent::Resource(r) => {
                let uri = match &r.resource {
                    rmcp::model::ResourceContents::TextResourceContents { uri, .. }
                    | rmcp::model::ResourceContents::BlobResourceContents { uri, .. } => {
                        uri.clone()
                    }
                };
                out.push(ContentBlock::Text(TextBlock {
                    text: format!("[embedded resource: {uri}]"),
                    cache_control: None,
                }));
            }
            RawContent::ResourceLink(link) => {
                out.push(ContentBlock::Text(TextBlock {
                    text: format!("[resource link: {} — {}]", link.name, link.uri),
                    cache_control: None,
                }));
            }
        }
    }
    out
}

/// Build the human-readable error text for an `isError: true` MCP result.
fn format_server_error(result: &CallToolResult) -> String {
    let mut s = String::new();
    for block in &result.content {
        if let RawContent::Text(t) = &block.raw {
            if !s.is_empty() {
                s.push('\n');
            }
            s.push_str(&t.text);
        }
    }
    if s.is_empty() {
        "tool reported an error (no text content)".to_string()
    } else {
        s
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.full_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> &Value {
        &self.input_schema
    }

    async fn invoke(&self, input: Value, cx: ToolContext) -> Result<Vec<ContentBlock>, ToolError> {
        // MCP arguments must be a JSON object (or omitted). Coerce: object → Some,
        // null → None, anything else is an input bug we surface.
        let arguments = match input {
            Value::Object(obj) => Some(obj),
            Value::Null => None,
            other => {
                return Err(ToolError::invalid_input(format!(
                    "MCP tools require object input, got {other:?}",
                )));
            }
        };

        let mut params = CallToolRequestParams::new(self.tool_name.clone());
        if let Some(args) = arguments {
            params = params.with_arguments(args);
        }

        let peer = self.conn.peer();
        let timeout = self.timeout;
        let call_fut = peer.call_tool(params);
        let timed = tokio::time::timeout(timeout, call_fut);

        let result: CallToolResult = tokio::select! {
            biased;
            () = cx.cancel.cancelled() => {
                return Err(ToolError::Cancelled);
            }
            outcome = timed => match outcome {
                Ok(Ok(r)) => r,
                Ok(Err(rpc)) => {
                    return Err(ToolError::execution(std::io::Error::other(format!(
                        "mcp server '{}' rpc error: {}",
                        self.server_name, rpc,
                    ))));
                }
                Err(_elapsed) => {
                    return Err(ToolError::execution(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!("mcp tool '{}' timed out after {:?}", self.full_name, timeout),
                    )));
                }
            },
        };

        if result.is_error.unwrap_or(false) {
            return Err(ToolError::execution(std::io::Error::other(
                format_server_error(&result),
            )));
        }

        Ok(translate_content(result.content))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_replaces_disallowed_chars() {
        assert_eq!(normalize_tool_name("simple"), "simple");
        assert_eq!(
            normalize_tool_name("with-dash_and_underscore"),
            "with-dash_and_underscore"
        );
        assert_eq!(normalize_tool_name("path/with/slash"), "path_with_slash");
        assert_eq!(normalize_tool_name("dotted.name"), "dotted_name");
        assert_eq!(normalize_tool_name("emoji😀tool"), "emoji_tool");
        assert_eq!(normalize_tool_name(""), "_");
    }

    #[test]
    fn full_name_format() {
        assert_eq!(
            full_tool_name("linear", "list_issues"),
            "mcp__linear__list_issues"
        );
        assert_eq!(full_tool_name("fs", "read/file"), "mcp__fs__read_file");
    }

    #[test]
    fn translate_text_block() {
        let blocks = vec![Content::text("hello")];
        let out = translate_content(blocks);
        assert_eq!(out.len(), 1);
        let ContentBlock::Text(t) = &out[0] else {
            panic!("expected text")
        };
        assert_eq!(t.text, "hello");
    }

    #[test]
    fn translate_image_block() {
        let blocks = vec![Content::image("BASE64DATA", "image/png")];
        let out = translate_content(blocks);
        assert_eq!(out.len(), 1);
        let ContentBlock::Image(i) = &out[0] else {
            panic!("expected image")
        };
        match &i.source {
            ImageSource::Base64 { media_type, data } => {
                assert_eq!(media_type, "image/png");
                assert_eq!(data, "BASE64DATA");
            }
            ImageSource::Url { .. } | ImageSource::BlobRef { .. } => {
                panic!("expected base64 image")
            }
        }
    }

    #[test]
    fn translate_audio_falls_back_to_text() {
        let raw = RawContent::Audio(rmcp::model::RawAudioContent {
            data: "AUDIODATA".to_string(),
            mime_type: "wav".to_string(),
        });
        let block = rmcp::model::Annotated {
            raw,
            annotations: None,
        };
        let out = translate_content(vec![block]);
        assert_eq!(out.len(), 1);
        let ContentBlock::Text(t) = &out[0] else {
            panic!("expected text fallback")
        };
        assert!(
            t.text.contains("audio content not yet supported"),
            "got: {}",
            t.text
        );
    }
}
