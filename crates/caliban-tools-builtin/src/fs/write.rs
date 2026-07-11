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

    fn mutates_files(&self) -> bool {
        true
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
        let path = input.get("path").and_then(Value::as_str)?;
        // #417: key on the *resolved* workspace target (the same resolution the
        // write uses) so different spellings of one file serialize; fall back to
        // the raw string key if resolution fails.
        Some(self.root.resolve(path).map_or_else(
            |_| crate::parallel::canonical_key(path),
            |r| crate::parallel::canonical_key_path(&r),
        ))
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

    #[test]
    fn parallel_conflict_key_is_stable_across_spellings() {
        // #417: the same target via a relative path and an absolute path must
        // yield the same conflict key so a concurrent Edit+Write serialize.
        // Keying on the resolved workspace target (not the raw string vs cwd)
        // is what makes this hold when the workspace root differs from the cwd.
        let tmp = TempDir::new().unwrap();
        let tool = WriteTool::new(WorkspaceRoot::new(tmp.path()));
        let rel = tool
            .parallel_conflict_key(&json!({"path": "f.txt"}))
            .expect("key");
        let abs_path = tmp.path().join("f.txt");
        let abs = tool
            .parallel_conflict_key(&json!({"path": abs_path.to_str().unwrap()}))
            .expect("key");
        assert_eq!(
            rel, abs,
            "relative and absolute spellings of one target must key the same",
        );
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

    #[cfg(unix)]
    #[tokio::test]
    async fn new_file_is_0644_not_0600() {
        // #224: the atomic write path used to leak the tempfile's 0600.
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let tool = WriteTool::new(WorkspaceRoot::new(tmp.path()));
        tool.invoke(json!({"path": "created.txt", "content": "x"}), ctx())
            .await
            .unwrap();
        let mode = std::fs::metadata(tmp.path().join("created.txt"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o644, "new file mode {mode:o}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rewrite_preserves_existing_mode() {
        // The reported failure: a full rewrite of an existing 0644 source file
        // dropped it to 0600. It must keep the destination's mode.
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("src.ts");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let tool = WriteTool::new(WorkspaceRoot::new(tmp.path()));
        tool.invoke(json!({"path": "src.ts", "content": "rewritten"}), ctx())
            .await
            .unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "rewrite mode {mode:o}");
    }
}
