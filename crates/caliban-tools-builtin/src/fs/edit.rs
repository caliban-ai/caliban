//! Edit tool — replace occurrences of a string within a file.

use std::sync::Arc;
use std::sync::OnceLock;

use async_trait::async_trait;
use caliban_agent_core::{Tool, ToolContext, ToolError};
use caliban_provider::{ContentBlock, TextBlock};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::fs::match_old::{self, MatchOutcome};
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

    fn mutates_files(&self) -> bool {
        true
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
        let path = input.get("path").and_then(Value::as_str)?;
        // #417: key on the *resolved* workspace target so different spellings of
        // one file serialize; fall back to the raw string key.
        Some(self.root.resolve(path).map_or_else(
            |_| crate::parallel::canonical_key(path),
            |r| crate::parallel::canonical_key_path(&r),
        ))
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

        let outcome = match_old::locate(
            &text,
            &parsed.old_string,
            &parsed.new_string,
            parsed.replace_all,
        );

        let (ranges, replacement) = match outcome {
            MatchOutcome::Located {
                ranges,
                replacement,
                tier,
            } => {
                if tier == match_old::MatchTier::Whitespace {
                    tracing::debug!(
                        path = %path.display(),
                        "Edit: matched via whitespace-tolerant tier"
                    );
                }
                (ranges, replacement)
            }
            MatchOutcome::Ambiguous { count, locations } => {
                let locs: Vec<String> = locations
                    .iter()
                    .map(|(s, e)| format!("lines {s}-{e}"))
                    .collect();
                return Err(ToolError::execution(std::io::Error::other(format!(
                    "old_string matched {count} times; expected exactly one (use replace_all=true to replace all). Locations: {}",
                    locs.join(", ")
                ))));
            }
            MatchOutcome::NotFound { near } => {
                let msg = match near {
                    Some(nm) => nm.render(),
                    None => "old_string not found in file".to_string(),
                };
                return Err(ToolError::execution(std::io::Error::other(msg)));
            }
        };

        // Apply ranges in reverse byte order so earlier offsets stay valid.
        let count = ranges.len();
        let mut replaced = text.clone();
        for range in ranges.iter().rev() {
            replaced.replace_range(range.clone(), &replacement);
        }

        // Confined atomic write (#415): symlink-refusing parent walk in
        // restricted mode keeps the write inside the workspace fence.
        self.root
            .atomic_write(&path, replaced.as_bytes())
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
        // After the match_old integration, a miss returns either a near-miss
        // diff (NearMiss::render begins with "closest match near line …") or
        // the bare "old_string not found in file" fallback when no near-miss
        // is available.  Either is acceptable; anything else indicates
        // regression in the error path.
        let msg = format!("{err}");
        assert!(
            msg.contains("closest match") || msg.contains("old_string not found in file"),
            "unexpected error message format: {msg}"
        );
    }

    /// When `old_string` has MORE lines than the file, `nearest_window` returns
    /// `None` (the guard that prevents an out-of-bounds slice), so the error
    /// message must be the bare `"old_string not found in file"` fallback — NOT
    /// a near-miss diff.  This test exercises that path end-to-end through
    /// `EditTool::invoke` rather than calling `match_old::locate` directly.
    #[tokio::test]
    async fn not_found_near_none_when_old_longer_than_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("file.txt");
        // 1-line file; old_string spans 3 lines → window cannot fit → near: None.
        std::fs::write(&path, "hello\n").unwrap();

        let tool = EditTool::new(WorkspaceRoot::new(tmp.path()));
        let err = tool
            .invoke(
                json!({
                    "path": "file.txt",
                    "old_string": "aaa\nbbb\nccc",
                    "new_string": "replaced"
                }),
                ctx(),
            )
            .await
            .unwrap_err();

        assert!(matches!(err, ToolError::Execution(_)));
        let msg = format!("{err}");
        // Must be the bare fallback — near-miss scan returns None when old is
        // longer than the file, so "closest match" must NOT appear.
        assert!(
            msg.contains("old_string not found in file"),
            "expected bare not-found message, got: {msg}"
        );
        assert!(
            !msg.contains("closest match"),
            "near-miss should be None for over-long old_string, got: {msg}"
        );

        // File must be unchanged.
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            contents, "hello\n",
            "file should be unchanged after failed edit"
        );
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

    /// Trailing whitespace in `old_string` is tolerated: the edit still applies
    /// and the file is written with the correct result.
    #[tokio::test]
    async fn trailing_whitespace_in_old_string_still_applies() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("file.txt");
        // File has no trailing whitespace on the first line.
        std::fs::write(&path, "let x = 1;\nlet y = 2;\n").unwrap();

        let tool = EditTool::new(WorkspaceRoot::new(tmp.path()));
        // old_string has trailing spaces on line 1, which the file doesn't.
        let out = tool
            .invoke(
                json!({
                    "path": "file.txt",
                    "old_string": "let x = 1;   \nlet y = 2;",
                    "new_string": "let x = 9;\nlet y = 8;"
                }),
                ctx(),
            )
            .await
            .unwrap();

        let ContentBlock::Text(t) = &out[0] else {
            panic!("expected Text block")
        };
        assert!(t.text.contains("1 replacement"), "output: {}", t.text);

        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written, "let x = 9;\nlet y = 8;\n");
    }

    /// `old_string` uniformly under-indented still matches; the written file
    /// has the correct indentation (the reindented `new_string`).
    #[tokio::test]
    async fn uniform_underindent_applies_with_correct_indentation() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("file.txt");
        // File has 4-space indented block.
        std::fs::write(&path, "    if x {\n        y();\n    }\n").unwrap();

        let tool = EditTool::new(WorkspaceRoot::new(tmp.path()));
        // old_string is un-indented — uniformly under-indented by 4 spaces.
        let out = tool
            .invoke(
                json!({
                    "path": "file.txt",
                    "old_string": "if x {\n    y();\n}",
                    "new_string": "if x {\n    z();\n}"
                }),
                ctx(),
            )
            .await
            .unwrap();

        let ContentBlock::Text(t) = &out[0] else {
            panic!("expected Text block")
        };
        assert!(t.text.contains("1 replacement"), "output: {}", t.text);

        let written = std::fs::read_to_string(&path).unwrap();
        // The replacement must be reindented: new_string gains +4 spaces on
        // every non-blank line to match the file's indentation.
        assert_eq!(written, "    if x {\n        z();\n    }\n");
        for line in written.lines().filter(|l| !l.trim().is_empty()) {
            assert!(
                line.starts_with("    "),
                "line should have 4-space indent: {line:?}"
            );
        }
    }

    /// M-7 (#240): a whitespace-tier `replace_all=true` edit with MULTIPLE
    /// uniform-delta windows round-trips through `EditTool::invoke` and writes
    /// every site at the correct indentation, reporting one replacement per
    /// window. Both blocks share the same +4 delta (`old_string` is unindented),
    /// so the reindented replacement is identical and may be spliced into both.
    #[tokio::test]
    async fn replace_all_whitespace_tier_uniform_windows_reindents_all_sites() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("file.txt");
        // Two +4-indented copies of the same block, separated by a marker line.
        std::fs::write(
            &path,
            "    if x {\n        y();\n    }\nMID\n    if x {\n        y();\n    }\n",
        )
        .unwrap();

        let tool = EditTool::new(WorkspaceRoot::new(tmp.path()));
        let out = tool
            .invoke(
                json!({
                    "path": "file.txt",
                    "old_string": "if x {\n    y();\n}",
                    "new_string": "if x {\n    z();\n}",
                    "replace_all": true
                }),
                ctx(),
            )
            .await
            .unwrap();

        let ContentBlock::Text(t) = &out[0] else {
            panic!("expected Text block")
        };
        // Success count == number of windows (2).
        assert!(t.text.contains("2 replacements"), "output: {}", t.text);

        let written = std::fs::read_to_string(&path).unwrap();
        // Both sites reindented +4: every non-blank line keeps 4-space indent.
        assert_eq!(
            written,
            "    if x {\n        z();\n    }\nMID\n    if x {\n        z();\n    }\n"
        );
        for line in written
            .lines()
            .filter(|l| !l.trim().is_empty() && *l != "MID")
        {
            assert!(
                line.starts_with("    "),
                "line should keep 4-space indent: {line:?}"
            );
        }
    }

    /// A genuine miss (no exact or whitespace match) returns an error whose
    /// message is the near-miss diff, NOT the bare `old_string not found in file`.
    #[tokio::test]
    async fn true_miss_returns_near_miss_feedback_not_bare_message() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("file.txt");
        std::fs::write(&path, "fn alpha() {\n    do_thing();\n}\n").unwrap();

        let tool = EditTool::new(WorkspaceRoot::new(tmp.path()));
        // old_string is close but wrong — do_OTHER vs do_thing.
        let err = tool
            .invoke(
                json!({
                    "path": "file.txt",
                    "old_string": "fn alpha() {\n    do_OTHER();\n}",
                    "new_string": "fn alpha() {}"
                }),
                ctx(),
            )
            .await
            .unwrap_err();

        assert!(matches!(err, ToolError::Execution(_)));
        let msg = format!("{err}");
        // Must NOT be the bare not-found message.
        assert!(
            !msg.contains("old_string not found in file"),
            "should be near-miss feedback, not bare error: {msg}"
        );
        // Should contain diff markers from the near-miss render.
        assert!(
            msg.contains("- ") || msg.contains("+ "),
            "no diff in: {msg}"
        );
        assert!(
            msg.contains("do_OTHER") || msg.contains("do_thing"),
            "expected diff content in: {msg}"
        );
    }
}
