//! `/rewind` overlay actions (ADR 0028, #549).
//!
//! The overlay lists per-prompt checkpoints (newest first); the user selects
//! one with the cursor and presses an action key. This module owns:
//!
//! - [`RewindAction`] / [`RewindRequest`] — the deferred-action types the key
//!   handler produces and the main loop consumes.
//! - [`action_for_key`] / [`resolve_prompt_index`] — pure helpers (unit-tested).
//! - [`execute_rewind`] — the async executor the main loop runs, applying the
//!   file and/or conversation restore and reporting the outcome via a toast.
//!
//! File restore ([`restore_files_only`]) is synchronous, but the summarize
//! modes drive the provider-backed [`SummarizingCompactor`] and must `.await`,
//! so *all* actions are routed through a pending request that the async main
//! loop executes uniformly.

use caliban_checkpoint::{CheckpointStore, ConversationRestoreMode, PromptSummary, RestoreOptions};
use caliban_sessions::PersistedSession;

use super::app::App;
use super::toast::Toast;

/// Which restore the user asked for from the `/rewind` overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RewindAction {
    /// `[c]` — restore the files the prompt touched; leave the conversation.
    Code,
    /// `[v]` — truncate the conversation at the prompt; leave the files.
    Conversation,
    /// `[b]` — both: restore files *and* truncate the conversation.
    Both,
    /// `[s]` — summarize the conversation slice *after* the prompt (forward).
    SummarizeForward,
    /// `[S]` — summarize the conversation slice *up to* the prompt (backward).
    SummarizeBackward,
    /// `[f]` — fork: write a *new* session branched from the prompt, leaving
    /// the current session, its checkpoints, and the working tree untouched
    /// (#37).
    Fork,
}

/// A selected rewind action against a specific checkpoint, set by the overlay
/// key handler and drained by the async main loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RewindRequest {
    /// The checkpoint's monotonic prompt index.
    pub(crate) prompt_index: u32,
    /// What to restore.
    pub(crate) action: RewindAction,
}

/// Map an overlay action key to its [`RewindAction`]. `S` (shift-s) is the
/// backward summarize; lowercase `s` is forward. Returns `None` for any other
/// character so the caller can fall through to the generic overlay keys.
#[must_use]
pub(crate) fn action_for_key(c: char) -> Option<RewindAction> {
    match c {
        'c' => Some(RewindAction::Code),
        'v' => Some(RewindAction::Conversation),
        'b' => Some(RewindAction::Both),
        's' => Some(RewindAction::SummarizeForward),
        'S' => Some(RewindAction::SummarizeBackward),
        'f' => Some(RewindAction::Fork),
        _ => None,
    }
}

/// Resolve the checkpoint index the cursor points at. `prompts` is the same
/// newest-first list the overlay renders, so a cursor of 0 is the newest
/// checkpoint. Returns `None` for an empty list or out-of-range cursor.
#[must_use]
pub(crate) fn resolve_prompt_index(prompts: &[PromptSummary], cursor: usize) -> Option<u32> {
    prompts.get(cursor).map(|p| p.prompt_index)
}

