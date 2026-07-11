//! Recovery flows for the turn loop:
//! - `MaxTokens` Stage A (budget escalation) + Stage B (meta-continuation).
//! - Reactive compaction on `ContextTooLong`.
//! - Refusal / `ContentFilter` synthetic-message surfacing.

/// Stage B meta-continuation prompt. Kept terse and model-neutral so 3P
/// providers don't get Anthropic-flavored copy.
pub(crate) const META_CONTINUATION_PROMPT: &str = "Output token limit hit. Resume directly \u{2014} no apology, no recap. \
     Pick up mid-thought. Break remaining work into smaller pieces.";

/// Synthetic message text for `stop_reason: Refusal`.
pub(crate) const REFUSAL_SYNTHETIC: &str = "Model declined to respond.";

/// Synthetic message text for `stop_reason: ContentFilter`.
pub(crate) const CONTENT_FILTER_SYNTHETIC: &str = "Response blocked by content filter.";

/// Consecutive autocompact failures after which threshold-gated autocompaction
/// is disabled for the remainder of the run (Plan B).
pub(crate) const MAX_CONSECUTIVE_COMPACT_FAILURES: u8 = 2;

/// Per-run autocompaction health tracking (Plan B). Lives at module scope so
/// the helper signatures in [`crate::stream`] can name it.
#[derive(Debug, Default)]
pub(crate) struct AutoCompactTracking {
    /// Consecutive threshold-gated compaction failures this run.
    pub(crate) consecutive_failures: u8,
    /// Set once `consecutive_failures` reaches the cap; suppresses further
    /// threshold-gated compaction attempts for the rest of the run.
    pub(crate) disabled: bool,
}

use std::sync::Arc;

use caliban_provider::{Message, StopReason};
use tokio_util::sync::CancellationToken;

use crate::agent::{Agent, AgentConfig};

use super::{InputProvider, StopCondition};

/// What the turn loop should do after a recovery decision (#152).
///
/// The variants map 1:1 onto the loop's control flow. Crucially, `RetryTurn`
/// and `InjectAndContinue` are DISTINCT: a retry re-enters the *same* turn
/// (`continue 'inner`) without consuming a turn slot, while an inject advances
/// to the next turn (`break 'inner`). Collapsing them breaks
/// `stage_a_retry_does_not_double_count_turn`.
pub(crate) enum RecoveryAction {
    /// Redo the current turn without consuming a slot → `continue 'inner`.
    RetryTurn,
    /// Push the carried messages onto history (may be empty) and advance to the
    /// next turn → `history.extend(msgs); break 'inner`.
    InjectAndContinue(Vec<Message>),
    /// Terminate the run with the given stop condition → `break 'outer`. Any
    /// synthetic message (Refusal / `ContentFilter`) has already been pushed
    /// onto history by the method that returns this.
    Surrender(StopCondition),
}

/// Owns the six per-turn / per-run recovery flags previously inlined in
/// `stream_until_done_with_settings`, and the A/B/C state-machine logic spread
/// across the `ContextTooLong` arm, the Stage-A pre-dispatch arm, and the
/// stop-reason match. Each method is a behavior-identical lift (#152).
#[derive(Debug, Default)]
pub(crate) struct RecoveryState {
    /// Stage-A budget escalation: did we already retry THIS turn with the
    /// escalated budget? Reset on every fresh turn.
    stage_a_attempted_this_turn: bool,
    /// Per-turn `max_tokens` override for the next request build (None →
    /// `config.max_tokens`). Always reset *together* with `stage_a_*`.
    override_max_tokens_for_request: Option<u32>,
    /// Stage-B meta-continuation count (per-run).
    meta_continuation_count: u8,
    /// One-shot reactive compaction guard (per-run).
    attempted_reactive_compact: bool,
    /// Cap on `TurnDecision::ContinueWith` injections (per-run).
    forced_continuations: u8,
    /// Threshold-gated autocompaction health (per-run).
    pub(crate) auto_tracking: AutoCompactTracking,
}

impl RecoveryState {
    /// Clear the paired Stage-A flags for a fresh turn. Replaces the duplicated
    /// `stage_a_attempted_this_turn = false; override_max_tokens_for_request = None;`
    /// reset sites.
    pub(crate) fn reset_for_new_turn(&mut self) {
        self.stage_a_attempted_this_turn = false;
        self.override_max_tokens_for_request = None;
    }

