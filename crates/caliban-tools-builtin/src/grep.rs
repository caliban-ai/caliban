//! Grep tool — ripgrep-library-based content search.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;

use async_trait::async_trait;
use caliban_agent_core::{Tool, ToolContext, ToolError};
use caliban_provider::{ContentBlock, TextBlock};
use globset::Glob;
use grep_regex::RegexMatcher;
use grep_searcher::{Searcher, Sink, SinkMatch};
use ignore::WalkBuilder;
use serde::Deserialize;
use serde_json::{Value, json};
use std::fmt::Write;

use crate::workspace::WorkspaceRoot;

const DEFAULT_MAX_MATCHES: usize = 100;
const MAX_MAX_MATCHES: usize = 500;

/// Ripgrep-library-based content search tool.
#[derive(Debug)]
pub struct GrepTool {
    root: Arc<WorkspaceRoot>,
    schema: OnceLock<Value>,
}

impl GrepTool {
    /// Construct a Grep tool using the given workspace root.
    #[must_use]
    pub fn new(root: WorkspaceRoot) -> Self {
        Self {
            root: Arc::new(root),
            schema: OnceLock::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct GrepInput {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    include: Option<String>,
    #[serde(default)]
    max_matches: Option<usize>,
}

struct CollectingSink<'a> {
    path: &'a std::path::Path,
    workspace_root: &'a WorkspaceRoot,
    results: &'a mut Vec<String>,
    max: usize,
}

impl Sink for CollectingSink<'_> {
    type Error = std::io::Error;

    fn matched(
        &mut self,
        _searcher: &Searcher,
        mat: &SinkMatch<'_>,
    ) -> Result<bool, std::io::Error> {
        if self.results.len() >= self.max {
            return Ok(false);
        }
        let line_num = mat.line_number().unwrap_or(0);
        let line_text = String::from_utf8_lossy(mat.bytes())
            .trim_end_matches('\n')
            .to_string();
        let rel = self.workspace_root.relativize(self.path);
        self.results
            .push(format!("{}:{}:{}", rel.display(), line_num, line_text));
        Ok(true)
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &'static str {
        "Grep"
    }

    fn description(&self) -> &'static str {
        "Search file contents using a regex pattern. Respects .gitignore by default. Returns matches in {path}:{line}:{text} format, capped at 100 matches by default (max 500)."
    }

