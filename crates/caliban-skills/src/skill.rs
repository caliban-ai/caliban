//! `Skill` struct — parsed frontmatter + markdown body.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

/// A loaded skill: YAML frontmatter + markdown body, plus its source path.
#[derive(Debug, Clone)]
pub struct Skill {
    /// `name:` from the frontmatter. Must match the parent directory name.
    pub name: String,
    /// `description:` from the frontmatter. Surfaced to the model in the
    /// `Skill` tool's description.
    pub description: String,
    /// Markdown body (everything after the closing `---`).
    pub body: String,
    /// Free-form `metadata:` map (passed through unchanged).
    pub metadata: BTreeMap<String, serde_yaml::Value>,
    /// Absolute path of the `SKILL.md` file this was loaded from.
    pub source_path: PathBuf,
}

/// Raw frontmatter shape used during parsing. Not exposed.
#[derive(Debug, Deserialize)]
pub(crate) struct Frontmatter {
    pub(crate) name: String,
    pub(crate) description: String,
    #[serde(default)]
    pub(crate) metadata: BTreeMap<String, serde_yaml::Value>,
}