    /// The `max_tokens` to use for the next request build.
    pub(crate) fn effective_max_tokens(&self, default: u32) -> u32 {
        self.override_max_tokens_for_request.unwrap_or(default)
    }

    /// Mutable handle to the autocompaction tracker for [`Agent::maybe_compact`].
    pub(crate) fn auto_tracking_mut(&mut self) -> &mut AutoCompactTracking {
        &mut self.auto_tracking
    }

    /// Reactive compaction on `ContextTooLong` (one-shot). Lift of the
    /// `ContextTooLong` provider-error arm: on the first hit, compact once and
    /// retry; if the compactor declines, surrender. On a second hit (guard
    /// already set), the caller never routes here (handled by the `other` arm).
    pub(crate) async fn on_context_too_long(
        &mut self,
        agent: &Agent,
        history: &mut Vec<Message>,
    ) -> RecoveryAction {
        tracing::warn!(
            target: "caliban::recovery",
            "recovery.reactive_compact.fired"
        );
        self.attempted_reactive_compact = true;
        // #421: resolve caps from the *active* model, not `config.model`. After
        // a `/model` swap the two diverge, and compacting to the old (larger)
        // model's budget can leave history still over the active model's limit —
        // the retry hits ContextTooLong again and the one-shot guard then routes
        // to a hard ProviderError.
        let caps = agent.provider.capabilities(agent.active_model().as_str());
        if let Ok(Some(compaction)) = agent.compactor.compact(history, &caps).await {
            *history = compaction.messages;
            // Redo this turn with the compacted history; don't consume a slot.
            return RecoveryAction::RetryTurn;
        }
        RecoveryAction::Surrender(StopCondition::ProviderError(
            "context too long; compactor declined".into(),
        ))
    }

    /// True when the reactive-compaction one-shot has not yet fired, i.e. the
    /// `ContextTooLong` arm should route through [`Self::on_context_too_long`].
    pub(crate) fn reactive_compact_available(&self) -> bool {
        !self.attempted_reactive_compact
    }

    /// Stage A: silent budget-escalation retry. Lift of the pre-dispatch
    /// `MaxTokens` arm. Returns `Some(RetryTurn)` (after arming the escalated
    /// budget) when Stage A should fire; `None` to fall through to normal
    /// turn processing. The caller keeps the `total_usage.merge` so the failed
    /// attempt is still billed.
    pub(crate) fn on_max_tokens_pre_dispatch(
        &mut self,
        cfg: &AgentConfig,
        turn_stop_reason: StopReason,
    ) -> Option<RecoveryAction> {
        if cfg.max_tokens_recovery
            && turn_stop_reason == StopReason::MaxTokens
            && !self.stage_a_attempted_this_turn
        {
            tracing::warn!(
                target: "caliban::recovery",
                from = cfg.max_tokens,
                to = cfg.escalated_max_tokens,
                "recovery.max_tokens.stage_a"
            );
            self.stage_a_attempted_this_turn = true;
            self.override_max_tokens_for_request = Some(cfg.escalated_max_tokens);
            return Some(RecoveryAction::RetryTurn);
        }
        None
    }

    /// True when the current turn is a *failure* outcome for `after_turn_failure`
    /// routing: Refusal / `ContentFilter` always; `MaxTokens` only once Stage B
    /// has exhausted its budget. Lift of the `turn_is_failure` predicate.
    pub(crate) fn turn_is_failure(&self, cfg: &AgentConfig, turn_stop_reason: StopReason) -> bool {
        matches!(
            turn_stop_reason,
            StopReason::Refusal | StopReason::ContentFilter
        ) || (turn_stop_reason == StopReason::MaxTokens
            && cfg.max_tokens_recovery
            && self.stage_a_attempted_this_turn
            && self.meta_continuation_count >= cfg.max_meta_continuations)
    }

    /// Whether a `TurnDecision::ContinueWith` may still inject (per-run cap).
    pub(crate) fn forced_continuation_available(&self) -> bool {
        self.forced_continuations < super::MAX_FORCED_CONTINUATIONS
    }

    /// Record that a forced continuation was consumed.
    pub(crate) fn record_forced_continuation(&mut self) {
        self.forced_continuations += 1;
    }

    /// The current forced-continuation count (for logging at the cap).
    pub(crate) fn forced_continuations(&self) -> u8 {
        self.forced_continuations
    }

