//! Edit tool — replace occurrences of a string within a file.

use std::sync::Arc;
use std::sync::OnceLock;

use async_trait::async_trait;
use caliban_agent_core::{Tool, ToolContext, ToolError};
use caliban_provider::{ContentBlock, TextBlock};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::workspace::WorkspaceRoot;

/// File editor tool.
#[derive(Debug)]
pub struct EditTool {
    root: Arc<WorkspaceRoot>,
    schema: OnceLock<Value>,
}

impl EditTool {
    /// Construct an Edit tool using the given workspace root.
    #[must_use]
    pub fn new(root: WorkspaceRoot) -> Self {
        Self {
            root: Arc::new(root),
            schema: OnceLock::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct EditInput {
    path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &'static str {
        "Edit"
    }

    fn description(&self) -> &'static str {
        "Replace occurrences of old_string with new_string in a file. By default expects exactly one match; set replace_all=true to replace all occurrences."
    }

    fn input_schema(&self) -> &Value {
        self.schema.get_or_init(|| json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to edit (relative to workspace root or absolute)" },
                "old_string": { "type": "string", "description": "Exact text to search for in the file" },
                "new_string": { "type": "string", "description": "Text to replace old_string with" },
                "replace_all": { "type": "boolean", "description": "Replace all occurrences instead of requiring exactly one (default false)" }
            },
            "required": ["path", "old_string", "new_string"]
        }))
    }

    fn parallel_conflict_key(&self, input: &Value) -> Option<String> {
        input
            .get("path")
            .and_then(Value::as_str)
            .map(crate::parallel::canonical_key)
    }

    /// Invoke the Edit tool.
    ///
    /// Reads the file at `input["path"]`, counts occurrences of `old_string`,
    /// applies the replacement, and writes the result back.
    ///
    /// # Errors
    ///
    /// Returns [`ToolError::InvalidInput`] if the JSON input is malformed or
    /// the path is empty. Returns [`ToolError::Execution`] if the file cannot
    /// be read or written, if `old_string` is not found, or if `replace_all`
    /// is false and more than one occurrence is found.
    async fn invoke(&self, input: Value, cx: ToolContext) -> Result<Vec<ContentBlock>, ToolError> {
        let parsed: EditInput = crate::parse_input(input)?;

        let path = self.root.resolve(&parsed.path)?;

        let text = tokio::fs::read_to_string(&path)
            .await
            .map_err(ToolError::execution)?;

        let count = text.matches(&*parsed.old_string).count();

        if count == 0 {
            return Err(ToolError::execution(std::io::Error::other(
                "old_string not found in file",
            )));
        }

        if !parsed.replace_all && count > 1 {
            return Err(ToolError::execution(std::io::Error::other(format!(
                "old_string matched {count} times; expected exactly one (use replace_all=true to replace all)"
            ))));
        }

        let replaced = if parsed.replace_all {
            text.replace(&*parsed.old_string, &parsed.new_string)
        } else {
            text.replacen(&*parsed.old_string, &parsed.new_string, 1)
        };

        // Atomic, crash-safe write — shared via `caliban_common::fs::write_atomic`.
        caliban_common::fs::write_atomic(&path, replaced.as_bytes())
            .map_err(ToolError::execution)?;

        // Fire FileChanged on success (best-effort).
        cx.fire_file_changed(&path, caliban_agent_core::FileChangeKind::Modified, "Edit")
            .await;

        Ok(vec![ContentBlock::Text(TextBlock {
            text: format!(
                "→ Edited {} ({} replacement{})",
                self.root.relativize(&path).display(),
                count,
                if count == 1 { "" } else { "s" },
            ),
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
    async fn single_match_replaces_and_writes() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("file.txt");
        std::fs::write(&path, "hello foo world").unwrap();

        let tool = EditTool::new(WorkspaceRoot::new(tmp.path()));
        let out = tool
            .invoke(
                json!({"path": "file.txt", "old_string": "foo", "new_string": "bar"}),
                ctx(),
            )
            .await
            .unwrap();

        let ContentBlock::Text(t) = &out[0] else {
            panic!("expected Text block")
        };
        assert!(t.text.contains("Edited"), "output: {}", t.text);
        assert!(t.text.contains("1 replacement"), "output: {}", t.text);

        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written, "hello bar world");
    }

    #[tokio::test]
    async fn zero_match_errors() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("file.txt");
        std::fs::write(&path, "hello world").unwrap();

        let tool = EditTool::new(WorkspaceRoot::new(tmp.path()));
        let err = tool
            .invoke(
                json!({"path": "file.txt", "old_string": "foo", "new_string": "bar"}),
                ctx(),
            )
            .await
            .unwrap_err();

        assert!(matches!(err, ToolError::Execution(_)));
        let msg = format!("{err}");
        assert!(msg.contains("not found"), "error message: {msg}");
    }

    #[tokio::test]
    async fn multiple_matches_without_replace_all_errors() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("file.txt");
        std::fs::write(&path, "foo and foo").unwrap();

        let tool = EditTool::new(WorkspaceRoot::new(tmp.path()));
        let err = tool
            .invoke(
                json!({"path": "file.txt", "old_string": "foo", "new_string": "bar"}),
                ctx(),
            )
            .await
            .unwrap_err();

        assert!(matches!(err, ToolError::Execution(_)));
        let msg = format!("{err}");
        assert!(msg.contains("2 times"), "error message: {msg}");
    }

    #[tokio::test]
    async fn replace_all_replaces_multiple() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("file.txt");
        std::fs::write(&path, "foo and foo").unwrap();

        let tool = EditTool::new(WorkspaceRoot::new(tmp.path()));
        let out = tool
            .invoke(
                json!({"path": "file.txt", "old_string": "foo", "new_string": "bar", "replace_all": true}),
                ctx(),
            )
            .await
            .unwrap();

        let ContentBlock::Text(t) = &out[0] else {
            panic!("expected Text block")
        };
        assert!(t.text.contains("2 replacements"), "output: {}", t.text);

        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written, "bar and bar");
    }
}
