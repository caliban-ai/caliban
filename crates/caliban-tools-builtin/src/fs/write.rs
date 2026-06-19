//! Write tool — write content to a file, creating parent directories as needed.

use std::sync::Arc;
use std::sync::OnceLock;

use async_trait::async_trait;
use caliban_agent_core::{Tool, ToolContext, ToolError};
use caliban_provider::{ContentBlock, TextBlock};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::workspace::WorkspaceRoot;

/// File writer tool.
#[derive(Debug)]
pub struct WriteTool {
    root: Arc<WorkspaceRoot>,
    schema: OnceLock<Value>,
}

impl WriteTool {
    /// Construct a Write tool using the given workspace root.
    #[must_use]
    pub fn new(root: WorkspaceRoot) -> Self {
        Self {
            root: Arc::new(root),
            schema: OnceLock::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct WriteInput {
    path: String,
    content: String,
}

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &'static str {
        "Write"
    }

    fn description(&self) -> &'static str {
        "Write content to a file. Creates the file (and any missing parent directories) if it does not exist; overwrites existing content."
    }

    fn input_schema(&self) -> &Value {
        self.schema.get_or_init(|| json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to write (relative to workspace root or absolute)" },
                "content": { "type": "string", "description": "Content to write to the file" }
            },
            "required": ["path", "content"]
        }))
    }

    fn parallel_conflict_key(&self, input: &Value) -> Option<String> {
        input
            .get("path")
            .and_then(Value::as_str)
            .map(crate::parallel::canonical_key)
    }

    /// Invoke the Write tool.
    ///
    /// Writes `input["content"]` to `input["path"]`, creating missing parent
    /// directories automatically.
    ///
    /// # Errors
    ///
    /// Returns [`ToolError::InvalidInput`] if the JSON input is malformed or
    /// the path is empty. Returns [`ToolError::Execution`] if the file cannot
    /// be written (e.g., permission denied).
    async fn invoke(&self, input: Value, cx: ToolContext) -> Result<Vec<ContentBlock>, ToolError> {
        let parsed: WriteInput = crate::parse_input(input)?;

        let path = self.root.resolve(&parsed.path)?;

        let existed_before = tokio::fs::metadata(&path).await.is_ok();

        // Atomic, crash-safe write (creates parent dirs) — shared with
        // Edit/MultiEdit/NotebookEdit via `caliban_common::fs::write_atomic`.
        caliban_common::fs::write_atomic(&path, parsed.content.as_bytes())
            .map_err(ToolError::execution)?;

        // Fire FileChanged on success (best-effort).
        let kind = if existed_before {
            caliban_agent_core::FileChangeKind::Modified
        } else {
            caliban_agent_core::FileChangeKind::Created
        };
        cx.fire_file_changed(&path, kind, "Write").await;

        Ok(vec![ContentBlock::Text(TextBlock {
            text: format!(
                "→ Wrote {} ({} bytes)",
                self.root.relativize(&path).display(),
                parsed.content.len(),
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
    async fn writes_new_file() {
        let tmp = TempDir::new().unwrap();
        let tool = WriteTool::new(WorkspaceRoot::new(tmp.path()));
        let out = tool
            .invoke(json!({"path": "new.txt", "content": "hello world"}), ctx())
            .await
            .unwrap();

        // Response text contains byte count
        let ContentBlock::Text(t) = &out[0] else {
            panic!("expected Text block")
        };
        assert!(t.text.contains("Wrote"), "output: {}", t.text);
        assert!(t.text.contains("11 bytes"), "output: {}", t.text);

        // File actually exists with correct content
        let written = std::fs::read_to_string(tmp.path().join("new.txt")).unwrap();
        assert_eq!(written, "hello world");
    }

    #[tokio::test]
    async fn overwrites_existing_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("existing.txt");
        std::fs::write(&path, "old content").unwrap();

        let tool = WriteTool::new(WorkspaceRoot::new(tmp.path()));
        tool.invoke(
            json!({"path": "existing.txt", "content": "new content"}),
            ctx(),
        )
        .await
        .unwrap();

        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written, "new content");
    }

    #[tokio::test]
    async fn creates_missing_parent_dirs() {
        let tmp = TempDir::new().unwrap();
        let tool = WriteTool::new(WorkspaceRoot::new(tmp.path()));
        let out = tool
            .invoke(
                json!({"path": "nested/deeper/file.txt", "content": "deep content"}),
                ctx(),
            )
            .await
            .unwrap();

        let ContentBlock::Text(t) = &out[0] else {
            panic!("expected Text block")
        };
        assert!(t.text.contains("Wrote"), "output: {}", t.text);

        let written = std::fs::read_to_string(tmp.path().join("nested/deeper/file.txt")).unwrap();
        assert_eq!(written, "deep content");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn permission_denied_errors() {
        use std::os::unix::fs::PermissionsExt;

        // Restore permissions on drop so TempDir cleanup doesn't panic.
        struct RestorePerms(std::path::PathBuf);
        impl Drop for RestorePerms {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
            }
        }

        let tmp = TempDir::new().unwrap();
        let locked_dir = tmp.path().join("locked");
        std::fs::create_dir_all(&locked_dir).unwrap();
        // Remove all permissions from the directory.
        std::fs::set_permissions(&locked_dir, std::fs::Permissions::from_mode(0o000)).unwrap();
        let _restore = RestorePerms(locked_dir.clone());

        let tool = WriteTool::new(WorkspaceRoot::new(tmp.path()));
        let err = tool
            .invoke(
                json!({"path": "locked/denied.txt", "content": "should fail"}),
                ctx(),
            )
            .await
            .unwrap_err();

        assert!(matches!(err, ToolError::Execution(_)));
    }
}