    /// The big post-turn stop-reason dispatch (Tasks 4–6): decide whether to
    /// continue, inject-and-advance, or surrender. 1:1 lift of the stop-reason
    /// match — including the Stage B/C `MaxTokens` recovery ladder, the
    /// Refusal / `ContentFilter` synthetic-message surfacing, the
    /// recovery-off `MaxTokens` halt, and the `EndTurn` / `StopSequence` /
    /// interactive-input-source completion path.
    ///
    /// `history` is mutated in place for the message-pushing arms (Stage B
    /// meta prompt, Refusal / `ContentFilter` synthetic, input-source resume).
    pub(crate) async fn on_stop_reason(
        &mut self,
        turn_stop_reason: StopReason,
        cfg: &AgentConfig,
        history: &mut Vec<Message>,
        input_source: Option<&Arc<dyn InputProvider>>,
        cancel: &CancellationToken,
    ) -> RecoveryAction {
        match turn_stop_reason {
            StopReason::ToolUse => {
                // Tool calls came back; reset Stage-A flag so the next turn has
                // a fresh budget-escalation budget, then advance.
                self.reset_for_new_turn();
                RecoveryAction::InjectAndContinue(vec![])
            }
            StopReason::MaxTokens if cfg.max_tokens_recovery => {
                // Stage A handled earlier (silent retry above tool-dispatch /
                // TurnEnd yield / counter inc). If we reach this arm it's
                // because Stage A already fired this turn and the retry still
                // hit MaxTokens — try Stage B.
                debug_assert!(
                    self.stage_a_attempted_this_turn,
                    "Stage A must have fired before we land here"
                );
                if self.meta_continuation_count < cfg.max_meta_continuations {
                    // Stage B: inject the meta-continuation prompt and advance.
                    tracing::warn!(
                        target: "caliban::recovery",
                        meta_continuation = self.meta_continuation_count + 1,
                        "recovery.max_tokens.stage_b"
                    );
                    self.meta_continuation_count += 1;
                    self.reset_for_new_turn();
                    return RecoveryAction::InjectAndContinue(vec![Message::user_text(
                        META_CONTINUATION_PROMPT,
                    )]);
                }
                // Stage C: surrender.
                tracing::error!(
                    target: "caliban::recovery",
                    "recovery.max_tokens.stage_c"
                );
                RecoveryAction::Surrender(StopCondition::MaxTokensExhausted)
            }
            StopReason::Refusal => {
                tracing::warn!(
                    target: "caliban::recovery",
                    "recovery.refusal"
                );
                history.push(Message::assistant_text(REFUSAL_SYNTHETIC));
                RecoveryAction::Surrender(StopCondition::Refusal(REFUSAL_SYNTHETIC.into()))
            }
            StopReason::ContentFilter => {
                tracing::warn!(
                    target: "caliban::recovery",
                    "recovery.content_filter"
                );
                history.push(Message::assistant_text(CONTENT_FILTER_SYNTHETIC));
                RecoveryAction::Surrender(StopCondition::ContentFilter(
                    CONTENT_FILTER_SYNTHETIC.into(),
                ))
            }
            StopReason::MaxTokens => {
                // Recovery disabled — surface as a distinct stop condition so
                // the TUI / headless driver can tell a budget blowout from a
                // clean end-of-turn.
                tracing::warn!(
                    target: "caliban::recovery",
                    "max_tokens.halt"
                );
                RecoveryAction::Surrender(StopCondition::MaxTokensExhausted)
            }
            _ => {
                // EndTurn or StopSequence — natural completion. If an
                // interactive input source is configured, await the next
                // operator message instead of ending the run (ADR 0047 / #81).
                // Human-driven, so NOT subject to MAX_FORCED_CONTINUATIONS.
                if let Some(provider) = input_source {
                    let next = provider.next_input(cancel).await;
                    if cancel.is_cancelled() {
                        return RecoveryAction::Surrender(StopCondition::Cancelled);
                    }
                    match next {
                        Some(msgs) if !msgs.is_empty() => {
                            self.reset_for_new_turn();
                            return RecoveryAction::InjectAndContinue(msgs);
                        }
                        _ => {
                            // None / empty → end of input.
                            return RecoveryAction::Surrender(StopCondition::EndOfTurn);
                        }
                    }
                }
                RecoveryAction::Surrender(StopCondition::EndOfTurn)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_are_non_empty() {
        assert!(!META_CONTINUATION_PROMPT.is_empty());
        assert!(!REFUSAL_SYNTHETIC.is_empty());
        assert!(!CONTENT_FILTER_SYNTHETIC.is_empty());
    }
}
