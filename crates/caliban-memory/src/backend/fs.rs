//! Filesystem-backed topic store (the gonzalo-free default).
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::auto::{TopicDraft, TopicFile, TopicSummary, render_topic_file, validate_slug};
use crate::backend::TopicBackend;
use crate::error::{MemoryError, Result};

/// Filesystem topic store. Enumerates `.md` siblings of `MEMORY.md`; the index
/// is a derived projection (never string-rewritten in place).
#[derive(Debug, Clone)]
pub struct FsTopicBackend {
    dir: PathBuf,
}

impl FsTopicBackend {
    /// Construct a backend over the given memory directory. The directory does
    /// not have to exist yet — `list` returns an empty vec, and `write` will
    /// create it on demand.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn topic_path(&self, slug: &str) -> PathBuf {
        self.dir.join(format!("{slug}.md"))
    }
}

/// Build the `MEMORY.md` body from summaries. Single source of truth for both
/// backends — one `- [title](slug.md) — kind: first-desc-line` per topic,
/// slug-sorted for determinism.
pub(crate) fn derive_index(summaries: &[TopicSummary]) -> String {
    let mut rows: Vec<&TopicSummary> = summaries.iter().collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    let mut out = String::from("# Memory index\n\n");
    for s in rows {
        let desc = s.description.lines().next().unwrap_or("").trim();
        let _ = writeln!(
            out,
            "- [{n}]({n}.md) — {k}: {desc}",
            n = s.name,
            k = s.kind.as_str()
        );
    }
    out
}

#[async_trait]
impl TopicBackend for FsTopicBackend {
    async fn list(&self) -> Result<Vec<TopicSummary>> {
        let mut out = Vec::new();
        let mut rd = match tokio::fs::read_dir(&self.dir).await {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(source) => {
                return Err(MemoryError::Io {
                    path: self.dir.clone(),
                    source,
                });
            }
        };
        while let Some(entry) = rd.next_entry().await.map_err(|source| MemoryError::Io {
            path: self.dir.clone(),
            source,
        })? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            if path.file_name().and_then(|s| s.to_str()) == Some("MEMORY.md") {
                continue;
            }
            match read_summary_fs(&path).await {
                Ok(s) => out.push(s),
                Err(e) => tracing::warn!(?path, error = %e, "skipping malformed topic file"),
            }
        }
        Ok(out)
    }

    async fn read(&self, name: &str) -> Result<TopicFile> {
        validate_slug(name)?;
        let path = self.topic_path(name);
        let raw = tokio::fs::read_to_string(&path)
            .await
            .map_err(|source| MemoryError::Io {
                path: path.clone(),
                source,
            })?;
        TopicFile::parse(&raw, &path)
    }

    async fn write(&self, draft: &TopicDraft) -> Result<String> {
        validate_slug(&draft.name)?;
        tokio::fs::create_dir_all(&self.dir)
            .await
            .map_err(|source| MemoryError::Io {
                path: self.dir.clone(),
                source,
            })?;
        let path = self.topic_path(&draft.name);
        let serialized = render_topic_file(draft);
        // Reuse the crate's atomic writer (sync); the write is small and rare.
        caliban_common::fs::write_atomic(&path, serialized.as_bytes()).map_err(|source| {
            MemoryError::Io {
                path: path.clone(),
                source,
            }
        })?;
        self.rewrite_index().await?;
        Ok(path.display().to_string())
    }

    async fn delete(&self, name: &str) -> Result<()> {
        validate_slug(name)?;
        let path = self.topic_path(name);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(MemoryError::Io { path, source }),
        }
        self.rewrite_index().await?;
        Ok(())
    }

    async fn index(&self) -> Result<String> {
        Ok(derive_index(&self.list().await?))
    }
}

impl FsTopicBackend {
    /// Materialise the derived index to `MEMORY.md` so the on-disk file stays in
    /// sync (the read path can still read it directly on the fs substrate).
    async fn rewrite_index(&self) -> Result<()> {
        let body = derive_index(&self.list().await?);
        let path = self.dir.join("MEMORY.md");
        caliban_common::fs::write_atomic(&path, body.as_bytes())
            .map_err(|source| MemoryError::Io { path, source })
    }
}

