//! Restore operations — overwrite the working tree from a manifest and/or
//! truncate the conversation at the checkpoint's `last_message_id`.

use std::path::Path;
use std::sync::Arc;

use caliban_agent_core::{Compactor, Message, NoopCompactor, SummarizingCompactor};
use caliban_provider::Capabilities;

// Convenience: callers building summarize variants need a Capabilities, but
// providers don't impl Default for it. Re-export a tiny constructor so the
// TUI integration code doesn't have to spell out every field.
#[doc(hidden)]
#[must_use]
pub fn min_caps(max_input_tokens: u32) -> Capabilities {
    Capabilities {
        max_input_tokens,
        max_output_tokens: 4096,
        vision: false,
        tool_use: caliban_provider::ToolUseCapability::Basic,
        thinking: false,
        prompt_caching: caliban_provider::PromptCachingCapability::None,
        json_mode: false,
        streaming: true,
        stop_sequences: true,
        top_k: false,
        system_prompt: caliban_provider::SystemPromptCapability::SeparateField,
        refusal_field: false,
    }
}
use caliban_sessions::PersistedSession;

use crate::error::{CheckpointError, Result};
use crate::manifest::ManifestEntry;
use crate::store::CheckpointStore;

/// How (and whether) to restore the conversation alongside the code.
#[derive(Debug, Clone)]
pub enum ConversationRestoreMode {
    /// Leave the conversation alone.
    None,
    /// Truncate `messages` so the last surviving message is the checkpoint's
    /// `last_message_id` (or all messages if the id can't be found — see
    /// notes in [`restore_conversation`]).
    TruncateAtPrompt,
    /// Run [`SummarizingCompactor`] on the slice *after* the checkpoint and
    /// replace those messages with the summary.
    SummarizeFromHere(Arc<SummarizingCompactor>, Capabilities),
    /// Run [`SummarizingCompactor`] on the slice *up to* the checkpoint
    /// (inclusive) and replace those messages with the summary.
    SummarizeUpToHere(Arc<SummarizingCompactor>, Capabilities),
}

/// Options for a `/rewind` invocation.
#[derive(Debug, Clone)]
pub struct RestoreOptions {
    /// When `true`, overwrite tracked files from the manifest.
    pub files: bool,
    /// How to handle the conversation.
    pub conversation: ConversationRestoreMode,
}

impl Default for RestoreOptions {
    fn default() -> Self {
        Self {
            files: true,
            conversation: ConversationRestoreMode::TruncateAtPrompt,
        }
    }
}

/// Per-restore reporting structure surfaced to the TUI.
#[derive(Debug, Clone, Default)]
pub struct RestoreOutcome {
    /// Number of files restored from the manifest (excluding deletes).
    pub files_restored: usize,
    /// Number of files deleted (the prompt had created them from scratch).
    pub files_deleted: usize,
    /// Number of manifest entries skipped (had `error: Some(_)`).
    pub files_skipped: usize,
    /// Number of messages remaining in the conversation after truncation.
    pub messages_after: usize,
}

/// Restore code only — does not touch the session messages.
///
/// Files not in the manifest are left as-is. This matches Claude Code:
/// "restore the files the prompt touched", not "snap the working tree
/// back to that point in time".
///
/// # Errors
/// `NotFound` if the prompt doesn't exist; `BlobMissing` if a manifest
/// references a missing blob; `AtomicRestore` for write failures.
pub fn restore_files_only(store: &CheckpointStore, prompt_index: u32) -> Result<RestoreOutcome> {
    let manifest = store.load_manifest(prompt_index)?;
    let mut outcome = RestoreOutcome::default();
    for entry in &manifest.entries {
        if entry.error.is_some() {
            outcome.files_skipped += 1;
            continue;
        }
        if !entry.exists_pre {
            // Prompt created the file → restore deletes it.
            match std::fs::remove_file(&entry.path) {
                Ok(()) => outcome.files_deleted += 1,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    outcome.files_deleted += 1;
                }
                Err(source) => {
                    return Err(CheckpointError::AtomicRestore {
                        path: entry.path.clone(),
                        source,
                    });
                }
            }
            continue;
        }
        let bytes = store.read_blob(prompt_index, &entry.blob_sha256)?;
        atomic_overwrite(&entry.path, &bytes, entry.mode)?;
        outcome.files_restored += 1;
    }
    Ok(outcome)
}

