//! Non-streaming wrappers: `run_turn` and `run_until_done`.

use std::sync::Arc;

use caliban_provider::{Message, StopReason};
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;

use crate::agent::Agent;
use crate::error::{Error, Result};
use crate::stream::{RunOutcome, TurnEvent, TurnOutcome};

impl Agent {
    /// Run a single provider turn (one provider call plus any tool dispatches)
    /// and return the [`TurnOutcome`] once the turn completes.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Cancelled`] if the cancellation token fires,
    /// [`Error::Misconfigured`] if the stream closes without emitting a
    /// [`TurnEvent::TurnEnd`], or any other [`Error`] variant propagated from
    /// the stream.
    pub async fn run_turn(
        self: Arc<Self>,
        messages: Vec<Message>,
        cancel: CancellationToken,
    ) -> Result<TurnOutcome> {
        let mut stream = self.stream_until_done(messages, cancel);
        while let Some(event) = stream.next().await {
            if let TurnEvent::TurnEnd {
                assistant_message,
                tool_results,
                stop_reason,
                usage,
                ..
            } = event?
            {
                return Ok(TurnOutcome {
                    continue_loop: stop_reason == StopReason::ToolUse,
                    assistant_message,
                    tool_results,
                    stop_reason,
                    usage,
                });
            }
        }
        Err(Error::Misconfigured("stream ended without TurnEnd".into()))
    }

    /// Drive the agent loop to completion and return the [`RunOutcome`].
    ///
    /// This is the simplest entry point for callers that do not need streaming
    /// progress events. It consumes all [`TurnEvent`]s and returns once the
    /// stream emits a [`TurnEvent::RunEnd`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::Cancelled`] if the cancellation token fires,
    /// [`Error::Misconfigured`] if the stream closes without emitting a
    /// [`TurnEvent::RunEnd`], or any other [`Error`] variant propagated from
    /// the stream.
    pub async fn run_until_done(
        self: Arc<Self>,
        messages: Vec<Message>,
        cancel: CancellationToken,
    ) -> Result<RunOutcome> {
        let mut stream = self.stream_until_done(messages, cancel);
        while let Some(event) = stream.next().await {
            if let TurnEvent::RunEnd {
                final_messages,
                total_usage,
                turn_count,
                stopped_for,
            } = event?
            {
                return Ok(RunOutcome {
                    final_messages,
                    turn_count,
                    total_usage,
                    stopped_for,
                });
            }
        }
        Err(Error::Misconfigured("stream ended without RunEnd".into()))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use caliban_provider::Message;
    use tokio_util::sync::CancellationToken;

    use crate::stream::StopCondition;

    #[test]
    fn stop_condition_variants_accessible() {
        // Ensure StopCondition variants used in RunOutcome are reachable.
        let _ = StopCondition::EndOfTurn;
        let _ = StopCondition::Cancelled;
    }

    #[test]
    fn cancellation_token_can_be_created() {
        let cancel = CancellationToken::new();
        assert!(!cancel.is_cancelled());
    }

    /// Compile-time assertion: verify `run_turn` and `run_until_done` have the
    /// correct `Arc<Self>` receiver and parameter types. This function is never
    /// called; it just must compile.
    #[allow(dead_code)]
    fn _assert_method_signatures() {
        use crate::agent::Agent;
        use crate::error::Result;
        use crate::stream::{RunOutcome, TurnOutcome};

        let _: fn(Arc<Agent>, Vec<Message>, CancellationToken) -> _ =
            |agent, msgs, cancel| agent.run_turn(msgs, cancel);
        let _: fn(Arc<Agent>, Vec<Message>, CancellationToken) -> _ =
            |agent, msgs, cancel| agent.run_until_done(msgs, cancel);

        let _: Option<Result<TurnOutcome>> = None;
        let _: Option<Result<RunOutcome>> = None;
    }
}