/// Execute a rewind against the wired checkpoint store, mutating the live
/// conversation (`app.messages` / `app.session`) and the context-window
/// accounting as needed, then surfacing the outcome as a toast.
///
/// Runs from the async main loop (not the sync key handler) because the
/// summarize modes await the compactor.
pub(crate) async fn execute_rewind(app: &mut App, req: RewindRequest) {
    let Some(store) = app.checkpoint_store.clone() else {
        app.toast = Some(Toast::error(
            "checkpointing is not enabled for this session",
        ));
        return;
    };
    let idx = req.prompt_index;

    // `[c]` is a pure file restore — no conversation touch, no compactor.
    if req.action == RewindAction::Code {
        match caliban_checkpoint::restore_files_only(&store, idx) {
            Ok(o) => {
                app.toast = Some(Toast::info(format!(
                    "rewound #{idx}: {} file(s) restored, {} deleted, {} skipped",
                    o.files_restored, o.files_deleted, o.files_skipped,
                )));
            }
            Err(e) => app.toast = Some(Toast::error(format!("rewind #{idx} failed: {e}"))),
        }
        return;
    }

    // `[f]` forks a new session and does NOT mutate the current one — it needs
    // the session store rather than the restore path (#37).
    if req.action == RewindAction::Fork {
        execute_fork(app, &store, idx);
        return;
    }

    // The remaining modes touch the conversation. Build the restore options.
    let files = req.action == RewindAction::Both;
    let conversation = match req.action {
        RewindAction::Conversation | RewindAction::Both => {
            ConversationRestoreMode::TruncateAtPrompt
        }
        RewindAction::SummarizeForward | RewindAction::SummarizeBackward => {
            let model = app.args.model.clone().unwrap_or_else(|| {
                crate::default_model_for(crate::resolved_provider(&app.args)).to_string()
            });
            let caps = app.agent.provider().capabilities(&model);
            let compactor = std::sync::Arc::new(caliban_agent_core::SummarizingCompactor {
                provider: app.agent.provider(),
                summarizer_model: model,
                target_fraction: 0.7,
                keep_recent_turns: 4,
            });
            if req.action == RewindAction::SummarizeForward {
                ConversationRestoreMode::SummarizeFromHere(compactor, caps)
            } else {
                ConversationRestoreMode::SummarizeUpToHere(compactor, caps)
            }
        }
        RewindAction::Code => unreachable!("Code handled above"),
        RewindAction::Fork => unreachable!("Fork handled above"),
    };

    // The live conversation is `app.messages`; the restore API operates on a
    // `PersistedSession`. Carry the messages through a session shim (the real
    // session when present), run the restore, then write the result back to
    // `app.messages`, the persisted session, and the context-window meter.
    let model = app.args.model.clone().unwrap_or_default();
    let mut session = app
        .session
        .clone()
        .unwrap_or_else(|| caliban_sessions::PersistedSession::new("tui-ephemeral", "tui", model));
    session.messages = app.messages.clone();

    match caliban_checkpoint::restore(
        &store,
        &mut session,
        idx,
        RestoreOptions {
            files,
            conversation,
        },
    )
    .await
    {
        Ok(o) => {
            app.messages.clone_from(&session.messages);
            if let Some(s) = app.session.as_mut() {
                s.messages.clone_from(&session.messages);
            }
            app.context_window.record_history(&app.messages);
            let file_note = if files {
                format!(", {} file(s) restored", o.files_restored)
            } else {
                String::new()
            };
            app.toast = Some(Toast::info(format!(
                "rewound #{idx}: {} message(s){file_note}",
                o.messages_after,
            )));
        }
        Err(e) => app.toast = Some(Toast::error(format!("rewind #{idx} failed: {e}"))),
    }
}

/// Fork the checkpoint at `idx` into a brand-new persisted session, leaving the
/// current session (`app.messages` / `app.session`), its checkpoint history, and
/// the working tree untouched (#37). Requires session persistence to be enabled
/// (`app.store`); otherwise it surfaces a guidance toast and does nothing.
fn execute_fork(app: &mut App, store: &CheckpointStore, idx: u32) {
    let Some(session_store) = app.store.clone() else {
        app.toast = Some(Toast::error(
            "forking needs session persistence — start caliban with --session",
        ));
        return;
    };

    // Build the source session the same way the conversation-restore path does:
    // the real persisted session when present, otherwise an ephemeral shim, with
    // `app.messages` as the live conversation.
    let model = app.args.model.clone().unwrap_or_default();
    let mut source = app
        .session
        .clone()
        .unwrap_or_else(|| PersistedSession::new("session", "tui", model));
    source.messages.clone_from(&app.messages);

    let new_name = compose_fork_name(
        &source.name,
        idx,
        &uuid::Uuid::new_v4().simple().to_string(),
    );

    match caliban_checkpoint::fork_session(store, &source, idx, &new_name) {
        Ok(forked) => {
            let msgs = forked.messages.len();
            // Save, then force the debounced write through so the fork is
            // immediately resumable / visible in `/resume`.
            if let Err(e) = session_store
                .save(&forked)
                .and_then(|()| session_store.flush())
            {
                app.toast = Some(Toast::error(format!("fork #{idx} save failed: {e}")));
                return;
            }
            app.toast = Some(Toast::info(format!(
                "forked #{idx} → session \"{new_name}\" ({msgs} msg(s)); resume with /resume",
            )));
        }
        Err(e) => app.toast = Some(Toast::error(format!("fork #{idx} failed: {e}"))),
    }
}