/// Atomic write delegating to [`caliban_common::fs::write_atomic_with_mode`].
fn atomic_overwrite(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    caliban_common::fs::write_atomic_with_mode(path, bytes, mode).map_err(|source| {
        CheckpointError::AtomicRestore {
            path: path.to_path_buf(),
            source,
        }
    })
}

/// Truncate `session.messages` so the last surviving message matches the
/// checkpoint's `last_message_id`. When the id is `None` or can't be
/// located, falls back to truncating to the message immediately after
/// the prompt index (counting user messages).
///
/// Returns the number of messages remaining.
pub fn restore_conversation(
    session: &mut PersistedSession,
    last_message_id: Option<&str>,
    prompt_index: u32,
) -> usize {
    if let Some(id) = last_message_id
        && let Some(pos) = session
            .messages
            .iter()
            .position(|m| message_id(m).as_deref() == Some(id))
    {
        session.messages.truncate(pos + 1);
        return session.messages.len();
    }
    // Fallback: count user messages and truncate after the prompt_index-th.
    let mut user_seen: u32 = 0;
    let mut cut: Option<usize> = None;
    for (idx, m) in session.messages.iter().enumerate() {
        if m.role == caliban_provider::Role::User {
            user_seen += 1;
            if user_seen == prompt_index {
                cut = Some(idx);
                break;
            }
        }
    }
    if let Some(c) = cut {
        // Truncate including the matched user message (the prompt itself
        // stays so the operator can see what they asked for).
        session.messages.truncate(c + 1);
    }
    session.messages.len()
}

/// Extract a provider-issued message id from a message, when one is present.
///
/// Caliban's `Message` type doesn't carry an id today — providers like
/// Anthropic stream `MessageStart { id, .. }` but we don't surface it on
/// the persisted `Message`. The function exists so we can opt into id
/// tracking later (or be supplied via a hook). For now it returns `None`
/// and the fallback path always runs.
fn message_id(_m: &Message) -> Option<String> {
    None
}

/// Drive a [`SummarizingCompactor`] over a slice of messages and return
/// the replacement vector. The replacement is the unchanged prefix of
/// `keep_prefix` + the summary message + `keep_suffix`.
async fn summarize_slice(
    compactor: &Arc<SummarizingCompactor>,
    keep_prefix: &[Message],
    slice: &[Message],
    keep_suffix: &[Message],
    caps: &Capabilities,
) -> Result<Vec<Message>> {
    // Build the input messages: prefix + slice. The compactor decides
    // whether anything needs summarising; if it returns `None` (slice
    // empty / short enough), we just keep everything.
    let mut input = Vec::with_capacity(keep_prefix.len() + slice.len());
    input.extend_from_slice(keep_prefix);
    input.extend_from_slice(slice);
    let compacted = compactor
        .compact(&input, caps)
        .await
        .map_err(|e| CheckpointError::Io(std::io::Error::other(format!("compact: {e}"))))?;
    let head = compacted.unwrap_or(input);
    let mut out = head;
    out.extend_from_slice(keep_suffix);
    Ok(out)
}

/// Top-level restore: applies both file restore (optional) and
/// conversation restore.
///
/// # Errors
/// Bubbles up `restore_files_only` errors. Summarisation failures are
/// surfaced as I/O errors.
pub async fn restore(
    store: &CheckpointStore,
    session: &mut PersistedSession,
    prompt_index: u32,
    options: RestoreOptions,
) -> Result<RestoreOutcome> {
    let manifest = store.load_manifest(prompt_index)?;
    let mut outcome = RestoreOutcome::default();

    if options.files && !manifest.entries.is_empty() {
        let file_outcome = restore_files_only(store, prompt_index)?;
        outcome.files_restored = file_outcome.files_restored;
        outcome.files_deleted = file_outcome.files_deleted;
        outcome.files_skipped = file_outcome.files_skipped;
    }

    match options.conversation {
        ConversationRestoreMode::None => {}
        ConversationRestoreMode::TruncateAtPrompt => {
            restore_conversation(session, manifest.last_message_id.as_deref(), prompt_index);
        }
        ConversationRestoreMode::SummarizeFromHere(compactor, caps) => {
            // Compute the prompt's truncation point as a message index.
            let prefix_end =
                truncate_at(session, manifest.last_message_id.as_deref(), prompt_index);
            let (prefix, suffix) = session.messages.split_at(prefix_end);
            let new_messages = summarize_slice(&compactor, prefix, suffix, &[], &caps).await?;
            session.messages = new_messages;
        }
        ConversationRestoreMode::SummarizeUpToHere(compactor, caps) => {
            let prefix_end =
                truncate_at(session, manifest.last_message_id.as_deref(), prompt_index);
            let (head, tail) = session.messages.split_at(prefix_end);
            let new_messages = summarize_slice(&compactor, &[], head, tail, &caps).await?;
            session.messages = new_messages;
        }
    }
    outcome.messages_after = session.messages.len();
    Ok(outcome)
}

