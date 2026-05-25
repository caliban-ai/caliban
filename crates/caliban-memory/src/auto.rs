//! Per-project auto-memory: topic file enumerator, reader, and writer.
//!
//! See `docs/superpowers/specs/2026-05-24-auto-memory-design.md` and
//! `adrs/0035-auto-memory.md`.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{MemoryError, Result};

/// The four memory-type categories the model classifies a topic file under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopicKind {
    /// Durable facts about the user.
    User,
    /// User-issued corrections / preferences for future interactions.
    Feedback,
    /// Durable project facts not already in the repo.
    Project,
    /// Stable external context (account IDs, URLs, API quotas).
    Reference,
}

impl TopicKind {
    /// Lower-case wire form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Feedback => "feedback",
            Self::Project => "project",
            Self::Reference => "reference",
        }
    }

    /// Parse from a string, accepting case-insensitively. Returns `None` for
    /// any input that is not one of the four valid types.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "user" => Some(Self::User),
            "feedback" => Some(Self::Feedback),
            "project" => Some(Self::Project),
            "reference" => Some(Self::Reference),
            _ => None,
        }
    }
}

/// Lightweight summary of a topic file (frontmatter only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicSummary {
    /// The slug (kebab-case, must match filename stem).
    pub name: String,
    /// One-line description (≤ 120 chars by convention).
    pub description: String,
    /// Memory type classification.
    pub kind: TopicKind,
    /// Absolute path to the topic file.
    pub path: PathBuf,
}

/// A fully-loaded topic file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicFile {
    /// The slug (kebab-case).
    pub name: String,
    /// One-line description.
    pub description: String,
    /// Memory type classification.
    pub kind: TopicKind,
    /// Markdown body (everything after the closing frontmatter `---`).
    pub body: String,
    /// Absolute path.
    pub path: PathBuf,
}

/// Draft passed to [`TopicLoader::write`]. The loader fills in path / on-disk
/// frontmatter from these fields.
#[derive(Debug, Clone)]
pub struct TopicDraft {
    /// The slug (kebab-case). Must pass [`validate_slug`].
    pub name: String,
    /// One-line description for the `MEMORY.md` index entry + frontmatter.
    pub description: String,
    /// Memory type classification.
    pub kind: TopicKind,
    /// Raw markdown body (no frontmatter — the loader emits it).
    pub body: String,
}

/// Frontmatter shape used by [`TopicLoader::read`] / [`TopicLoader::list`].
#[derive(Debug, Deserialize)]
struct RawFrontmatter {
    name: String,
    description: String,
    #[serde(default)]
    metadata: RawMetadata,
}

#[derive(Debug, Default, Deserialize)]
struct RawMetadata {
    #[serde(rename = "type")]
    kind: Option<String>,
}

/// Enumerator + reader/writer for topic `.md` files under a single memory
/// directory.
#[derive(Debug, Clone)]
pub struct TopicLoader {
    dir: PathBuf,
}

