//! Built-in tools for reading and writing per-project auto-memory.
//!
//! See `docs/superpowers/specs/2026-05-24-auto-memory-design.md` and
//! `adrs/0035-auto-memory.md`. Both tools are sandboxed to the
//! `auto_memory_dir` resolved at construction time — they never touch paths
//! outside it.

use std::sync::Arc;
use std::sync::OnceLock;

use async_trait::async_trait;
use caliban_agent_core::{Tool, ToolContext, ToolError};
use caliban_memory::{TopicDraft, TopicKind, TopicLoader};
use caliban_provider::{ContentBlock, TextBlock};
use serde::Deserialize;
use serde_json::{Value, json};

/// `ReadMemoryTopic` — read a topic file by slug. Sandboxed to the loader's
/// memory directory.
#[derive(Debug)]
pub struct ReadMemoryTopicTool {
    loader: Arc<TopicLoader>,
    schema: OnceLock<Value>,
}

impl ReadMemoryTopicTool {
    /// Construct a `ReadMemoryTopic` tool backed by the given loader.
    #[must_use]
    pub fn new(loader: Arc<TopicLoader>) -> Self {
        Self {
            loader,
            schema: OnceLock::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ReadInput {
    name: String,
}

#[async_trait]
impl Tool for ReadMemoryTopicTool {
    fn name(&self) -> &'static str {
        "ReadMemoryTopic"
    }

    fn description(&self) -> &'static str {
        "Read one auto-memory topic file by slug. The slug is the value in the `MEMORY.md` index entry (without `.md`). Returns the topic's markdown body."
    }

    fn input_schema(&self) -> &Value {
        self.schema.get_or_init(|| {
            json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Topic slug (kebab-case, no path separators, no leading '.')."
                    }
                },
                "required": ["name"]
            })
        })
    }

    async fn invoke(&self, input: Value, _cx: ToolContext) -> Result<Vec<ContentBlock>, ToolError> {
        let parsed: ReadInput = serde_json::from_value(input)
            .map_err(|e| ToolError::invalid_input(format!("invalid input: {e}")))?;
        let topic = self.loader.read(&parsed.name).map_err(|e| match e {
            caliban_memory::MemoryError::InvalidSlug { .. } => {
                ToolError::invalid_input(e.to_string())
            }
            other => ToolError::execution(other),
        })?;
        let text = format!(
            "→ Memory topic '{}' ({}): {}\n\n{}",
            topic.name,
            topic.kind.as_str(),
            topic.description,
            topic.body
        );
        Ok(vec![ContentBlock::Text(TextBlock {
            text,
            cache_control: None,
        })])
    }
}

/// `WriteMemoryTopic` — atomically write a topic file *and* update the
/// `MEMORY.md` index entry for it.
#[derive(Debug)]
pub struct WriteMemoryTopicTool {
    loader: Arc<TopicLoader>,
    schema: OnceLock<Value>,
}

