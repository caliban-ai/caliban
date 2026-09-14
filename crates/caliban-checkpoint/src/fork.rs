//! Fork-from-checkpoint (#37) — branch a *new* session from a selected
//! checkpoint without mutating the current one.
//!
//! `/rewind` (ADR 0028, [`crate::restore`]) restores a checkpoint **in
//! place**: it overwrites the working tree and/or truncates the live
//! conversation. A *fork* is the non-destructive counterpart — it produces a
//! brand-new [`PersistedSession`] seeded with the conversation as it stood at
//! the checkpoint, leaving the source session, its checkpoint history, and the
//! working tree untouched. The caller persists the returned session through a
//! `SessionStore`; the user later resumes it (`/resume`, or
//! `caliban --session <name>`).
//!
//! **Files are not part of a fork.** There is one working tree, so a fork
//! branches the *conversation* only — matching Claude Code's fork model.
//! Combine with a `[c]` code restore ([`crate::restore_files_only`]) when the
//! files should also roll back to the checkpoint.

use chrono::Utc;

use caliban_provider::Usage;
use caliban_sessions::PersistedSession;

use crate::error::Result;
use crate::restore::restore_conversation;
use crate::store::CheckpointStore;

/// Build a new session branched from the checkpoint at `prompt_index`.
///
/// The returned session is a clone of `source` with a fresh identity —
/// `new_name`, `created_at`/`updated_at` reset to now, and a zeroed cumulative
/// [`Usage`] (a fork starts its own cost meter; the message history carries the
/// conversation, not the parent's accounting). Its conversation is truncated at
/// the checkpoint using the exact same rule as `/rewind`'s `[v]` (conversation)
/// action ([`restore_conversation`]): the target prompt and its assistant
/// turn(s) survive, everything after is dropped.
///
/// Nothing is written to disk and `source` is not mutated — the caller owns
/// persistence. Todos and plan-mode carry over from `source` as-is (there is no
/// per-checkpoint snapshot of that state to restore).
///
/// # Errors
/// [`crate::error::CheckpointError::NotFound`] when `prompt_index` has no
/// checkpoint manifest in `store`; other I/O / serde errors from loading it.
pub fn fork_session(
    store: &CheckpointStore,
    source: &PersistedSession,
    prompt_index: u32,
    new_name: &str,
) -> Result<PersistedSession> {
    // Load the manifest first: this validates the checkpoint exists (surfacing
    // `NotFound` for a bad index) and supplies `last_message_id` for the
    // truncation, matching `restore`'s shape.
    let manifest = store.load_manifest(prompt_index)?;

    let mut forked = source.clone();
    new_name.clone_into(&mut forked.name);
    let now = Utc::now();
    forked.created_at = now;
    forked.updated_at = now;
    forked.total_usage = Usage::default();

    restore_conversation(
        &mut forked,
        manifest.last_message_id.as_deref(),
        prompt_index,
    );

    Ok(forked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Manifest, ManifestKind};
    use caliban_provider::{ContentBlock, Message, Role, TextBlock};
    use tempfile::TempDir;

    fn store_in(tmp: &TempDir) -> CheckpointStore {
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let canonical_ws = std::fs::canonicalize(&ws).unwrap();
        let store_root = tmp.path().join("store");
        std::fs::create_dir_all(&store_root).unwrap();
        CheckpointStore::open_in(&store_root, &canonical_ws, "sess-1").unwrap()
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

    fn source_session() -> PersistedSession {
        let mut s = PersistedSession::new("parent", "anthropic", "claude");
        s.messages = vec![
            user("prompt 1"),
            assistant("response 1"),
            user("prompt 2"),
            assistant("response 2"),
            user("prompt 3"),
            assistant("response 3"),
        ];
        s.total_usage = Usage {
            input_tokens: 100,
            output_tokens: 50,
            ..Usage::default()
        };
        s
    }

    fn save_prompt(store: &CheckpointStore, idx: u32) {
        store
            .save_manifest(&Manifest::new(idx, ManifestKind::Files, format!("p{idx}")))
            .unwrap();
    }

    #[test]
    fn fork_truncates_conversation_and_stamps_new_identity() {
        let tmp = TempDir::new().unwrap();
        let store = store_in(&tmp);
        save_prompt(&store, 2);
        let source = source_session();

        let forked = fork_session(&store, &source, 2, "parent-fork2-abc").unwrap();

        // Conversation truncated at prompt 2 → U1,A1,U2,A2 survive (#181 rule).
        assert_eq!(forked.messages.len(), 4);
        assert_eq!(forked.messages[3].role, Role::Assistant);
        // Fresh identity.
        assert_eq!(forked.name, "parent-fork2-abc");
        assert_eq!(
            forked.total_usage,
            Usage::default(),
            "a fork starts its own cost meter"
        );
        // Provider/model carried over from the source.
        assert_eq!(forked.provider, "anthropic");
        assert_eq!(forked.model, "claude");
    }

    #[test]
    fn fork_does_not_mutate_source() {
        let tmp = TempDir::new().unwrap();
        let store = store_in(&tmp);
        save_prompt(&store, 1);
        let source = source_session();
        let before_len = source.messages.len();
        let before_name = source.name.clone();
        let before_usage = source.total_usage;

        let _ = fork_session(&store, &source, 1, "parent-fork1-xyz").unwrap();

        assert_eq!(source.messages.len(), before_len, "source messages intact");
        assert_eq!(source.name, before_name, "source name intact");
        assert_eq!(source.total_usage, before_usage, "source usage intact");
    }

    #[test]
    fn fork_at_last_prompt_keeps_whole_conversation() {
        let tmp = TempDir::new().unwrap();
        let store = store_in(&tmp);
        save_prompt(&store, 3);
        let source = source_session();

        let forked = fork_session(&store, &source, 3, "p-fork3").unwrap();

        assert_eq!(
            forked.messages.len(),
            6,
            "forking at the final prompt keeps everything"
        );
    }

    #[test]
    fn fork_missing_prompt_is_not_found() {
        let tmp = TempDir::new().unwrap();
        let store = store_in(&tmp);
        let source = source_session();

        let err = fork_session(&store, &source, 99, "p-fork99").unwrap_err();
        assert!(
            matches!(err, crate::error::CheckpointError::NotFound(99)),
            "absent checkpoint should surface NotFound, got {err:?}"
        );
    }

    #[test]
    fn fork_preserves_todos_and_plan_mode() {
        use caliban_agent_core::{Todo, TodoStatus};
        let tmp = TempDir::new().unwrap();
        let store = store_in(&tmp);
        save_prompt(&store, 2);
        let mut source = source_session();
        source.plan_mode = true;
        source.todos = vec![Todo {
            id: "1".into(),
            content: "carry me".into(),
            status: TodoStatus::InProgress,
        }];

        let forked = fork_session(&store, &source, 2, "p-fork2").unwrap();
        assert!(forked.plan_mode, "plan mode carries into the fork");
        assert_eq!(forked.todos, source.todos, "todos carry into the fork");
    }
}