impl TopicLoader {
    /// Construct a loader over the given memory directory. The directory does
    /// not have to exist yet — `list` returns an empty vec, and `write` will
    /// create it on demand.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The directory this loader manages.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Enumerate every `.md` sibling of `MEMORY.md`, parsing frontmatter for
    /// each. Files with malformed frontmatter are silently skipped with a
    /// `warn!` log entry (rationale: a single corrupted topic file should not
    /// brick the whole memory tier).
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::Io`] if the directory exists but cannot be read.
    pub fn list(&self) -> Result<Vec<TopicSummary>> {
        let mut out = Vec::new();
        if !self.dir.exists() {
            return Ok(out);
        }
        let entries = std::fs::read_dir(&self.dir).map_err(|source| MemoryError::Io {
            path: self.dir.clone(),
            source,
        })?;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            // Skip the index file itself.
            if path.file_name().and_then(|s| s.to_str()) == Some("MEMORY.md") {
                continue;
            }
            match Self::read_summary(&path) {
                Ok(mut summary) => {
                    if summary.name != stem {
                        tracing::warn!(
                            target: caliban_common::tracing_targets::TARGET_MEMORY_AUTO,
                            path = %path.display(),
                            frontmatter_name = %summary.name,
                            file_stem = %stem,
                            "topic frontmatter name does not match filename; using filename",
                        );
                        summary.name = stem.to_string();
                    }
                    out.push(summary);
                }
                Err(e) => {
                    tracing::warn!(
                        target: caliban_common::tracing_targets::TARGET_MEMORY_AUTO,
                        path = %path.display(),
                        error = %e,
                        "skipping malformed topic file",
                    );
                }
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// Read a topic by slug. The slug must pass [`validate_slug`] — no path
    /// separators, no `..`, no leading `.`.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::InvalidSlug`] for traversal / illegal slugs,
    /// [`MemoryError::Io`] if the file does not exist or cannot be read, and
    /// [`MemoryError::InvalidTopic`] if the frontmatter is malformed.
    pub fn read(&self, name: &str) -> Result<TopicFile> {
        validate_slug(name)?;
        let path = self.dir.join(format!("{name}.md"));
        let raw = std::fs::read_to_string(&path).map_err(|source| MemoryError::Io {
            path: path.clone(),
            source,
        })?;
        let (fm, body) = parse_frontmatter(&raw, &path)?;
        let kind =
            TopicKind::parse(fm.metadata.kind.as_deref().unwrap_or("")).ok_or_else(|| {
                MemoryError::InvalidTopic {
                    path: path.clone(),
                    reason: format!(
                        "metadata.type must be one of user|feedback|project|reference (got {:?})",
                        fm.metadata.kind
                    ),
                }
            })?;
        Ok(TopicFile {
            name: fm.name,
            description: fm.description,
            kind,
            body: body.to_string(),
            path,
        })
    }

    /// Atomically write a topic file (`<slug>.md`) and update the `MEMORY.md`
    /// index line for it. Returns the topic's absolute path on success.
    ///
    /// Write semantics:
    /// 1. Write the topic body + frontmatter to `<slug>.md.tmp`.
    /// 2. Rename to `<slug>.md` (atomic on the same filesystem).
    /// 3. Rewrite `MEMORY.md` with an updated index line for the slug
    ///    (`MEMORY.md` is rewritten via the same tmp+rename dance).
    ///
    /// A crash between (2) and (3) leaves an orphan topic file that
    /// `rebuild-index` can re-detect.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::InvalidSlug`] for bad slugs, [`MemoryError::Io`]
    /// on any IO failure.
    pub fn write(&self, draft: &TopicDraft) -> Result<PathBuf> {
        validate_slug(&draft.name)?;
        std::fs::create_dir_all(&self.dir).map_err(|source| MemoryError::Io {
            path: self.dir.clone(),
            source,
        })?;

        let path = self.dir.join(format!("{}.md", draft.name));
        let serialized = render_topic_file(draft);
        caliban_common::fs::write_atomic(&path, serialized.as_bytes()).map_err(|source| {
            MemoryError::Io {
                path: path.clone(),
                source,
            }
        })?;

        update_index_line(&self.dir, draft)?;
        Ok(path)
    }

    /// Delete a topic file by slug and remove its `MEMORY.md` index line.
    /// Missing files are treated as success (idempotent delete).
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::InvalidSlug`] for bad slugs or [`MemoryError::Io`]
    /// on IO failure.
    pub fn delete(&self, name: &str) -> Result<()> {
        validate_slug(name)?;
        let path = self.dir.join(format!("{name}.md"));
        match std::fs::remove_file(&path) {
            Ok(()) | Err(_) if !path.exists() => {}
            Err(e) => {
                return Err(MemoryError::Io {
                    path: path.clone(),
                    source: e,
                });
            }
            Ok(()) => {}
        }
        remove_index_line(&self.dir, name)?;
        Ok(())
    }

    fn read_summary(path: &Path) -> Result<TopicSummary> {
        let raw = std::fs::read_to_string(path).map_err(|source| MemoryError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let (fm, _) = parse_frontmatter(&raw, path)?;
        let kind =
            TopicKind::parse(fm.metadata.kind.as_deref().unwrap_or("")).ok_or_else(|| {
                MemoryError::InvalidTopic {
                    path: path.to_path_buf(),
                    reason: format!(
                        "metadata.type must be one of user|feedback|project|reference (got {:?})",
                        fm.metadata.kind
                    ),
                }
            })?;
        Ok(TopicSummary {
            name: fm.name,
            description: fm.description,
            kind,
            path: path.to_path_buf(),
        })
    }
}

/// Validate a topic slug. Rules: non-empty, no path separators (`/`, `\\`),
/// no `..`, no leading dot.
///
/// # Errors
///
/// Returns [`MemoryError::InvalidSlug`] if the slug fails any rule.
pub fn validate_slug(slug: &str) -> Result<()> {
    if slug.is_empty() {
        return Err(MemoryError::InvalidSlug {
            slug: slug.to_string(),
            reason: "slug must be non-empty".into(),
        });
    }
    if slug.contains('/') || slug.contains('\\') {
        return Err(MemoryError::InvalidSlug {
            slug: slug.to_string(),
            reason: "slug must not contain path separators".into(),
        });
    }
    if slug.contains("..") {
        return Err(MemoryError::InvalidSlug {
            slug: slug.to_string(),
            reason: "slug must not contain '..'".into(),
        });
    }
    if slug.starts_with('.') {
        return Err(MemoryError::InvalidSlug {
            slug: slug.to_string(),
            reason: "slug must not start with '.'".into(),
        });
    }
    if slug.contains('\0') {
        return Err(MemoryError::InvalidSlug {
            slug: slug.to_string(),
            reason: "slug must not contain NUL".into(),
        });
    }
    Ok(())
}

/// Split a raw file into frontmatter struct + body. Frontmatter delimiters are
/// `---\n` opening and `\n---\n` (or `\n---` at EOF) closing.
fn parse_frontmatter<'a>(raw: &'a str, path: &Path) -> Result<(RawFrontmatter, &'a str)> {
    let trimmed = raw.trim_start_matches('\u{feff}');
    let body_start = "---\n";
    if !trimmed.starts_with(body_start) {
        return Err(MemoryError::InvalidTopic {
            path: path.to_path_buf(),
            reason: "missing leading `---` frontmatter delimiter".into(),
        });
    }
    let after_start = &trimmed[body_start.len()..];
    let Some(end_idx) = after_start.find("\n---\n").or_else(|| {
        after_start
            .find("\n---")
            .filter(|i| after_start[*i..].starts_with("\n---"))
    }) else {
        return Err(MemoryError::InvalidTopic {
            path: path.to_path_buf(),
            reason: "missing closing `---` frontmatter delimiter".into(),
        });
    };
    let yaml_chunk = &after_start[..end_idx];
    let body_start_offset = end_idx + "\n---\n".len();
    let body = if body_start_offset >= after_start.len() {
        ""
    } else {
        &after_start[body_start_offset..]
    };
    let fm: RawFrontmatter =
        serde_yaml::from_str(yaml_chunk).map_err(|e| MemoryError::InvalidTopic {
            path: path.to_path_buf(),
            reason: format!("yaml: {e}"),
        })?;
    if fm.name.trim().is_empty() {
        return Err(MemoryError::InvalidTopic {
            path: path.to_path_buf(),
            reason: "name must be non-empty".into(),
        });
    }
    if fm.description.trim().is_empty() {
        return Err(MemoryError::InvalidTopic {
            path: path.to_path_buf(),
            reason: "description must be non-empty".into(),
        });
    }
    Ok((fm, body))
}

/// Render a [`TopicDraft`] to on-disk markdown (frontmatter + body).
fn render_topic_file(draft: &TopicDraft) -> String {
    let mut out = String::with_capacity(draft.body.len() + 256);
    out.push_str("---\n");
    out.push_str("name: ");
    out.push_str(&draft.name);
    out.push('\n');
    out.push_str("description: \"");
    out.push_str(&escape_yaml_string(&draft.description));
    out.push_str("\"\n");
    out.push_str("metadata:\n");
    out.push_str("  node_type: memory\n");
    out.push_str("  type: ");
    out.push_str(draft.kind.as_str());
    out.push('\n');
    out.push_str("---\n\n");
    out.push_str(&draft.body);
    if !draft.body.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Escape `"` and `\` for a double-quoted YAML scalar.