impl WriteMemoryTopicTool {
    /// Construct a `WriteMemoryTopic` tool backed by the given loader.
    #[must_use]
    pub fn new(loader: Arc<TopicLoader>) -> Self {
        Self {
            loader,
            schema: OnceLock::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct WriteInput {
    name: String,
    description: String,
    #[serde(rename = "type")]
    kind: String,
    body: String,
}

#[async_trait]
impl Tool for WriteMemoryTopicTool {
    fn name(&self) -> &'static str {
        "WriteMemoryTopic"
    }

    fn description(&self) -> &'static str {
        "Write or update an auto-memory topic file. Atomic: writes the topic file AND updates the MEMORY.md index entry in a single call. `type` must be one of: user, feedback, project, reference."
    }

    fn input_schema(&self) -> &Value {
        self.schema.get_or_init(|| {
            json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Topic slug (kebab-case, no path separators, no leading '.')."
                    },
                    "description": {
                        "type": "string",
                        "description": "One-line summary (≤120 chars). Surfaces into the MEMORY.md index entry."
                    },
                    "type": {
                        "type": "string",
                        "enum": ["user", "feedback", "project", "reference"],
                        "description": "Memory type. user=facts about the user, feedback=durable rules/preferences, project=durable project facts, reference=stable external IDs."
                    },
                    "body": {
                        "type": "string",
                        "description": "Markdown body. Use [[other-slug]] to cross-reference siblings (purely informational)."
                    }
                },
                "required": ["name", "description", "type", "body"]
            })
        })
    }

    async fn invoke(&self, input: Value, _cx: ToolContext) -> Result<Vec<ContentBlock>, ToolError> {
        let parsed: WriteInput = serde_json::from_value(input)
            .map_err(|e| ToolError::invalid_input(format!("invalid input: {e}")))?;
        let kind = TopicKind::parse(&parsed.kind).ok_or_else(|| {
            ToolError::invalid_input(format!(
                "type must be one of user|feedback|project|reference (got '{}')",
                parsed.kind
            ))
        })?;
        let draft = TopicDraft {
            name: parsed.name,
            description: parsed.description,
            kind,
            body: parsed.body,
        };
        let path = self.loader.write(&draft).map_err(|e| match e {
            caliban_memory::MemoryError::InvalidSlug { .. } => {
                ToolError::invalid_input(e.to_string())
            }
            other => ToolError::execution(other),
        })?;
        Ok(vec![ContentBlock::Text(TextBlock {
            text: format!(
                "→ Wrote memory topic '{}' to {} and updated MEMORY.md index",
                draft.name,
                path.display(),
            ),
            cache_control: None,
        })])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caliban_memory::TopicLoader;
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

    fn loader(dir: &std::path::Path) -> Arc<TopicLoader> {
        Arc::new(TopicLoader::new(dir.to_path_buf()))
    }

    #[tokio::test]
    async fn read_returns_body_content() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("foo.md"),
            "---\nname: foo\ndescription: \"d\"\nmetadata:\n  type: user\n---\n\nThe body text.\n",
        )
        .unwrap();
        let tool = ReadMemoryTopicTool::new(loader(tmp.path()));
        let out = tool.invoke(json!({"name": "foo"}), ctx()).await.unwrap();
        let ContentBlock::Text(t) = &out[0] else {
            panic!()
        };
        assert!(t.text.contains("The body text."));
        assert!(t.text.contains("foo"));
        assert!(t.text.contains("(user)"));
    }

    #[tokio::test]
    async fn write_creates_file_and_updates_index() {
        let tmp = TempDir::new().unwrap();
        let tool = WriteMemoryTopicTool::new(loader(tmp.path()));
        tool.invoke(
            json!({
                "name": "personal-email",
                "description": "use personal email for ~/dev/personal/**",
                "type": "feedback",
                "body": "Use john.ford2002@gmail.com.\n"
            }),
            ctx(),
        )
        .await
        .unwrap();
        let topic_path = tmp.path().join("personal-email.md");
        assert!(topic_path.exists());
        // tmp file must not linger
        assert!(!tmp.path().join("personal-email.md.tmp").exists());
        let index = std::fs::read_to_string(tmp.path().join("MEMORY.md")).unwrap();
        assert!(index.contains("[personal-email](personal-email.md)"));
    }

    #[tokio::test]
    async fn write_rejects_invalid_type() {
        let tmp = TempDir::new().unwrap();
        let tool = WriteMemoryTopicTool::new(loader(tmp.path()));
        let err = tool
            .invoke(
                json!({
                    "name": "bad",
                    "description": "d",
                    "type": "junk",
                    "body": "x"
                }),
                ctx(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn write_rejects_traversal_slug() {
        let tmp = TempDir::new().unwrap();
        let tool = WriteMemoryTopicTool::new(loader(tmp.path()));
        let err = tool
            .invoke(
                json!({
                    "name": "../escape",
                    "description": "d",
                    "type": "user",
                    "body": "x"
                }),
                ctx(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }
}