    fn input_schema(&self) -> &Value {
        self.schema.get_or_init(|| json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Regex pattern to search for" },
                "path": { "type": "string", "description": "Search root (default: workspace root)" },
                "include": { "type": "string", "description": "Glob filter for files to search (e.g., '*.rs')" },
                "max_matches": { "type": "integer", "description": "Maximum matches to return (default 100, max 500)", "minimum": 1, "maximum": 500 }
            },
            "required": ["pattern"]
        }))
    }

    async fn invoke(&self, input: Value, _cx: ToolContext) -> Result<Vec<ContentBlock>, ToolError> {
        let parsed: GrepInput = serde_json::from_value(input)
            .map_err(|e| ToolError::invalid_input(format!("invalid input: {e}")))?;

        let search_root: PathBuf = match parsed.path {
            Some(p) => self.root.resolve(&p)?,
            None => self.root.root().to_path_buf(),
        };

        let max_matches = parsed
            .max_matches
            .unwrap_or(DEFAULT_MAX_MATCHES)
            .min(MAX_MAX_MATCHES);

        let matcher = RegexMatcher::new(&parsed.pattern)
            .map_err(|e| ToolError::invalid_input(format!("invalid regex: {e}")))?;

        let include_glob = match parsed.include.as_ref() {
            Some(g) => Some(
                Glob::new(g)
                    .map_err(|e| ToolError::invalid_input(format!("invalid include glob: {e}")))?
                    .compile_matcher(),
            ),
            None => None,
        };

        let walk = WalkBuilder::new(&search_root)
            .hidden(true)
            .git_ignore(true)
            .build();

        let mut results: Vec<String> = Vec::with_capacity(max_matches);
        let mut truncated = false;

        let workspace_root = &*self.root;
        for entry in walk {
            if results.len() >= max_matches {
                truncated = true;
                break;
            }
            let Ok(entry) = entry else { continue };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            if let Some(glob) = &include_glob {
                let name = entry.path().file_name().unwrap_or_default();
                if !glob.is_match(std::path::Path::new(name)) {
                    continue;
                }
            }
            let mut searcher = grep_searcher::SearcherBuilder::new()
                .line_number(true)
                .build();
            let mut sink = CollectingSink {
                path: entry.path(),
                workspace_root,
                results: &mut results,
                max: max_matches,
            };
            // Errors searching a single file are non-fatal — log and continue.
            let _ = searcher.search_path(&matcher, entry.path(), &mut sink);
        }

        if results.is_empty() {
            return Ok(vec![ContentBlock::Text(TextBlock {
                text: format!("→ Grep '{}': no matches", parsed.pattern),
                cache_control: None,
            })]);
        }

        let mut text = format!(
            "→ Grep '{}': {} match{}\n",
            parsed.pattern,
            results.len(),
            if results.len() == 1 { "" } else { "es" }
        );
        for line in &results {
            writeln!(text, "{line}").map_err(ToolError::execution)?;
        }
        if truncated {
            writeln!(
                text,
                "(truncated at {max_matches} matches; raise max_matches for more)"
            )
            .map_err(ToolError::execution)?;
        }

        Ok(vec![ContentBlock::Text(TextBlock {
            text,
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
        }
    }

    #[tokio::test]
    async fn finds_pattern_in_file() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "fn hello() {}\nfn bye() {}\n").unwrap();
        let tool = GrepTool::new(WorkspaceRoot::new(tmp.path()));
        let out = tool
            .invoke(json!({"pattern": "hello"}), ctx())
            .await
            .unwrap();
        let ContentBlock::Text(t) = &out[0] else {
            panic!()
        };
        assert!(t.text.contains("a.rs:1:fn hello()"));
        assert!(!t.text.contains("bye"));
    }

    #[tokio::test]
    async fn multiple_files_searched() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "target_pattern here\n").unwrap();
        std::fs::write(tmp.path().join("b.rs"), "target_pattern there\n").unwrap();
        let tool = GrepTool::new(WorkspaceRoot::new(tmp.path()));
        let out = tool
            .invoke(json!({"pattern": "target_pattern"}), ctx())
            .await
            .unwrap();
        let ContentBlock::Text(t) = &out[0] else {
            panic!()
        };
        assert!(t.text.contains("a.rs:"));
        assert!(t.text.contains("b.rs:"));
    }

    #[tokio::test]
    async fn include_filter_applied() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "target_pattern\n").unwrap();
        std::fs::write(tmp.path().join("b.py"), "target_pattern\n").unwrap();
        let tool = GrepTool::new(WorkspaceRoot::new(tmp.path()));
        let out = tool
            .invoke(
                json!({"pattern": "target_pattern", "include": "*.rs"}),
                ctx(),
            )
            .await
            .unwrap();
        let ContentBlock::Text(t) = &out[0] else {
            panic!()
        };
        assert!(t.text.contains("a.rs"));
        assert!(!t.text.contains("b.py"));
    }

    #[tokio::test]
    async fn no_matches_returns_friendly_message() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "nothing relevant\n").unwrap();
        let tool = GrepTool::new(WorkspaceRoot::new(tmp.path()));
        let out = tool
            .invoke(json!({"pattern": "absent_pattern"}), ctx())
            .await
            .unwrap();
        let ContentBlock::Text(t) = &out[0] else {
            panic!()
        };
        assert!(t.text.contains("no matches"));
    }

    #[tokio::test]
    async fn invalid_regex_errors() {
        let tmp = TempDir::new().unwrap();
        let tool = GrepTool::new(WorkspaceRoot::new(tmp.path()));
        let err = tool
            .invoke(json!({"pattern": "[unclosed"}), ctx())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }
}