fn escape_yaml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

/// Update (or insert) the index line for `draft.name` inside `MEMORY.md`.
/// Atomic via tmp + rename.
fn update_index_line(dir: &Path, draft: &TopicDraft) -> Result<()> {
    let index_path = dir.join("MEMORY.md");
    let existing = match std::fs::read_to_string(&index_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(MemoryError::Io {
                path: index_path.clone(),
                source,
            });
        }
    };

    let new_line = format!(
        "- [{title}]({slug}.md) — {kind}: {desc}",
        title = draft.name,
        slug = draft.name,
        kind = draft.kind.as_str(),
        desc = draft.description.lines().next().unwrap_or("").trim(),
    );

    let new_body = rewrite_with_index_line(&existing, &draft.name, &new_line);
    caliban_common::fs::write_atomic(&index_path, new_body.as_bytes()).map_err(|source| {
        MemoryError::Io {
            path: index_path.clone(),
            source,
        }
    })?;
    Ok(())
}

/// Remove a topic's index line, if present.
fn remove_index_line(dir: &Path, slug: &str) -> Result<()> {
    let index_path = dir.join("MEMORY.md");
    let existing = match std::fs::read_to_string(&index_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(MemoryError::Io {
                path: index_path.clone(),
                source,
            });
        }
    };
    let needle = format!("]({slug}.md)");
    let kept: Vec<&str> = existing.lines().filter(|l| !l.contains(&needle)).collect();
    let mut new_body = kept.join("\n");
    if existing.ends_with('\n') && !new_body.ends_with('\n') {
        new_body.push('\n');
    }
    caliban_common::fs::write_atomic(&index_path, new_body.as_bytes()).map_err(|source| {
        MemoryError::Io {
            path: index_path.clone(),
            source,
        }
    })?;
    Ok(())
}