async fn read_summary_fs(path: &Path) -> Result<TopicSummary> {
    let raw = tokio::fs::read_to_string(path)
        .await
        .map_err(|source| MemoryError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    TopicSummary::parse(&raw, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auto::TopicKind;
    use crate::backend::TopicBackend;
    use crate::error::MemoryError;

    fn draft(name: &str, desc: &str) -> TopicDraft {
        TopicDraft {
            name: name.into(),
            description: desc.into(),
            kind: TopicKind::Project,
            body: "b".into(),
        }
    }

    fn topic_md(name: &str, kind: &str, desc: &str, body: &str) -> String {
        format!(
            "---\nname: {name}\ndescription: \"{desc}\"\nmetadata:\n  node_type: memory\n  type: {kind}\n---\n\n{body}\n",
        )
    }

    #[tokio::test]
    async fn write_read_list_delete_and_index_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let be = FsTopicBackend::new(tmp.path().to_path_buf());

        be.write(&draft("alpha", "first line")).await.unwrap();
        be.write(&draft("beta", "second")).await.unwrap();

        let mut names: Vec<_> = be
            .list()
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect();
        names.sort_unstable();
        assert_eq!(names, vec!["alpha".to_string(), "beta".to_string()]);

        let f = be.read("alpha").await.unwrap();
        assert_eq!(f.name, "alpha");

        let idx = be.index().await.unwrap();
        assert!(idx.contains("[alpha](alpha.md)"));
        assert!(idx.contains("first line"));

        be.delete("alpha").await.unwrap();
        assert_eq!(be.list().await.unwrap().len(), 1);
        assert!(!be.index().await.unwrap().contains("[alpha]"));
        // idempotent
        be.delete("alpha").await.unwrap();
    }

    #[tokio::test]
    async fn list_enumerates_topic_files_excluding_memory_md() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        tokio::fs::write(
            dir.join("MEMORY.md"),
            "# Memory index\n\n- [foo](foo.md) — user: foo\n",
        )
        .await
        .unwrap();
        tokio::fs::write(
            dir.join("foo.md"),
            topic_md("foo", "user", "foo desc", "body"),
        )
        .await
        .unwrap();
        tokio::fs::write(
            dir.join("bar.md"),
            topic_md("bar", "feedback", "bar desc", "body"),
        )
        .await
        .unwrap();

        let be = FsTopicBackend::new(dir.to_path_buf());
        let topics = be.list().await.unwrap();
        let mut names: Vec<_> = topics.iter().map(|t| t.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, vec!["bar", "foo"]);
        assert!(topics.iter().any(|t| matches!(t.kind, TopicKind::User)));
        assert!(topics.iter().any(|t| matches!(t.kind, TopicKind::Feedback)));
    }

    #[tokio::test]
    async fn read_round_trips_a_topic() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        tokio::fs::write(
            dir.join("user-role.md"),
            topic_md(
                "user-role",
                "user",
                "role + context",
                "# User role\n\nSenior engineer.\n",
            ),
        )
        .await
        .unwrap();

        let be = FsTopicBackend::new(dir.to_path_buf());
        let topic = be.read("user-role").await.unwrap();
        assert_eq!(topic.name, "user-role");
        assert_eq!(topic.kind, TopicKind::User);
        assert!(topic.body.contains("Senior engineer."));
    }

    #[tokio::test]
    async fn write_creates_topic_and_updates_index() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let be = FsTopicBackend::new(dir.to_path_buf());
        let locator = be
            .write(&TopicDraft {
                name: "personal-email".to_string(),
                description: "use personal email for ~/dev/personal/**".to_string(),
                kind: TopicKind::Feedback,
                body: "Use john.ford2002@gmail.com.\n".to_string(),
            })
            .await
            .unwrap();
        let path = PathBuf::from(&locator);
        assert!(path.exists());
        assert!(!dir.join("personal-email.md.tmp").exists());

        // Topic file contains frontmatter + body.
        let written = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(written.contains("name: personal-email"));
        assert!(written.contains("type: feedback"));
        assert!(written.contains("john.ford2002@gmail.com"));

        // Index updated.
        let index = tokio::fs::read_to_string(dir.join("MEMORY.md"))
            .await
            .unwrap();
        assert!(index.contains("[personal-email](personal-email.md)"));
        assert!(index.contains("feedback:"));
    }

    #[tokio::test]
    async fn write_twice_keeps_single_index_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let be = FsTopicBackend::new(dir.to_path_buf());
        be.write(&draft("foo", "old desc")).await.unwrap();
        be.write(&draft("foo", "new desc")).await.unwrap();
        let index = tokio::fs::read_to_string(dir.join("MEMORY.md"))
            .await
            .unwrap();
        // exactly one entry for foo
        assert_eq!(index.matches("[foo](foo.md)").count(), 1);
        assert!(index.contains("new desc"));
        assert!(!index.contains("old desc"));
    }

    #[tokio::test]
    async fn read_rejects_invalid_type_in_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        tokio::fs::write(dir.join("bad.md"), topic_md("bad", "junk", "desc", "body"))
            .await
            .unwrap();
        let be = FsTopicBackend::new(dir.to_path_buf());
        let err = be.read("bad").await.unwrap_err();
        assert!(matches!(err, MemoryError::InvalidTopic { .. }));
    }

    #[tokio::test]
    async fn read_rejects_missing_required_frontmatter_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        tokio::fs::write(
            dir.join("incomplete.md"),
            "---\ndescription: \"no name\"\nmetadata:\n  type: user\n---\n\nbody\n",
        )
        .await
        .unwrap();
        let be = FsTopicBackend::new(dir.to_path_buf());
        let err = be.read("incomplete").await.unwrap_err();
        assert!(matches!(err, MemoryError::InvalidTopic { .. }));
    }

    #[tokio::test]
    async fn cross_reference_brackets_preserved_in_body() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let body = "Crosslinks: [[parity-gap-matrix]], [[sprint-mode]].\n".to_string();
        let be = FsTopicBackend::new(dir.to_path_buf());
        be.write(&TopicDraft {
            name: "user-role".to_string(),
            description: "role".to_string(),
            kind: TopicKind::User,
            body: body.clone(),
        })
        .await
        .unwrap();
        let topic = be.read("user-role").await.unwrap();
        assert!(topic.body.contains("[[parity-gap-matrix]]"));
        assert!(topic.body.contains("[[sprint-mode]]"));
    }

    #[tokio::test]
    async fn delete_removes_file_and_index_line() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let be = FsTopicBackend::new(dir.to_path_buf());
        be.write(&draft("tmp-topic", "tmp")).await.unwrap();
        be.delete("tmp-topic").await.unwrap();
        assert!(!dir.join("tmp-topic.md").exists());
        let index = tokio::fs::read_to_string(dir.join("MEMORY.md"))
            .await
            .unwrap();
        assert!(!index.contains("tmp-topic.md"));
    }

    #[tokio::test]
    async fn list_on_nonexistent_dir_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist");
        let be = FsTopicBackend::new(missing);
        assert!(be.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn list_skips_non_md_files_and_subdirectories() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        tokio::fs::write(dir.join("notes.txt"), "not markdown")
            .await
            .unwrap();
        tokio::fs::create_dir(dir.join("subdir")).await.unwrap();
        // A `.md` directory entry must also be ignored (not a file).
        tokio::fs::create_dir(dir.join("dir.md")).await.unwrap();
        tokio::fs::write(
            dir.join("ok.md"),
            topic_md("ok", "project", "ok desc", "body"),
        )
        .await
        .unwrap();

        let be = FsTopicBackend::new(dir.to_path_buf());
        let topics = be.list().await.unwrap();
        let names: Vec<_> = topics.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["ok"]);
    }

    #[tokio::test]
    async fn list_skips_malformed_topic_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        // Malformed: no frontmatter delimiters at all.
        tokio::fs::write(dir.join("broken.md"), "no frontmatter here\n")
            .await
            .unwrap();
        tokio::fs::write(
            dir.join("good.md"),
            topic_md("good", "reference", "good desc", "body"),
        )
        .await
        .unwrap();

        let be = FsTopicBackend::new(dir.to_path_buf());
        let topics = be.list().await.unwrap();
        let names: Vec<_> = topics.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["good"]);
        assert_eq!(topics[0].kind, TopicKind::Reference);
    }

    #[tokio::test]
    async fn read_rejects_invalid_slug() {
        let tmp = tempfile::tempdir().unwrap();
        let be = FsTopicBackend::new(tmp.path().to_path_buf());
        let err = be.read("../escape").await.unwrap_err();
        assert!(matches!(err, MemoryError::InvalidSlug { .. }));
    }

    #[tokio::test]
    async fn read_missing_file_is_io_error() {
        let tmp = tempfile::tempdir().unwrap();
        let be = FsTopicBackend::new(tmp.path().to_path_buf());
        let err = be.read("nope").await.unwrap_err();
        assert!(matches!(err, MemoryError::Io { .. }));
    }

    #[tokio::test]
    async fn write_then_read_round_trips_description_with_quotes() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let be = FsTopicBackend::new(dir.to_path_buf());
        be.write(&TopicDraft {
            name: "quoted".to_string(),
            description: "use \"smart\" quotes \\ backslash".to_string(),
            kind: TopicKind::Reference,
            body: "body".to_string(),
        })
        .await
        .unwrap();
        let topic = be.read("quoted").await.unwrap();
        assert_eq!(topic.description, "use \"smart\" quotes \\ backslash");
        assert_eq!(topic.kind, TopicKind::Reference);
    }

    #[tokio::test]
    async fn write_creates_index_with_header_when_none_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let be = FsTopicBackend::new(dir.to_path_buf());
        be.write(&draft("first", "first desc")).await.unwrap();
        let index = tokio::fs::read_to_string(dir.join("MEMORY.md"))
            .await
            .unwrap();
        assert!(index.starts_with("# Memory index\n\n"));
        assert!(index.contains("[first](first.md)"));
    }

    #[tokio::test]
    async fn delete_rejects_invalid_slug() {
        let tmp = tempfile::tempdir().unwrap();
        let be = FsTopicBackend::new(tmp.path().to_path_buf());
        let err = be.delete("a/b").await.unwrap_err();
        assert!(matches!(err, MemoryError::InvalidSlug { .. }));
    }

    #[tokio::test]
    async fn delete_missing_topic_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let be = FsTopicBackend::new(tmp.path().to_path_buf());
        // Deleting a topic that was never written succeeds.
        be.delete("never-existed").await.unwrap();
    }

    #[tokio::test]
    async fn fs_backend_passes_conformance() {
        let tmp = tempfile::tempdir().unwrap();
        let be = FsTopicBackend::new(tmp.path().to_path_buf());
        crate::backend::conformance::run_topic_backend_conformance(&be).await;
    }
}
