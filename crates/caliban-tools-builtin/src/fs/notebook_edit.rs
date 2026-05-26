//! `NotebookEdit` tool — read/write Jupyter `.ipynb` cells (nbformat v4 only).
//!
//! Actions: `add`, `edit`, `delete`. Preserves cell metadata + outputs across
//! edits. Atomic write via tmpfile + rename. Fires `FileChanged` after a
//! successful write.

use std::path::Path;
use std::sync::Arc;
use std::sync::OnceLock;

use async_trait::async_trait;
use caliban_agent_core::{Tool, ToolContext, ToolError};
use caliban_provider::{ContentBlock, TextBlock};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::workspace::WorkspaceRoot;

/// `NotebookEdit` tool — edits notebook cells in place.
#[derive(Debug)]
pub struct NotebookEditTool {
    root: Arc<WorkspaceRoot>,
    schema: OnceLock<Value>,
}

impl NotebookEditTool {
    /// Construct a `NotebookEdit` tool using the given workspace root.
    #[must_use]
    pub fn new(root: WorkspaceRoot) -> Self {
        Self {
            root: Arc::new(root),
            schema: OnceLock::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct NotebookEditInput {
    path: String,
    action: String,
    #[serde(default)]
    index: Option<usize>,
    #[serde(default)]
    cell_type: Option<String>,
    #[serde(default)]
    source: Option<String>,
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// Validate that the document is an nbformat v4 notebook with a `cells` array.
fn require_nbformat_v4(notebook: &Value) -> Result<(), ToolError> {
    let nbformat = notebook
        .get("nbformat")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            ToolError::execution(std::io::Error::other(
                "NotebookEdit: missing or invalid `nbformat` field",
            ))
        })?;
    if nbformat != 4 {
        return Err(ToolError::execution(std::io::Error::other(format!(
            "NotebookEdit requires nbformat 4; found {nbformat}"
        ))));
    }
    if !notebook
        .get("cells")
        .is_some_and(serde_json::Value::is_array)
    {
        return Err(ToolError::execution(std::io::Error::other(
            "NotebookEdit: missing `cells` array",
        )));
    }
    Ok(())
}

/// Convert a source string to the nbformat representation: an array of lines,
/// each ending with `\n` except possibly the last.
fn source_to_lines(s: &str) -> Vec<String> {
    if s.is_empty() {
        return vec![];
    }
    let mut lines: Vec<String> = s.split_inclusive('\n').map(str::to_string).collect();
    // Special case: an input ending without trailing newline gives the last
    // element with no `\n`; nbformat is fine with that.
    if lines.is_empty() {
        lines.push(s.to_string());
    }
    lines
}

/// Build a new cell with given type, source, and a fresh UUID id.
fn build_new_cell(cell_type: &str, source: &str) -> Result<Value, ToolError> {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let mut cell = Map::new();
    cell.insert("cell_type".into(), Value::String(cell_type.into()));
    cell.insert("id".into(), Value::String(id));
    cell.insert("metadata".into(), Value::Object(serde_json::Map::new()));
    cell.insert(
        "source".into(),
        Value::Array(
            source_to_lines(source)
                .into_iter()
                .map(Value::String)
                .collect(),
        ),
    );
    match cell_type {
        "code" => {
            cell.insert("execution_count".into(), Value::Null);
            cell.insert("outputs".into(), Value::Array(vec![]));
        }
        "markdown" | "raw" => {}
        other => {
            return Err(ToolError::invalid_input(format!(
                "unknown cell_type: {other} (expected code/markdown/raw)"
            )));
        }
    }
    Ok(Value::Object(cell))
}

/// Apply the action to the notebook in place, returning a human-readable
/// summary on success.
fn apply_action(notebook: &mut Value, input: &NotebookEditInput) -> Result<String, ToolError> {
    let cells = notebook
        .get_mut("cells")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| {
            ToolError::execution(std::io::Error::other("NotebookEdit: missing cells array"))
        })?;

    match input.action.as_str() {
        "add" => {
            let cell_type = input.cell_type.as_deref().unwrap_or("code");
            let source = input.source.as_deref().unwrap_or("");
            let new_cell = build_new_cell(cell_type, source)?;
            let position = match input.index {
                Some(i) if i <= cells.len() => i,
                Some(i) => {
                    return Err(ToolError::invalid_input(format!(
                        "index {i} out of bounds (notebook has {} cells)",
                        cells.len()
                    )));
                }
                None => cells.len(),
            };
            cells.insert(position, new_cell);
            Ok(format!("Added {cell_type} cell at index {position}."))
        }
        "edit" => {
            let i = input
                .index
                .ok_or_else(|| ToolError::invalid_input("edit requires `index`"))?;
            if i >= cells.len() {
                return Err(ToolError::invalid_input(format!(
                    "index {i} out of bounds (notebook has {} cells)",
                    cells.len()
                )));
            }
            let source = input
                .source
                .as_deref()
                .ok_or_else(|| ToolError::invalid_input("edit requires `source`"))?;
            let cell = cells
                .get_mut(i)
                .and_then(Value::as_object_mut)
                .ok_or_else(|| {
                    ToolError::execution(std::io::Error::other(format!(
                        "cell at index {i} is not an object"
                    )))
                })?;
            // Update source.
            cell.insert(
                "source".into(),
                Value::Array(
                    source_to_lines(source)
                        .into_iter()
                        .map(Value::String)
                        .collect(),
                ),
            );
            // Optionally change cell_type. When changing to/from code the
            // outputs/execution_count fields must follow the schema.
            if let Some(new_type) = input.cell_type.as_deref() {
                let prev = cell
                    .get("cell_type")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                cell.insert("cell_type".into(), Value::String(new_type.into()));
                match (prev.as_str(), new_type) {
                    ("code", "markdown" | "raw") => {
                        cell.remove("execution_count");
                        cell.remove("outputs");
                    }
                    ("markdown" | "raw", "code") => {
                        cell.insert("execution_count".into(), Value::Null);
                        cell.insert("outputs".into(), Value::Array(vec![]));
                    }
                    _ => {}
                }
            }
            Ok(format!("Edited cell at index {i}."))
        }
        "delete" => {
            let i = input
                .index
                .ok_or_else(|| ToolError::invalid_input("delete requires `index`"))?;
            if i >= cells.len() {
                return Err(ToolError::invalid_input(format!(
                    "index {i} out of bounds (notebook has {} cells)",
                    cells.len()
                )));
            }
            cells.remove(i);
            Ok(format!("Deleted cell at index {i}."))
        }
        other => Err(ToolError::invalid_input(format!(
            "unknown action: {other} (expected add/edit/delete)"
        ))),
    }
}

/// Atomic JSON write via [`caliban_common::fs::write_atomic`]. Returns the
/// final byte count.
fn atomic_write_json(path: &Path, value: &Value) -> Result<usize, ToolError> {
    let body = serde_json::to_vec_pretty(value)
        .map_err(|e| ToolError::execution(std::io::Error::other(format!("serialize: {e}"))))?;
    caliban_common::fs::write_atomic(path, &body).map_err(ToolError::execution)?;
    Ok(body.len())
}

#[async_trait]
impl Tool for NotebookEditTool {
    fn name(&self) -> &'static str {
        "NotebookEdit"
    }

