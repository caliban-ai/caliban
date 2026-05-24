//! Read tool — read a file's text contents.

use std::sync::Arc;
use std::sync::OnceLock;

use std::fmt::Write as _;

use async_trait::async_trait;
use caliban_agent_core::{Tool, ToolContext, ToolError};
use caliban_provider::{ContentBlock, TextBlock};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::workspace::WorkspaceRoot;

const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
const DEFAULT_LIMIT: usize = 2000;

/// File reader tool.
#[derive(Debug)]
pub struct ReadTool {
    root: Arc<WorkspaceRoot>,
    schema: OnceLock<Value>,
}

impl ReadTool {
    /// Construct a Read tool using the given workspace root.
    #[must_use]
    pub fn new(root: WorkspaceRoot) -> Self {
        Self {
            root: Arc::new(root),
            schema: OnceLock::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ReadInput {
    path: String,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    offset: Option<usize>,
}

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &'static str {
        "Read"
    }

    fn description(&self) -> &'static str {
        "Read a UTF-8 text file. Returns the file's contents prefixed with a header line. Use offset+limit to read large files in chunks."
    }

    fn input_schema(&self) -> &Value {
        self.schema.get_or_init(|| json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to read (relative to workspace root or absolute)" },
                "limit": { "type": "integer", "description": "Maximum number of lines to return (default 2000)", "minimum": 1 },
                "offset": { "type": "integer", "description": "1-indexed line to start at (default 1)", "minimum": 1 }
            },
            "required": ["path"]
        }))
    }

    /// Invoke the Read tool.
    ///
    /// Reads the file at `input["path"]`, optionally slicing by `offset` and
    /// `limit`. Returns one [`ContentBlock::Text`] with a header line followed
    /// by line-numbered content.
    ///
    /// # Errors
    ///
    /// Returns [`ToolError::InvalidInput`] if the JSON input is malformed or
    /// the path is empty. Returns [`ToolError::Execution`] if the file cannot
    /// be read or exceeds the 5 MB cap.
    async fn invoke(&self, input: Value, _cx: ToolContext) -> Result<Vec<ContentBlock>, ToolError> {
        let parsed: ReadInput = serde_json::from_value(input)
            .map_err(|e| ToolError::invalid_input(format!("invalid input: {e}")))?;

        let path = self.root.resolve(&parsed.path)?;
        let metadata = tokio::fs::metadata(&path)
            .await
            .map_err(ToolError::execution)?;
        if metadata.len() > MAX_FILE_BYTES {
            return Err(ToolError::execution(std::io::Error::other(format!(
                "file {} is {} bytes, larger than 5MB max; use offset+limit",
                path.display(),
                metadata.len(),
            ))));
        }

        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(ToolError::execution)?;

        let total = content.lines().count();
        let offset = parsed.offset.unwrap_or(1).saturating_sub(1);
        let limit = parsed.limit.unwrap_or(DEFAULT_LIMIT);
        let end = offset.saturating_add(limit).min(total);

        let chunk = content.lines().skip(offset).take(limit).enumerate().fold(
            String::new(),
            |mut s, (i, line)| {
                let _ = writeln!(s, "{:>5}  {}", offset + i + 1, line);
                s
            },
        );

        let header = format!(
            "→ Read {}, lines {}-{} of {}\n\n",
            self.root.relativize(&path).display(),
            offset + 1,
            end,
            total,
        );

        Ok(vec![ContentBlock::Text(TextBlock {
            text: format!("{header}{chunk}"),
            cache_control: None,
        })])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    fn ctx() -> ToolContext {
        ToolContext {
            tool_use_id: "t1".into(),
            cancel: CancellationToken::new(),
            hooks: None,
            turn_index: 0,
        }
    }

    #[tokio::test]
    async fn reads_existing_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("foo.txt");
        std::fs::write(&path, "hello\nworld\n").unwrap();
        let tool = ReadTool::new(WorkspaceRoot::new(tmp.path()));
        let out = tool
            .invoke(json!({"path": "foo.txt"}), ctx())
            .await
            .unwrap();
        let ContentBlock::Text(t) = &out[0] else {
            panic!()
        };
        assert!(t.text.contains("hello"));
        assert!(t.text.contains("world"));
    }

    #[tokio::test]
    async fn missing_file_errors() {
        let tmp = TempDir::new().unwrap();
        let tool = ReadTool::new(WorkspaceRoot::new(tmp.path()));
        let err = tool
            .invoke(json!({"path": "nope.txt"}), ctx())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Execution(_)));
    }

    #[tokio::test]
    async fn empty_file_succeeds() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("empty.txt");
        std::fs::write(&path, "").unwrap();
        let tool = ReadTool::new(WorkspaceRoot::new(tmp.path()));
        let out = tool
            .invoke(json!({"path": "empty.txt"}), ctx())
            .await
            .unwrap();
        let ContentBlock::Text(t) = &out[0] else {
            panic!()
        };
        assert!(t.text.contains("lines 1-0 of 0") || t.text.contains("0 of 0"));
    }

    #[tokio::test]
    async fn offset_and_limit() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("many.txt");
        std::fs::write(&path, "a\nb\nc\nd\ne\n").unwrap();
        let tool = ReadTool::new(WorkspaceRoot::new(tmp.path()));
        let out = tool
            .invoke(json!({"path": "many.txt", "offset": 2, "limit": 2}), ctx())
            .await
            .unwrap();
        let ContentBlock::Text(t) = &out[0] else {
            panic!()
        };
        assert!(t.text.contains('b'));
        assert!(t.text.contains('c'));
        // Line "d" (4th line) must not appear as a numbered body line.
        // The header contains "Read" which has a 'd', so we check the body lines
        // directly rather than the full text.
        let body_lines: Vec<&str> = t.text.lines().skip(2).collect(); // skip header + blank
        assert!(!body_lines.iter().any(|l| {
            l.trim_start_matches(|c: char| c.is_ascii_digit() || c == ' ')
                .starts_with('d')
        }));
    }
}
