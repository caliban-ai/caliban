//! Test that `WriteTool` / `EditTool` fire `FileChanged` events on success.

use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use caliban_agent_core::{FileChangeKind, FileChangedCtx, Hooks, Result, Tool, ToolContext};
use caliban_tools_builtin::{EditTool, WorkspaceRoot, WriteTool};
use serde_json::json;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct RecorderHooks {
    events: Mutex<Vec<(String, String, FileChangeKind)>>,
}

#[async_trait]
impl Hooks for RecorderHooks {
    async fn file_changed(&self, ctx: &FileChangedCtx<'_>) -> Result<()> {
        self.events.lock().unwrap().push((
            ctx.tool.into(),
            ctx.path.display().to_string(),
            ctx.kind,
        ));
        Ok(())
    }
}

fn ctx_with_hooks(hooks: Arc<RecorderHooks>) -> ToolContext {
    ToolContext {
        tool_use_id: "t1".into(),
        cancel: CancellationToken::new(),
        hooks: Some(hooks),
        turn_index: 0,
    }
}

#[tokio::test]
async fn write_creates_new_file_fires_created() {
    let tmp = TempDir::new().unwrap();
    let tool = WriteTool::new(WorkspaceRoot::new(tmp.path()));
    let hooks = Arc::new(RecorderHooks::default());
    let cx = ctx_with_hooks(Arc::clone(&hooks));
    tool.invoke(json!({"path": "new.txt", "content": "hello"}), cx)
        .await
        .unwrap();
    let ev = hooks.events.lock().unwrap().clone();
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].0, "Write");
    assert_eq!(ev[0].2, FileChangeKind::Created);
}

#[tokio::test]
async fn write_overwrite_fires_modified() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("x.txt"), "old").unwrap();
    let tool = WriteTool::new(WorkspaceRoot::new(tmp.path()));
    let hooks = Arc::new(RecorderHooks::default());
    let cx = ctx_with_hooks(Arc::clone(&hooks));
    tool.invoke(json!({"path": "x.txt", "content": "new"}), cx)
        .await
        .unwrap();
    let ev = hooks.events.lock().unwrap().clone();
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].0, "Write");
    assert_eq!(ev[0].2, FileChangeKind::Modified);
}

#[tokio::test]
async fn edit_fires_modified() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("y.txt"), "hello world").unwrap();
    let tool = EditTool::new(WorkspaceRoot::new(tmp.path()));
    let hooks = Arc::new(RecorderHooks::default());
    let cx = ctx_with_hooks(Arc::clone(&hooks));
    tool.invoke(
        json!({
            "path": "y.txt",
            "old_string": "world",
            "new_string": "rust",
            "replace_all": false,
        }),
        cx,
    )
    .await
    .unwrap();
    let ev = hooks.events.lock().unwrap().clone();
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].0, "Edit");
    assert_eq!(ev[0].2, FileChangeKind::Modified);
}

#[tokio::test]
async fn write_without_hooks_works() {
    // Pass `hooks: None` — the tool should work without panicking.
    let tmp = TempDir::new().unwrap();
    let tool = WriteTool::new(WorkspaceRoot::new(tmp.path()));
    let cx = ToolContext {
        tool_use_id: "t1".into(),
        cancel: CancellationToken::new(),
        hooks: None,
        turn_index: 0,
    };
    let out = tool
        .invoke(json!({"path": "z.txt", "content": "no hook"}), cx)
        .await
        .unwrap();
    assert!(matches!(
        out.first(),
        Some(caliban_provider::ContentBlock::Text(_))
    ));
}
