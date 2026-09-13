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

use caliban_checkpoint::{ConversationRestoreMode, PromptSummary, RestoreOptions};

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_for_key_maps_advertised_keys_and_rejects_others() {
        assert_eq!(action_for_key('c'), Some(RewindAction::Code));
        assert_eq!(action_for_key('v'), Some(RewindAction::Conversation));
        assert_eq!(action_for_key('b'), Some(RewindAction::Both));
        assert_eq!(action_for_key('s'), Some(RewindAction::SummarizeForward));
        assert_eq!(action_for_key('S'), Some(RewindAction::SummarizeBackward));
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