/// Insert-or-replace the index line for `slug`. We match on the
/// `](<slug>.md)` substring, which is robust against operators tweaking the
/// title or one-line summary in place.
fn rewrite_with_index_line(existing: &str, slug: &str, new_line: &str) -> String {
    if existing.is_empty() {
        let mut s = String::from("# Memory index\n\n");
        s.push_str(new_line);
        s.push('\n');
        return s;
    }
    let needle = format!("]({slug}.md)");
    let mut replaced = false;
    let mut out_lines: Vec<String> = Vec::with_capacity(existing.lines().count() + 1);
    for line in existing.lines() {
        if !replaced && line.contains(&needle) {
            out_lines.push(new_line.to_string());
            replaced = true;
        } else {
            out_lines.push(line.to_string());
        }
    }
    if !replaced {
        // Append after the last existing index-style line, otherwise at EOF.
        let mut insert_idx = out_lines.len();
        // Insert after the last `- [` bullet line if any exist.
        for (i, line) in out_lines.iter().enumerate().rev() {
            if line.trim_start().starts_with("- [") {
                insert_idx = i + 1;
                break;
            }
        }
        out_lines.insert(insert_idx, new_line.to_string());
    }
    let mut s = out_lines.join("\n");
    if existing.ends_with('\n') || !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// Strip every `<!-- … -->` HTML comment (greedy, multi-line) from `body`.
/// Used by the memory loader before splicing into the system prompt — the
/// on-disk file is untouched.
#[must_use]
pub fn strip_html_comments(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 3 < bytes.len() && &bytes[i..i + 4] == b"<!--" {
            // find closing -->; if not found, drop the rest.
            if let Some(end) = find_subslice(&bytes[i + 4..], b"-->") {
                i += 4 + end + 3;
                continue;
            }
            break;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    for i in 0..=hay.len() - needle.len() {
        if &hay[i..i + needle.len()] == needle {
            return Some(i);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn topic_md(name: &str, kind: &str, desc: &str, body: &str) -> String {
        format!(
            "---\nname: {name}\ndescription: \"{desc}\"\nmetadata:\n  node_type: memory\n  type: {kind}\n---\n\n{body}\n",
        )
    }

    #[test]
    fn list_enumerates_topic_files_excluding_memory_md() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        std::fs::write(
            dir.join("MEMORY.md"),
            "# Memory index\n\n- [foo](foo.md) — user: foo\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("foo.md"),
            topic_md("foo", "user", "foo desc", "body"),
        )
        .unwrap();
        std::fs::write(
            dir.join("bar.md"),
            topic_md("bar", "feedback", "bar desc", "body"),
        )
        .unwrap();

        let loader = TopicLoader::new(dir.to_path_buf());
        let topics = loader.list().unwrap();
        let names: Vec<_> = topics.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["bar", "foo"]);
        assert!(topics.iter().any(|t| matches!(t.kind, TopicKind::User)));
        assert!(topics.iter().any(|t| matches!(t.kind, TopicKind::Feedback)));
    }

    #[test]
    fn read_round_trips_a_topic() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        std::fs::write(
            dir.join("user-role.md"),
            topic_md(
                "user-role",
                "user",
                "role + context",
                "# User role\n\nSenior engineer.\n",
            ),
        )
        .unwrap();

        let loader = TopicLoader::new(dir.to_path_buf());
        let topic = loader.read("user-role").unwrap();
        assert_eq!(topic.name, "user-role");
        assert_eq!(topic.kind, TopicKind::User);
        assert!(topic.body.contains("Senior engineer."));
    }

    #[test]
    fn write_creates_topic_and_updates_index() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("MEMORY.md"), "# Memory index\n\n").unwrap();
        let loader = TopicLoader::new(dir.to_path_buf());
        let path = loader
            .write(&TopicDraft {
                name: "personal-email".to_string(),
                description: "use personal email for ~/dev/personal/**".to_string(),
                kind: TopicKind::Feedback,
                body: "Use john.ford2002@gmail.com.\n".to_string(),
            })
            .unwrap();
        assert!(path.exists());
        assert!(!dir.join("personal-email.md.tmp").exists());

        // Topic file contains frontmatter + body.
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("name: personal-email"));
        assert!(written.contains("type: feedback"));
        assert!(written.contains("john.ford2002@gmail.com"));

        // Index updated.
        let index = std::fs::read_to_string(dir.join("MEMORY.md")).unwrap();
        assert!(index.contains("[personal-email](personal-email.md)"));
        assert!(index.contains("feedback:"));
    }

    #[test]
    fn write_updates_existing_index_line_in_place() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        std::fs::write(
            dir.join("MEMORY.md"),
            "# Memory index\n\n- [foo](foo.md) — user: old desc\n",
        )
        .unwrap();
        let loader = TopicLoader::new(dir.to_path_buf());
        loader
            .write(&TopicDraft {
                name: "foo".to_string(),
                description: "new desc".to_string(),
                kind: TopicKind::User,
                body: "body".to_string(),
            })
            .unwrap();
        let index = std::fs::read_to_string(dir.join("MEMORY.md")).unwrap();
        // exactly one entry for foo
        assert_eq!(index.matches("[foo](foo.md)").count(), 1);
        assert!(index.contains("new desc"));
        assert!(!index.contains("old desc"));
    }

    #[test]
    fn read_rejects_invalid_type_in_frontmatter() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("bad.md"), topic_md("bad", "junk", "desc", "body")).unwrap();
        let loader = TopicLoader::new(dir.to_path_buf());
        let err = loader.read("bad").unwrap_err();
        assert!(matches!(err, MemoryError::InvalidTopic { .. }));
    }

    #[test]
    fn read_rejects_missing_required_frontmatter_fields() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        std::fs::write(
            dir.join("incomplete.md"),
            "---\ndescription: \"no name\"\nmetadata:\n  type: user\n---\n\nbody\n",
        )
        .unwrap();
        let loader = TopicLoader::new(dir.to_path_buf());
        let err = loader.read("incomplete").unwrap_err();
        assert!(matches!(err, MemoryError::InvalidTopic { .. }));
    }

    #[test]
    fn cross_reference_brackets_preserved_in_body() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let body = "Crosslinks: [[parity-gap-matrix]], [[sprint-mode]].\n".to_string();
        let loader = TopicLoader::new(dir.to_path_buf());
        loader
            .write(&TopicDraft {
                name: "user-role".to_string(),
                description: "role".to_string(),
                kind: TopicKind::User,
                body: body.clone(),
            })
            .unwrap();
        let topic = loader.read("user-role").unwrap();
        assert!(topic.body.contains("[[parity-gap-matrix]]"));
        assert!(topic.body.contains("[[sprint-mode]]"));
    }

    #[test]
    fn validate_slug_rejects_path_traversal() {
        assert!(validate_slug("ok").is_ok());
        assert!(validate_slug("ok-slug_1").is_ok());
        assert!(validate_slug("").is_err());
        assert!(validate_slug("a/b").is_err());
        assert!(validate_slug("a\\b").is_err());
        assert!(validate_slug("..").is_err());
        assert!(validate_slug("a..b").is_err());
        assert!(validate_slug(".hidden").is_err());
    }

    #[test]
    fn strip_html_comments_handles_single_and_multiline() {
        let single = "hello <!-- inline --> world";
        assert_eq!(strip_html_comments(single), "hello  world");

        let multi = "before\n<!-- line one\nline two\n-->\nafter";
        let stripped = strip_html_comments(multi);
        assert!(stripped.contains("before"));
        assert!(stripped.contains("after"));
        assert!(!stripped.contains("line one"));
        assert!(!stripped.contains("line two"));
    }

    #[test]
    fn delete_removes_file_and_index_line() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let loader = TopicLoader::new(dir.to_path_buf());
        loader
            .write(&TopicDraft {
                name: "tmp-topic".to_string(),
                description: "tmp".to_string(),
                kind: TopicKind::Project,
                body: "body".to_string(),
            })
            .unwrap();
        loader.delete("tmp-topic").unwrap();
        assert!(!dir.join("tmp-topic.md").exists());
        let index = std::fs::read_to_string(dir.join("MEMORY.md")).unwrap();
        assert!(!index.contains("tmp-topic.md"));
    }
}