    fn description(&self) -> &'static str {
        "Edit cells in a Jupyter notebook (.ipynb, nbformat v4 only). Actions: add (insert a new cell, optionally at index), edit (replace cell source and optionally cell_type at index), delete (remove cell at index). Preserves cell metadata + outputs across edits. Atomic write."
    }

    fn input_schema(&self) -> &Value {
        self.schema.get_or_init(|| json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to .ipynb (relative to workspace root or absolute)" },
                "action": { "type": "string", "enum": ["add", "edit", "delete"] },
                "index": { "type": "integer", "minimum": 0, "description": "Cell index (required for edit/delete; optional for add — appends if omitted)" },
                "cell_type": { "type": "string", "enum": ["code", "markdown", "raw"], "description": "Cell type (required for add; optional for edit)" },
                "source": { "type": "string", "description": "Cell source (required for add/edit)" }
            },
            "required": ["path", "action"]
        }))
    }

    fn parallel_conflict_key(&self, input: &Value) -> Option<String> {
        input
            .get("path")
            .and_then(Value::as_str)
            .map(crate::parallel::canonical_key)
    }

    async fn invoke(&self, input: Value, cx: ToolContext) -> Result<Vec<ContentBlock>, ToolError> {
        let parsed: NotebookEditInput = serde_json::from_value(input)
            .map_err(|e| ToolError::invalid_input(format!("invalid input: {e}")))?;

        let path = self.root.resolve(&parsed.path)?;
        let body = tokio::fs::read_to_string(&path)
            .await
            .map_err(ToolError::execution)?;
        let mut notebook: Value = serde_json::from_str(&body).map_err(|e| {
            ToolError::execution(std::io::Error::other(format!("invalid notebook JSON: {e}")))
        })?;
        require_nbformat_v4(&notebook)?;

        let summary = apply_action(&mut notebook, &parsed)?;

        let path_clone = path.clone();
        let value_clone = notebook;
        let bytes =
            tokio::task::spawn_blocking(move || atomic_write_json(&path_clone, &value_clone))
                .await
                .map_err(|e| ToolError::execution(std::io::Error::other(format!("{e}"))))??;

        // Fire FileChanged on success (best-effort).
        if let Some(hooks) = cx.hooks.as_ref() {
            let fc_ctx = caliban_agent_core::FileChangedCtx {
                path: &path,
                kind: caliban_agent_core::FileChangeKind::Modified,
                tool: "NotebookEdit",
            };
            if let Err(e) = hooks.file_changed(&fc_ctx).await {
                tracing::warn!(error = %e, "file_changed hook error (non-fatal)");
            }
        }

        Ok(vec![ContentBlock::Text(TextBlock {
            text: format!(
                "→ NotebookEdit {}: {} ({} bytes written)",
                self.root.relativize(&path).display(),
                summary,
                bytes
            ),
            cache_control: None,
        })])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use caliban_agent_core::{FileChangedCtx, Hooks};
    use serde_json::json;
    use std::sync::{Arc, Mutex};
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

    /// Captures `FileChanged` events for assertion.
    #[derive(Default)]
    struct RecordingHooks {
        events: Mutex<Vec<(std::path::PathBuf, String, String)>>,
    }

    #[async_trait]
    impl Hooks for RecordingHooks {
        async fn file_changed(&self, ctx: &FileChangedCtx<'_>) -> caliban_agent_core::Result<()> {
            self.events.lock().unwrap().push((
                ctx.path.to_path_buf(),
                ctx.kind.as_str().to_string(),
                ctx.tool.to_string(),
            ));
            Ok(())
        }
    }

    fn minimal_notebook_v4() -> Value {
        json!({
            "nbformat": 4,
            "nbformat_minor": 5,
            "metadata": { "kernelspec": { "name": "python3" } },
            "cells": [
                {
                    "cell_type": "markdown",
                    "id": "cell-a",
                    "metadata": {},
                    "source": ["# Heading\n"]
                },
                {
                    "cell_type": "code",
                    "id": "cell-b",
                    "metadata": { "tags": ["important"] },
                    "execution_count": 2,
                    "outputs": [
                        { "output_type": "stream", "name": "stdout", "text": ["hi\n"] }
                    ],
                    "source": ["print('hi')\n"]
                }
            ]
        })
    }

    async fn write_notebook(tmp: &TempDir, contents: &Value) -> std::path::PathBuf {
        let path = tmp.path().join("nb.ipynb");
        tokio::fs::write(&path, serde_json::to_vec_pretty(contents).unwrap())
            .await
            .unwrap();
        path
    }

    // ----------------------------------------------------------------------

    #[tokio::test]
    async fn add_cell_at_index_inserts_before() {
        let tmp = TempDir::new().unwrap();
        let _path = write_notebook(&tmp, &minimal_notebook_v4()).await;

        let tool = NotebookEditTool::new(WorkspaceRoot::new(tmp.path()));
        let out = tool
            .invoke(
                json!({
                    "path": "nb.ipynb",
                    "action": "add",
                    "index": 1,
                    "cell_type": "markdown",
                    "source": "## inserted\n"
                }),
                ctx(),
            )
            .await
            .unwrap();
        let ContentBlock::Text(t) = &out[0] else {
            panic!("expected Text")
        };
        assert!(
            t.text.contains("Added markdown cell at index 1"),
            "out: {}",
            t.text
        );

        let body = tokio::fs::read_to_string(tmp.path().join("nb.ipynb"))
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(&body).unwrap();
        let cells = parsed.get("cells").unwrap().as_array().unwrap();
        assert_eq!(cells.len(), 3);
        assert_eq!(cells[1].get("cell_type").unwrap(), "markdown");
        // Original cell-b is now at index 2.
        assert_eq!(cells[2].get("id").unwrap(), "cell-b");
    }

    #[tokio::test]
    async fn add_appends_when_index_omitted() {
        let tmp = TempDir::new().unwrap();
        let _path = write_notebook(&tmp, &minimal_notebook_v4()).await;

        let tool = NotebookEditTool::new(WorkspaceRoot::new(tmp.path()));
        tool.invoke(
            json!({
                "path": "nb.ipynb",
                "action": "add",
                "cell_type": "code",
                "source": "x = 1\n"
            }),
            ctx(),
        )
        .await
        .unwrap();

        let body = tokio::fs::read_to_string(tmp.path().join("nb.ipynb"))
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(&body).unwrap();
        let cells = parsed.get("cells").unwrap().as_array().unwrap();
        assert_eq!(cells.len(), 3);
        let last = &cells[2];
        assert_eq!(last.get("cell_type").unwrap(), "code");
        // New code cells get execution_count: null and outputs: [].
        assert!(last.get("execution_count").unwrap().is_null());
        assert!(last.get("outputs").unwrap().as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn edit_preserves_metadata_and_outputs() {
        let tmp = TempDir::new().unwrap();
        let _path = write_notebook(&tmp, &minimal_notebook_v4()).await;

        let tool = NotebookEditTool::new(WorkspaceRoot::new(tmp.path()));
        tool.invoke(
            json!({
                "path": "nb.ipynb",
                "action": "edit",
                "index": 1,
                "source": "print('updated')\n"
            }),
            ctx(),
        )
        .await
        .unwrap();

        let body = tokio::fs::read_to_string(tmp.path().join("nb.ipynb"))
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(&body).unwrap();
        let cell = &parsed.get("cells").unwrap().as_array().unwrap()[1];
        assert_eq!(cell.get("id").unwrap(), "cell-b");
        assert_eq!(
            cell.get("metadata").unwrap().get("tags").unwrap(),
            &json!(["important"])
        );
        let outputs = cell.get("outputs").unwrap().as_array().unwrap();
        assert_eq!(outputs.len(), 1);
        let source = cell.get("source").unwrap().as_array().unwrap();
        assert_eq!(source[0], json!("print('updated')\n"));
    }

    #[tokio::test]
    async fn delete_shifts_indices() {
        let tmp = TempDir::new().unwrap();
        let _path = write_notebook(&tmp, &minimal_notebook_v4()).await;

        let tool = NotebookEditTool::new(WorkspaceRoot::new(tmp.path()));
        tool.invoke(
            json!({"path": "nb.ipynb", "action": "delete", "index": 0}),
            ctx(),
        )
        .await
        .unwrap();

        let body = tokio::fs::read_to_string(tmp.path().join("nb.ipynb"))
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(&body).unwrap();
        let cells = parsed.get("cells").unwrap().as_array().unwrap();
        assert_eq!(cells.len(), 1);
        // The surviving cell is the formerly-second one.
        assert_eq!(cells[0].get("id").unwrap(), "cell-b");
    }

    #[tokio::test]
    async fn rejects_nbformat_v3() {
        let tmp = TempDir::new().unwrap();
        let mut nb = minimal_notebook_v4();
        nb["nbformat"] = json!(3);
        let _path = write_notebook(&tmp, &nb).await;

        let tool = NotebookEditTool::new(WorkspaceRoot::new(tmp.path()));
        let err = tool
            .invoke(
                json!({"path": "nb.ipynb", "action": "delete", "index": 0}),
                ctx(),
            )
            .await
            .unwrap_err();
        let msg = format!("{err}");
        assert!(matches!(err, ToolError::Execution(_)), "got: {err:?}");
        assert!(msg.contains("nbformat 4"), "msg: {msg}");
    }

    #[tokio::test]
    async fn write_is_atomic_well_formed_json() {
        // Verify after a successful edit that the file is parseable JSON.
        let tmp = TempDir::new().unwrap();
        let _path = write_notebook(&tmp, &minimal_notebook_v4()).await;
        let tool = NotebookEditTool::new(WorkspaceRoot::new(tmp.path()));
        tool.invoke(
            json!({"path": "nb.ipynb", "action": "edit", "index": 0, "source": "# updated\n"}),
            ctx(),
        )
        .await
        .unwrap();
        let body = tokio::fs::read_to_string(tmp.path().join("nb.ipynb"))
            .await
            .unwrap();
        let _v: Value = serde_json::from_str(&body).expect("file must be valid JSON");
    }

    #[tokio::test]
    async fn file_changed_hook_fires_on_success() {
        let tmp = TempDir::new().unwrap();
        let _path = write_notebook(&tmp, &minimal_notebook_v4()).await;
        let hooks = Arc::new(RecordingHooks::default());
        let tool = NotebookEditTool::new(WorkspaceRoot::new(tmp.path()));
        let cx = ToolContext {
            tool_use_id: "t1".into(),
            cancel: CancellationToken::new(),
            hooks: Some(hooks.clone() as Arc<dyn Hooks + Send + Sync>),
            turn_index: 0,
        };
        tool.invoke(
            json!({"path": "nb.ipynb", "action": "delete", "index": 0}),
            cx,
        )
        .await
        .unwrap();
        let events = hooks.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].2, "NotebookEdit");
    }
}
