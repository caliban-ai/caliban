//! Glob tool — find files matching a pattern.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;

use async_trait::async_trait;
use caliban_agent_core::{Tool, ToolContext, ToolError};
use caliban_provider::{ContentBlock, TextBlock};
use globset::GlobBuilder;
use ignore::WalkBuilder;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::workspace::WorkspaceRoot;

const MAX_MATCHES: usize = 200;

/// Filesystem glob pattern matcher.
#[derive(Debug)]
pub struct GlobTool {
    root: Arc<WorkspaceRoot>,
    schema: OnceLock<Value>,
}

impl GlobTool {
    /// Construct a Glob tool using the given workspace root.
    #[must_use]
    pub fn new(root: WorkspaceRoot) -> Self {
        Self {
            root: Arc::new(root),
            schema: OnceLock::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct GlobInput {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
}

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &'static str {
        "Glob"
    }

    fn description(&self) -> &'static str {
        "Find files matching a glob pattern (e.g., '**/*.rs', 'src/**/*.py'). Respects .gitignore by default. Capped at 200 results."
    }

    fn input_schema(&self) -> &Value {
        self.schema.get_or_init(|| {
            json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern (e.g., '**/*.rs')"
                    },
                    "path": {
                        "type": "string",
                        "description": "Search root relative to workspace root (default: workspace root)"
                    }
                },
                "required": ["pattern"]
            })
        })
    }

    async fn invoke(&self, input: Value, _cx: ToolContext) -> Result<Vec<ContentBlock>, ToolError> {
        let parsed: GlobInput = serde_json::from_value(input)
            .map_err(|e| ToolError::invalid_input(format!("invalid input: {e}")))?;

        let search_root: PathBuf = match parsed.path {
            Some(ref p) => self.root.resolve(p)?,
            None => self.root.root().to_path_buf(),
        };

        let glob = GlobBuilder::new(&parsed.pattern)
            .literal_separator(true)
            .build()
            .map_err(|e| ToolError::invalid_input(format!("invalid glob pattern: {e}")))?
            .compile_matcher();

        let walk = WalkBuilder::new(&search_root)
            .hidden(true)
            .git_ignore(true)
            .build();

        let mut matches = Vec::new();
        let mut truncated = false;
        for entry in walk {
            let Ok(entry) = entry else {
                continue;
            };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            // Match against the path relative to search root, not absolute.
            let rel = entry
                .path()
                .strip_prefix(&search_root)
                .unwrap_or(entry.path());
            if glob.is_match(rel) {
                matches.push(self.root.relativize(entry.path()));
                if matches.len() >= MAX_MATCHES {
                    truncated = true;
                    break;
                }
            }
        }

        matches.sort();

        let suffix = if matches.len() == 1 { "" } else { "s" };
        let mut text = format!(
            "→ Glob '{}' matched {} file{}:\n",
            parsed.pattern,
            matches.len(),
            suffix
        );
        for m in &matches {
            writeln!(text, "  {}", m.display()).map_err(ToolError::execution)?;
        }
        if truncated {
            text.push_str("(truncated at 200 matches; refine pattern for more)\n");
        }

        Ok(vec![ContentBlock::Text(TextBlock {
            text,
            cache_control: None,
        })])
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    use super::*;

    fn ctx() -> ToolContext {
        ToolContext {
            tool_use_id: "t1".into(),
            cancel: CancellationToken::new(),
        }
    }

    #[tokio::test]
    async fn matches_simple_pattern() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "").unwrap();
        std::fs::write(tmp.path().join("b.rs"), "").unwrap();
        std::fs::write(tmp.path().join("c.txt"), "").unwrap();
        let tool = GlobTool::new(WorkspaceRoot::new(tmp.path()));
        let out = tool
            .invoke(json!({"pattern": "*.rs"}), ctx())
            .await
            .unwrap();
        let ContentBlock::Text(t) = &out[0] else {
            panic!()
        };
        assert!(t.text.contains("a.rs"));
        assert!(t.text.contains("b.rs"));
        assert!(!t.text.contains("c.txt"));
    }

    #[tokio::test]
    async fn matches_nested_pattern() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("nested/deep");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("found.rs"), "").unwrap();
        std::fs::write(tmp.path().join("root.rs"), "").unwrap();
        let tool = GlobTool::new(WorkspaceRoot::new(tmp.path()));
        let out = tool
            .invoke(json!({"pattern": "**/*.rs"}), ctx())
            .await
            .unwrap();
        let ContentBlock::Text(t) = &out[0] else {
            panic!()
        };
        assert!(t.text.contains("found.rs"));
        assert!(t.text.contains("root.rs"));
    }

    #[tokio::test]
    async fn gitignore_honored() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(".gitignore"), "ignored.rs\n").unwrap();
        std::fs::write(tmp.path().join("kept.rs"), "").unwrap();
        std::fs::write(tmp.path().join("ignored.rs"), "").unwrap();
        // Must be in a git repo for ignore::Walk to honor .gitignore.
        std::process::Command::new("git")
            .arg("init")
            .current_dir(tmp.path())
            .output()
            .ok();
        let tool = GlobTool::new(WorkspaceRoot::new(tmp.path()));
        let out = tool
            .invoke(json!({"pattern": "*.rs"}), ctx())
            .await
            .unwrap();
        let ContentBlock::Text(t) = &out[0] else {
            panic!()
        };
        assert!(t.text.contains("kept.rs"));
        // ignored.rs should NOT appear
        assert!(!t.text.contains("ignored.rs"));
    }

    #[tokio::test]
    async fn invalid_pattern_errors() {
        let tmp = TempDir::new().unwrap();
        let tool = GlobTool::new(WorkspaceRoot::new(tmp.path()));
        // globset is quite permissive; "[" without close-bracket is invalid.
        let err = tool
            .invoke(json!({"pattern": "["}), ctx())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }
}
