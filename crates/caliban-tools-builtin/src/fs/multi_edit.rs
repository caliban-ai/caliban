//! `MultiEdit` tool — apply a sequence of `{old_string, new_string,
//! replace_all?}` edits to a single file, atomically. If any edit fails to
//! match, the entire operation is rolled back and the file is unchanged.

use std::sync::Arc;
use std::sync::OnceLock;

use async_trait::async_trait;
use caliban_agent_core::{Tool, ToolContext, ToolError};
use caliban_provider::{ContentBlock, TextBlock};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::fs::match_old::{self, MatchOutcome};
use crate::workspace::WorkspaceRoot;

/// `MultiEdit` tool — sequential atomic replacements on one file.
#[derive(Debug)]
pub struct MultiEditTool {
    root: Arc<WorkspaceRoot>,
    schema: OnceLock<Value>,
}

impl MultiEditTool {
    /// Construct a `MultiEdit` tool using the given workspace root.
    #[must_use]
    pub fn new(root: WorkspaceRoot) -> Self {
        Self {
            root: Arc::new(root),
            schema: OnceLock::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct MultiEditInput {
    path: String,
    edits: Vec<EditOp>,
}

#[derive(Debug, Deserialize)]
struct EditOp {
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

/// Apply a sequence of edits to `text` in memory, returning the final text
/// and a per-edit replacement count. On any miss/ambiguity returns an error
/// describing the failing edit (1-indexed) — the caller MUST discard the
/// in-memory string.
fn apply_edits(text: String, edits: &[EditOp]) -> Result<(String, Vec<usize>), ToolError> {
    let mut current = text;
    let mut counts = Vec::with_capacity(edits.len());
    for (idx, e) in edits.iter().enumerate() {
        let n = idx + 1; // 1-based edit number for error messages
        let outcome = match_old::locate(&current, &e.old_string, &e.new_string, e.replace_all);
        match outcome {
            MatchOutcome::Located {
                ranges,
                replacement,
                tier,
            } => {
                if tier == match_old::MatchTier::Whitespace {
                    tracing::debug!(
                        edit = n,
                        "MultiEdit: edit #{n} matched via whitespace-tolerant tier"
                    );
                }
                let count = ranges.len();
                // Apply ranges in reverse byte order so earlier offsets stay valid.
                for range in ranges.iter().rev() {
                    current.replace_range(range.clone(), &replacement);
                }
                counts.push(count);
            }
            MatchOutcome::Ambiguous { count, locations } => {
                let locs: Vec<String> = locations
                    .iter()
                    .map(|(s, e)| format!("lines {s}-{e}"))
                    .collect();
                return Err(ToolError::execution(std::io::Error::other(format!(
                    "edit #{n}: old_string matched {count} times; expected exactly one (use replace_all=true). Locations: {}",
                    locs.join(", ")
                ))));
            }
            MatchOutcome::NotFound { near } => {
                let body = match near {
                    Some(nm) => nm.render(),
                    None => "old_string not found in current contents (rolling back)".to_string(),
                };
                return Err(ToolError::execution(std::io::Error::other(format!(
                    "edit #{n}: {body}"
                ))));
            }
        }
    }
    Ok((current, counts))
}

#[async_trait]
impl Tool for MultiEditTool {
    fn name(&self) -> &'static str {
        "MultiEdit"
    }

    fn description(&self) -> &'static str {
        "Apply a sequence of {old_string, new_string, replace_all?} edits to a single file, atomically. Each edit operates on the result of the prior edit. If any edit's old_string is missing or matches multiple times without replace_all=true, the entire operation is aborted and the file is left unchanged."
    }

    fn input_schema(&self) -> &Value {
        self.schema.get_or_init(|| json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to edit (relative to workspace root or absolute)" },
                "edits": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "properties": {
                            "old_string": { "type": "string" },
                            "new_string": { "type": "string" },
                            "replace_all": { "type": "boolean", "default": false }
                        },
                        "required": ["old_string", "new_string"]
                    }
                }
            },
            "required": ["path", "edits"]
        }))
    }

    fn parallel_conflict_key(&self, input: &Value) -> Option<String> {
        input
            .get("path")
            .and_then(Value::as_str)
            .map(crate::parallel::canonical_key)
    }

    async fn invoke(&self, input: Value, cx: ToolContext) -> Result<Vec<ContentBlock>, ToolError> {
        let parsed: MultiEditInput = crate::parse_input(input)?;
        if parsed.edits.is_empty() {
            return Err(ToolError::invalid_input("edits array must be non-empty"));
        }

        let path = self.root.resolve(&parsed.path)?;
        let text = tokio::fs::read_to_string(&path)
            .await
            .map_err(ToolError::execution)?;

        let (final_text, counts) = apply_edits(text, &parsed.edits)?;

        let path_clone = path.clone();
        let body = final_text.clone();
        // Atomic, crash-safe write — shared via `caliban_common::fs::write_atomic`.
        tokio::task::spawn_blocking(move || {
            caliban_common::fs::write_atomic(&path_clone, body.as_bytes())
                .map_err(ToolError::execution)
        })
        .await
        .map_err(|e| ToolError::execution(std::io::Error::other(format!("{e}"))))??;

        cx.fire_file_changed(
            &path,
            caliban_agent_core::FileChangeKind::Modified,
            "MultiEdit",
        )
        .await;

        let total: usize = counts.iter().sum();
        Ok(vec![ContentBlock::Text(TextBlock {
            text: format!(
                "→ MultiEdit {} ({} edit{}, {} total replacement{})",
                self.root.relativize(&path).display(),
                counts.len(),
                if counts.len() == 1 { "" } else { "s" },
                total,
                if total == 1 { "" } else { "s" },
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

    #[derive(Default)]
    struct RecordingHooks {
        events: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl Hooks for RecordingHooks {
        async fn file_changed(&self, ctx: &FileChangedCtx<'_>) -> caliban_agent_core::Result<()> {
            self.events.lock().unwrap().push(ctx.tool.to_string());
            Ok(())
        }
    }

    // ----------------------------------------------------------------------
    // Pure apply_edits tests
    // ----------------------------------------------------------------------

    #[test]
    fn sequential_apply_happy_path() {
        let edits = vec![
            EditOp {
                old_string: "foo".into(),
                new_string: "bar".into(),
                replace_all: false,
            },
            EditOp {
                old_string: "bar".into(),
                new_string: "baz".into(),
                replace_all: false,
            },
        ];
        let (out, counts) = apply_edits("hello foo world".into(), &edits).unwrap();
        assert_eq!(out, "hello baz world");
        assert_eq!(counts, vec![1, 1]);
    }

    #[test]
    fn rollback_when_second_edit_misses() {
        let edits = vec![
            EditOp {
                old_string: "foo".into(),
                new_string: "bar".into(),
                replace_all: false,
            },
            EditOp {
                old_string: "MISSING".into(),
                new_string: "x".into(),
                replace_all: false,
            },
        ];
        let err = apply_edits("hello foo world".into(), &edits).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("edit #2"), "msg: {msg}");
    }

    #[test]
    fn duplicate_without_replace_all_fails() {
        let edits = vec![EditOp {
            old_string: "x".into(),
            new_string: "y".into(),
            replace_all: false,
        }];
        let err = apply_edits("x and x".into(), &edits).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("matched 2 times"), "msg: {msg}");
    }

    #[test]
    fn replace_all_replaces_every_occurrence() {
        let edits = vec![EditOp {
            old_string: "x".into(),
            new_string: "Y".into(),
            replace_all: true,
        }];
        let (out, counts) = apply_edits("x and x and x".into(), &edits).unwrap();
        assert_eq!(out, "Y and Y and Y");
        assert_eq!(counts, vec![3]);
    }

    // ----------------------------------------------------------------------
    // Tool::invoke integration
    // ----------------------------------------------------------------------

    #[tokio::test]
    async fn invoke_writes_file_on_success() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("file.txt");
        tokio::fs::write(&path, "alpha beta gamma").await.unwrap();
        let tool = MultiEditTool::new(WorkspaceRoot::new(tmp.path()));
        tool.invoke(
            json!({
                "path": "file.txt",
                "edits": [
                    {"old_string": "alpha", "new_string": "ALPHA"},
                    {"old_string": "gamma", "new_string": "GAMMA"}
                ]
            }),
            ctx(),
        )
        .await
        .unwrap();
        let written = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(written, "ALPHA beta GAMMA");
    }

    #[tokio::test]
    async fn rollback_leaves_file_unchanged() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("file.txt");
        let original = "the quick brown fox";
        tokio::fs::write(&path, original).await.unwrap();
        let tool = MultiEditTool::new(WorkspaceRoot::new(tmp.path()));
        let err = tool
            .invoke(
                json!({
                    "path": "file.txt",
                    "edits": [
                        {"old_string": "the", "new_string": "THE"},
                        {"old_string": "MISSING", "new_string": "X"}
                    ]
                }),
                ctx(),
            )
            .await
            .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("edit #2"), "msg: {msg}");
        let after = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(after, original, "file must be unchanged after rollback");
    }

    #[tokio::test]
    async fn invoke_atomic_write_writes_completely() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("file.txt");
        tokio::fs::write(&path, "X").await.unwrap();
        let tool = MultiEditTool::new(WorkspaceRoot::new(tmp.path()));
        tool.invoke(
            json!({
                "path": "file.txt",
                "edits": [{"old_string": "X", "new_string": "Y"}]
            }),
            ctx(),
        )
        .await
        .unwrap();
        // After write, only the file is present (no leftover tempfile in same dir).
        let mut found_other = false;
        for entry in std::fs::read_dir(tmp.path()).unwrap() {
            let e = entry.unwrap();
            if e.path() != path {
                found_other = true;
            }
        }
        assert!(!found_other, "tempfile should have been renamed away");
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), "Y");
    }

    #[tokio::test]
    async fn file_changed_hook_fires_on_success() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("file.txt");
        tokio::fs::write(&path, "AB").await.unwrap();
        let hooks = Arc::new(RecordingHooks::default());
        let tool = MultiEditTool::new(WorkspaceRoot::new(tmp.path()));
        let cx = ToolContext {
            tool_use_id: "t1".into(),
            cancel: CancellationToken::new(),
            hooks: Some(hooks.clone() as Arc<dyn Hooks + Send + Sync>),
            turn_index: 0,
        };
        tool.invoke(
            json!({
                "path": "file.txt",
                "edits": [{"old_string": "A", "new_string": "Z"}]
            }),
            cx,
        )
        .await
        .unwrap();
        assert_eq!(hooks.events.lock().unwrap().as_slice(), &["MultiEdit"]);
    }

    #[tokio::test]
    async fn empty_edits_array_rejected() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("file.txt");
        tokio::fs::write(&path, "x").await.unwrap();
        let tool = MultiEditTool::new(WorkspaceRoot::new(tmp.path()));
        let err = tool
            .invoke(json!({"path": "file.txt", "edits": []}), ctx())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)), "got: {err:?}");
    }

    // ------------------------------------------------------------------
    // Whitespace-tolerant matching tests (match_old integration)
    // ------------------------------------------------------------------

    /// A whitespace-only-different edit within a sequence applies: the second
    /// edit has trailing whitespace the file doesn't, but the matcher's
    /// whitespace-tolerant tier should match it and apply the replacement.
    #[test]
    fn whitespace_tolerant_edit_in_sequence_applies() {
        let edits = vec![
            EditOp {
                old_string: "alpha".into(),
                new_string: "ALPHA".into(),
                replace_all: false,
            },
            // old_string has trailing spaces; the running text won't (after first edit).
            EditOp {
                old_string: "beta   \ngamma".into(), // trailing ws on first line
                new_string: "BETA\nGAMMA".into(),
                replace_all: false,
            },
        ];
        let text = "alpha\nbeta\ngamma\n".to_string();
        let (out, counts) = apply_edits(text, &edits).unwrap();
        assert_eq!(out, "ALPHA\nBETA\nGAMMA\n");
        assert_eq!(counts, vec![1, 1]);
    }

    /// A missing edit (not found, near-miss available) rolls back the WHOLE file:
    /// - error message is prefixed `edit #N:`
    /// - error message contains near-miss diff markers
    /// - file on disk is unchanged
    #[tokio::test]
    async fn missing_edit_rolls_back_file_and_error_has_near_miss() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("file.txt");
        let original = "fn alpha() {\n    do_thing();\n}\n";
        tokio::fs::write(&path, original).await.unwrap();

        let tool = MultiEditTool::new(WorkspaceRoot::new(tmp.path()));
        let err = tool
            .invoke(
                json!({
                    "path": "file.txt",
                    "edits": [
                        // First edit succeeds.
                        {"old_string": "fn alpha", "new_string": "fn alpha"},
                        // Second edit: close but wrong — do_OTHER vs do_thing → near-miss.
                        {
                            "old_string": "fn alpha() {\n    do_OTHER();\n}",
                            "new_string": "fn alpha() {}"
                        }
                    ]
                }),
                ctx(),
            )
            .await
            .unwrap_err();

        let msg = format!("{err}");
        // Must be prefixed edit #2.
        assert!(msg.contains("edit #2"), "expected 'edit #2' prefix: {msg}");
        // Must contain near-miss diff markers.
        assert!(
            msg.contains("- ") || msg.contains("+ "),
            "expected diff in near-miss: {msg}"
        );
        assert!(
            msg.contains("do_OTHER") || msg.contains("do_thing"),
            "expected diff content: {msg}"
        );
        // File must be unchanged (rollback).
        let after = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(after, original, "file must be unchanged after rollback");
    }

    /// When `old_string` is longer than the file, near-miss scan returns None and
    /// the error message uses the bare fallback text (not a diff), still prefixed
    /// `edit #N:`.
    #[tokio::test]
    async fn not_found_near_none_bare_message_with_prefix() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("file.txt");
        // 1-line file; old_string spans 3 lines → window cannot fit → near: None.
        tokio::fs::write(&path, "hello\n").await.unwrap();

        let tool = MultiEditTool::new(WorkspaceRoot::new(tmp.path()));
        let err = tool
            .invoke(
                json!({
                    "path": "file.txt",
                    "edits": [
                        {"old_string": "aaa\nbbb\nccc", "new_string": "replaced"}
                    ]
                }),
                ctx(),
            )
            .await
            .unwrap_err();

        let msg = format!("{err}");
        // Must have edit #1 prefix.
        assert!(msg.contains("edit #1"), "expected 'edit #1' prefix: {msg}");
        // Must use bare fallback (no near-miss diff).
        assert!(
            msg.contains("old_string not found in current contents"),
            "expected bare not-found message: {msg}"
        );
        assert!(
            !msg.contains("closest match"),
            "should not have near-miss when old longer than file: {msg}"
        );

        // File must be unchanged.
        let after = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(after, "hello\n", "file must be unchanged after failed edit");
    }
}