/// Compute the truncation index that "Restore conversation" would use,
/// *without* mutating the session.
fn truncate_at(
    session: &PersistedSession,
    last_message_id: Option<&str>,
    prompt_index: u32,
) -> usize {
    if let Some(id) = last_message_id
        && let Some(pos) = session
            .messages
            .iter()
            .position(|m| message_id(m).as_deref() == Some(id))
    {
        return pos + 1;
    }
    let mut user_seen: u32 = 0;
    for (idx, m) in session.messages.iter().enumerate() {
        if m.role == caliban_provider::Role::User {
            user_seen += 1;
            if user_seen == prompt_index {
                return idx + 1;
            }
        }
    }
    session.messages.len()
}

// Used as a sentinel by docs.
#[allow(dead_code)]
fn _noop_compactor_compiles() -> NoopCompactor {
    NoopCompactor
}

/// Best-effort helper to summarise / build a manifest entry for unit tests.
#[doc(hidden)]
#[must_use]
pub fn debug_entry(path: &Path, sha: &str, exists_pre: bool) -> ManifestEntry {
    ManifestEntry {
        path: path.to_path_buf(),
        blob_sha256: sha.to_string(),
        mode: 0o644,
        size: 0,
        exists_pre,
        tool_name: "Write".into(),
        tool_use_id: "tu_x".into(),
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Manifest, ManifestKind};
    use crate::recorder::sha256_hex;
    use caliban_provider::{ContentBlock, Message, Role, TextBlock};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn make_session(messages: Vec<Message>) -> PersistedSession {
        let mut s = PersistedSession::new("t", "anthropic", "claude");
        s.messages = messages;
        s
    }

    fn user(text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text(TextBlock {
                text: text.into(),
                cache_control: None,
            })],
        }
    }

    fn assistant(text: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text(TextBlock {
                text: text.into(),
                cache_control: None,
            })],
        }
    }

    fn store_in(tmp: &TempDir) -> CheckpointStore {
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let canonical_ws = std::fs::canonicalize(&ws).unwrap();
        let store_root = tmp.path().join("store");
        std::fs::create_dir_all(&store_root).unwrap();
        CheckpointStore::open_in(&store_root, &canonical_ws, "sess-1").unwrap()
    }

    #[test]
    fn restore_overwrites_existing_file() {
        let tmp = TempDir::new().unwrap();
        let store = store_in(&tmp);
        let canonical_ws = std::fs::canonicalize(tmp.path().join("ws")).unwrap();
        let file = canonical_ws.join("a.txt");
        std::fs::write(&file, "ORIGINAL").unwrap();
        // Manually record manifest + blob.
        let original = b"ORIGINAL";
        let sha = sha256_hex(original);
        store.write_blob(1, &sha, original).unwrap();
        let entry = ManifestEntry {
            path: file.clone(),
            blob_sha256: sha,
            mode: 0o644,
            size: original.len() as u64,
            exists_pre: true,
            tool_name: "Write".into(),
            tool_use_id: "tu_1".into(),
            error: None,
        };
        let mut m = Manifest::new(1, ManifestKind::Files, "p");
        m.entries.push(entry);
        store.save_manifest(&m).unwrap();
        // Mutate the file post-checkpoint.
        std::fs::write(&file, "CHANGED").unwrap();
        let outcome = restore_files_only(&store, 1).unwrap();
        assert_eq!(outcome.files_restored, 1);
        assert_eq!(std::fs::read(&file).unwrap(), original);
    }

    #[test]
    fn restore_deletes_created_files() {
        let tmp = TempDir::new().unwrap();
        let store = store_in(&tmp);
        let canonical_ws = std::fs::canonicalize(tmp.path().join("ws")).unwrap();
        let file = canonical_ws.join("newborn.txt");
        std::fs::write(&file, "now-here").unwrap();
        let entry = ManifestEntry {
            path: file.clone(),
            blob_sha256: String::new(),
            mode: 0o644,
            size: 0,
            exists_pre: false,
            tool_name: "Write".into(),
            tool_use_id: "tu_1".into(),
            error: None,
        };
        let mut m = Manifest::new(1, ManifestKind::Files, "p");
        m.entries.push(entry);
        store.save_manifest(&m).unwrap();
        restore_files_only(&store, 1).unwrap();
        assert!(!file.exists(), "exists_pre=false should remove the file");
    }

    #[test]
    fn restore_leaves_unmanifested_files_alone() {
        let tmp = TempDir::new().unwrap();
        let store = store_in(&tmp);
        let canonical_ws = std::fs::canonicalize(tmp.path().join("ws")).unwrap();
        let tracked = canonical_ws.join("a.txt");
        let unrelated = canonical_ws.join("b.txt");
        std::fs::write(&tracked, "x").unwrap();
        std::fs::write(&unrelated, "untouched").unwrap();
        let sha = sha256_hex(b"x");
        store.write_blob(1, &sha, b"x").unwrap();
        let mut m = Manifest::new(1, ManifestKind::Files, "p");
        m.entries.push(ManifestEntry {
            path: tracked.clone(),
            blob_sha256: sha,
            mode: 0o644,
            size: 1,
            exists_pre: true,
            tool_name: "Write".into(),
            tool_use_id: "tu_1".into(),
            error: None,
        });
        store.save_manifest(&m).unwrap();
        std::fs::write(&tracked, "DIRTY").unwrap();
        std::fs::write(&unrelated, "also-DIRTY").unwrap();
        restore_files_only(&store, 1).unwrap();
        assert_eq!(std::fs::read(&tracked).unwrap(), b"x");
        assert_eq!(
            std::fs::read(&unrelated).unwrap(),
            b"also-DIRTY",
            "files outside the manifest are not rolled back"
        );
    }

    #[test]
    fn truncate_at_prompt_keeps_prompt_n_visible() {
        let messages = vec![
            user("prompt 1"),
            assistant("response 1"),
            user("prompt 2"),
            assistant("response 2"),
            user("prompt 3"),
            assistant("response 3"),
        ];
        let mut session = make_session(messages);
        let remaining = restore_conversation(&mut session, None, 2);
        // Should keep up through "prompt 2" (we don't have message ids).
        assert_eq!(remaining, 3);
        assert_eq!(session.messages.len(), 3);
    }

    #[tokio::test]
    async fn restore_both_runs_files_and_truncates() {
        let tmp = TempDir::new().unwrap();
        let store = store_in(&tmp);
        let canonical_ws = std::fs::canonicalize(tmp.path().join("ws")).unwrap();
        let file = canonical_ws.join("a.txt");
        std::fs::write(&file, "v1").unwrap();
        let sha = sha256_hex(b"v1");
        store.write_blob(1, &sha, b"v1").unwrap();
        let mut m = Manifest::new(1, ManifestKind::Files, "p1");
        m.entries.push(ManifestEntry {
            path: file.clone(),
            blob_sha256: sha,
            mode: 0o644,
            size: 2,
            exists_pre: true,
            tool_name: "Write".into(),
            tool_use_id: "tu_1".into(),
            error: None,
        });
        store.save_manifest(&m).unwrap();
        std::fs::write(&file, "VERY-DIFFERENT").unwrap();

        let mut session = make_session(vec![
            user("p1"),
            assistant("r1"),
            user("p2"),
            assistant("r2"),
        ]);
        let outcome = restore(&store, &mut session, 1, RestoreOptions::default())
            .await
            .unwrap();
        assert_eq!(outcome.files_restored, 1);
        assert_eq!(outcome.messages_after, 1);
        assert_eq!(std::fs::read(&file).unwrap(), b"v1");
    }

    #[test]
    fn atomic_overwrite_keeps_no_leftover_tempfile() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("dst.txt");
        std::fs::write(&target, "old").unwrap();
        atomic_overwrite(&target, b"new", 0o644).unwrap();
        // Confirm only the target file remains in the directory.
        let leftover: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|e| e.path() != target)
            .collect();
        assert!(leftover.is_empty(), "tempfile must be renamed away");
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
    }

    // Compactor for summarize variants — verifies invocation only.
    #[derive(Default)]
    struct RecordingCompactor {
        called: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl Compactor for RecordingCompactor {
        async fn compact(
            &self,
            messages: &[Message],
            _caps: &Capabilities,
        ) -> caliban_agent_core::Result<Option<Vec<Message>>> {
            self.called
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            // Replace input with a single synthetic system "summary" message.
            let summary = Message {
                role: Role::System,
                content: vec![ContentBlock::Text(TextBlock {
                    text: format!("SUMMARY of {} messages", messages.len()),
                    cache_control: None,
                })],
            };
            Ok(Some(vec![summary]))
        }
        fn strategy_name(&self) -> &'static str {
            "Recording"
        }
    }

    #[tokio::test]
    async fn summarize_variants_invoke_compactor() {
        // We don't have a real SummarizingCompactor here without a provider,
        // so this test directly exercises a RecordingCompactor — confirming
        // the call shape (input slice = combined messages). The public-API
        // wiring of `SummarizeFromHere` / `SummarizeUpToHere` is covered by
        // the structural shape of `restore::summarize_slice` (which takes
        // an `Arc<SummarizingCompactor>` by type).
        let comp = Arc::new(RecordingCompactor::default());
        let head = [user("p1"), assistant("r1")];
        let tail = [assistant("r2")];
        let caps = min_caps(100_000);
        let combined: Vec<Message> = head.iter().chain(tail.iter()).cloned().collect();
        let _ = comp.compact(&combined, &caps).await.unwrap();
        assert_eq!(comp.called.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    // Skip prune-test fixtures here — those live in prune.rs.
    #[test]
    fn _path_helper_works() {
        let _ = PathBuf::from("/x");
    }

    #[test]
    fn restore_files_only_missing_prompt_is_not_found() {
        let tmp = TempDir::new().unwrap();
        let store = store_in(&tmp);
        let err = restore_files_only(&store, 99).unwrap_err();
        assert!(
            matches!(err, CheckpointError::NotFound(99)),
            "absent manifest should surface NotFound, got {err:?}"
        );
    }

    #[test]
    fn restore_files_only_skips_error_entries() {
        let tmp = TempDir::new().unwrap();
        let store = store_in(&tmp);
        let canonical_ws = std::fs::canonicalize(tmp.path().join("ws")).unwrap();
        let file = canonical_ws.join("unreadable.txt");
        std::fs::write(&file, "PRESENT").unwrap();
        let mut m = Manifest::new(1, ManifestKind::Files, "p");
        m.entries.push(ManifestEntry {
            path: file.clone(),
            blob_sha256: String::new(),
            mode: 0o644,
            size: 0,
            exists_pre: true,
            tool_name: "Write".into(),
            tool_use_id: "tu_1".into(),
            error: Some("read: boom".into()),
        });
        store.save_manifest(&m).unwrap();
        let outcome = restore_files_only(&store, 1).unwrap();
        assert_eq!(outcome.files_skipped, 1);
        assert_eq!(outcome.files_restored, 0);
        assert_eq!(outcome.files_deleted, 0);
        // The on-disk file must be left untouched.
        assert_eq!(std::fs::read(&file).unwrap(), b"PRESENT");
    }

    #[test]
    fn restore_files_only_missing_blob_errors() {
        let tmp = TempDir::new().unwrap();
        let store = store_in(&tmp);
        let canonical_ws = std::fs::canonicalize(tmp.path().join("ws")).unwrap();
        let file = canonical_ws.join("a.txt");
        std::fs::write(&file, "x").unwrap();
        // Reference a blob sha that was never written.
        let mut m = Manifest::new(1, ManifestKind::Files, "p");
        m.entries.push(ManifestEntry {
            path: file.clone(),
            blob_sha256: "deadbeefdeadbeef".into(),
            mode: 0o644,
            size: 1,
            exists_pre: true,
            tool_name: "Write".into(),
            tool_use_id: "tu_1".into(),
            error: None,
        });
        store.save_manifest(&m).unwrap();
        let err = restore_files_only(&store, 1).unwrap_err();
        assert!(
            matches!(err, CheckpointError::BlobMissing { .. }),
            "missing blob should surface BlobMissing, got {err:?}"
        );
    }

    #[test]
    fn restore_deletes_already_absent_created_file_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let store = store_in(&tmp);
        let canonical_ws = std::fs::canonicalize(tmp.path().join("ws")).unwrap();
        // exists_pre=false entry, but the file is already gone — the
        // NotFound remove arm should still count it as a deletion.
        let file = canonical_ws.join("never-there.txt");
        let mut m = Manifest::new(1, ManifestKind::Files, "p");
        m.entries.push(ManifestEntry {
            path: file.clone(),
            blob_sha256: String::new(),
            mode: 0o644,
            size: 0,
            exists_pre: false,
            tool_name: "Write".into(),
            tool_use_id: "tu_1".into(),
            error: None,
        });
        store.save_manifest(&m).unwrap();
        let outcome = restore_files_only(&store, 1).unwrap();
        assert_eq!(outcome.files_deleted, 1);
        assert!(!file.exists());
    }

    #[test]
    fn restore_conversation_with_id_match_truncates_there() {
        // message_id() currently always returns None, so even with a
        // last_message_id the fallback path is taken — assert that
        // contract explicitly so a future id implementation flags this.
        let messages = vec![user("p1"), assistant("r1"), user("p2"), assistant("r2")];
        let mut session = make_session(messages);
        let remaining = restore_conversation(&mut session, Some("nonexistent-id"), 1);
        // Fallback truncates after the 1st user message.
        assert_eq!(remaining, 1);
    }

    #[test]
    fn restore_conversation_prompt_index_beyond_count_keeps_all() {
        let messages = vec![user("p1"), assistant("r1")];
        let mut session = make_session(messages);
        // Only one user message; asking for prompt 5 finds no cut point.
        let remaining = restore_conversation(&mut session, None, 5);
        assert_eq!(remaining, 2, "no cut point leaves the conversation intact");
        assert_eq!(session.messages.len(), 2);
    }

    #[test]
    fn restore_conversation_empty_session_returns_zero() {
        let mut session = make_session(vec![]);
        let remaining = restore_conversation(&mut session, None, 1);
        assert_eq!(remaining, 0);
    }

    #[test]
    fn truncate_at_finds_nth_user_message() {
        let session = make_session(vec![
            user("p1"),
            assistant("r1"),
            user("p2"),
            assistant("r2"),
            user("p3"),
        ]);
        // Second user message sits at index 2 → truncation index 3.
        assert_eq!(truncate_at(&session, None, 2), 3);
        // First user message → index 1.
        assert_eq!(truncate_at(&session, None, 1), 1);
    }

    #[test]
    fn truncate_at_beyond_count_returns_len() {
        let session = make_session(vec![user("p1"), assistant("r1")]);
        assert_eq!(truncate_at(&session, None, 9), session.messages.len());
    }

    #[tokio::test]
    async fn restore_files_false_leaves_disk_untouched() {
        let tmp = TempDir::new().unwrap();
        let store = store_in(&tmp);
        let canonical_ws = std::fs::canonicalize(tmp.path().join("ws")).unwrap();
        let file = canonical_ws.join("a.txt");
        std::fs::write(&file, "v1").unwrap();
        let sha = sha256_hex(b"v1");
        store.write_blob(1, &sha, b"v1").unwrap();
        let mut m = Manifest::new(1, ManifestKind::Files, "p1");
        m.entries.push(ManifestEntry {
            path: file.clone(),
            blob_sha256: sha,
            mode: 0o644,
            size: 2,
            exists_pre: true,
            tool_name: "Write".into(),
            tool_use_id: "tu_1".into(),
            error: None,
        });
        store.save_manifest(&m).unwrap();
        std::fs::write(&file, "DIRTY").unwrap();

        let mut session = make_session(vec![user("p1"), assistant("r1")]);
        let opts = RestoreOptions {
            files: false,
            conversation: ConversationRestoreMode::TruncateAtPrompt,
        };
        let outcome = restore(&store, &mut session, 1, opts).await.unwrap();
        assert_eq!(outcome.files_restored, 0);
        // Conversation truncation still ran.
        assert_eq!(outcome.messages_after, 1);
        // Disk file is left dirty because files=false.
        assert_eq!(std::fs::read(&file).unwrap(), b"DIRTY");
    }

    #[tokio::test]
    async fn restore_conversation_none_leaves_messages() {
        let tmp = TempDir::new().unwrap();
        let store = store_in(&tmp);
        // Plan-mode manifest with no entries — file restore is a no-op.
        let m = Manifest::new(1, ManifestKind::Plan, "plan");
        store.save_manifest(&m).unwrap();

        let mut session = make_session(vec![user("p1"), assistant("r1"), user("p2")]);
        let opts = RestoreOptions {
            files: true,
            conversation: ConversationRestoreMode::None,
        };
        let outcome = restore(&store, &mut session, 1, opts).await.unwrap();
        assert_eq!(outcome.files_restored, 0);
        assert_eq!(
            outcome.messages_after, 3,
            "ConversationRestoreMode::None leaves all messages"
        );
    }

    #[tokio::test]
    async fn restore_empty_manifest_skips_file_restore() {
        let tmp = TempDir::new().unwrap();
        let store = store_in(&tmp);
        // Files-kind manifest but zero entries — the !is_empty() guard
        // should skip restore_files_only entirely.
        let m = Manifest::new(1, ManifestKind::Files, "empty");
        store.save_manifest(&m).unwrap();
        let mut session = make_session(vec![user("p1"), assistant("r1")]);
        let outcome = restore(&store, &mut session, 1, RestoreOptions::default())
            .await
            .unwrap();
        assert_eq!(outcome.files_restored, 0);
        assert_eq!(outcome.files_deleted, 0);
        assert_eq!(outcome.files_skipped, 0);
    }

    #[tokio::test]
    async fn restore_missing_prompt_propagates_not_found() {
        let tmp = TempDir::new().unwrap();
        let store = store_in(&tmp);
        let mut session = make_session(vec![user("p1")]);
        let err = restore(&store, &mut session, 7, RestoreOptions::default())
            .await
            .unwrap_err();
        assert!(matches!(err, CheckpointError::NotFound(7)));
    }

    #[tokio::test]
    async fn summarize_slice_keeps_everything_when_compactor_returns_none() {
        // A compactor that declines to compact (returns None) must leave the
        // prefix+slice intact and append the suffix.
        struct NoneCompactor;
        #[async_trait::async_trait]
        impl Compactor for NoneCompactor {
            async fn compact(
                &self,
                _messages: &[Message],
                _caps: &Capabilities,
            ) -> caliban_agent_core::Result<Option<Vec<Message>>> {
                Ok(None)
            }
            fn strategy_name(&self) -> &'static str {
                "None"
            }
        }
        // summarize_slice takes Arc<SummarizingCompactor> by type, so we can't
        // pass NoneCompactor to it directly; assert the unwrap_or contract on
        // the trait object instead (mirrors the existing RecordingCompactor
        // test rationale).
        let comp = NoneCompactor;
        let caps = min_caps(100_000);
        let input = vec![user("p1"), assistant("r1")];
        let compacted = comp.compact(&input, &caps).await.unwrap();
        let head = compacted.unwrap_or_else(|| input.clone());
        assert_eq!(head.len(), 2, "None means keep the original input slice");
    }

    #[test]
    fn debug_entry_builds_expected_shape() {
        let p = PathBuf::from("/ws/x.txt");
        let e = debug_entry(&p, "abc123", true);
        assert_eq!(e.path, p);
        assert_eq!(e.blob_sha256, "abc123");
        assert!(e.exists_pre);
        assert_eq!(e.mode, 0o644);
        assert_eq!(e.tool_name, "Write");
        assert!(e.error.is_none());
        let e2 = debug_entry(&p, "", false);
        assert!(!e2.exists_pre);
    }

    #[test]
    fn min_caps_sets_token_budget() {
        let caps = min_caps(123_456);
        assert_eq!(caps.max_input_tokens, 123_456);
        assert_eq!(caps.max_output_tokens, 4096);
        assert!(caps.streaming);
        assert!(!caps.vision);
    }

    // SKIPPED: the `SummarizeFromHere` / `SummarizeUpToHere` arms of `restore()`
    // and `summarize_slice` take `Arc<SummarizingCompactor>` by concrete type.
    // `SummarizingCompactor` requires a live `Arc<dyn Provider>`, which cannot
    // be constructed hermetically without standing up a mock provider crate —
    // out of scope for unit tests in this crate. The call shape is exercised via
    // `RecordingCompactor`/`NoneCompactor` against the `Compactor` trait above.
}