/// Compose a valid, unique session name for a fork:
/// `<sanitized-parent>-fork<idx>-<short>`.
///
/// `caliban_sessions` requires a name that is non-empty, ≤ 64 bytes, and made of
/// only `[A-Za-z0-9_-]`. We sanitize the parent (any other char → `-`), append a
/// `-fork<idx>-` tag plus a short unique suffix, and truncate the parent portion
/// so the whole name fits in 64. `unique` is any string (a UUID at the call
/// site); only its first 8 alphanumerics are used.
fn compose_fork_name(parent: &str, idx: u32, unique: &str) -> String {
    let short: String = unique
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(8)
        .collect();
    let short = if short.is_empty() {
        "x".to_string()
    } else {
        short
    };
    let suffix = format!("-fork{idx}-{short}");

    let base: String = parent
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let base = base.trim_matches('-');
    let base = if base.is_empty() { "session" } else { base };

    // Reserve room for the suffix; truncate the base to fit 64 total. The base
    // is all-ASCII after sanitizing, so byte-slicing is char-safe.
    let max_base = 64usize.saturating_sub(suffix.len());
    let base_trunc = base[..base.len().min(max_base)].trim_end_matches('-');
    let base_final = if base_trunc.is_empty() {
        "s"
    } else {
        base_trunc
    };

    format!("{base_final}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_valid_session_name(name: &str) -> bool {
        !name.is_empty()
            && name.len() <= 64
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    }

    #[test]
    fn compose_fork_name_is_valid_and_descriptive() {
        let n = compose_fork_name("myproj", 3, "abcdef1234567890");
        assert!(n.starts_with("myproj-fork3-"), "got {n}");
        assert!(is_valid_session_name(&n), "invalid: {n}");
    }

    #[test]
    fn compose_fork_name_sanitizes_invalid_chars() {
        let n = compose_fork_name("my/proj name!.v2", 1, "deadbeef");
        assert!(is_valid_session_name(&n), "invalid: {n}");
        assert!(
            !n.contains('/') && !n.contains(' ') && !n.contains('!') && !n.contains('.'),
            "unsanitized char leaked: {n}"
        );
    }

    #[test]
    fn compose_fork_name_truncates_long_parent_to_fit_64() {
        let parent = "a".repeat(200);
        let n = compose_fork_name(&parent, 42, "abcdef12");
        assert!(n.len() <= 64, "len {} > 64: {n}", n.len());
        assert!(is_valid_session_name(&n));
        assert!(n.ends_with("-fork42-abcdef12"), "suffix preserved: {n}");
    }

    #[test]
    fn compose_fork_name_empty_parent_falls_back() {
        let n = compose_fork_name("", 2, "cafebabe");
        assert!(n.starts_with("session-fork2-"), "got {n}");
        assert!(is_valid_session_name(&n));
    }

    #[test]
    fn compose_fork_name_all_invalid_parent_still_valid() {
        let n = compose_fork_name("///", 5, "1234abcd");
        assert!(is_valid_session_name(&n), "invalid: {n}");
    }

    #[test]
    fn action_for_key_maps_advertised_keys_and_rejects_others() {
        assert_eq!(action_for_key('c'), Some(RewindAction::Code));
        assert_eq!(action_for_key('v'), Some(RewindAction::Conversation));
        assert_eq!(action_for_key('b'), Some(RewindAction::Both));
        assert_eq!(action_for_key('s'), Some(RewindAction::SummarizeForward));
        assert_eq!(action_for_key('S'), Some(RewindAction::SummarizeBackward));
        assert_eq!(action_for_key('f'), Some(RewindAction::Fork));
        // Case matters: `s` and `S` are distinct actions.
        assert_ne!(action_for_key('s'), action_for_key('S'));
        // Anything else falls through to the generic overlay keys.
        for other in ['q', 'x', 'C', 'a', '1'] {
            assert_eq!(action_for_key(other), None, "should not bind {other:?}");
        }
    }

    fn summary(idx: u32) -> PromptSummary {
        PromptSummary {
            prompt_index: idx,
            title: format!("prompt {idx}"),
            kind: caliban_checkpoint::ManifestKind::Files,
            created_at: chrono::Utc::now(),
            file_count: 0,
            partial: false,
        }
    }

    #[test]
    fn resolve_prompt_index_indexes_the_newest_first_list() {
        // list_prompts is newest-first, so cursor 0 is the highest index.
        let prompts = vec![summary(5), summary(4), summary(3)];
        assert_eq!(resolve_prompt_index(&prompts, 0), Some(5));
        assert_eq!(resolve_prompt_index(&prompts, 2), Some(3));
        // Out-of-range / empty → None (no action fired).
        assert_eq!(resolve_prompt_index(&prompts, 3), None);
        assert_eq!(resolve_prompt_index(&[], 0), None);
    }
}
