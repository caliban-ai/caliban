//! #380: with `OTEL_LOG_USER_PROMPTS` **off** (the default), the chat generation
//! span carries neither `gen_ai.input.messages` nor `gen_ai.output.messages` —
//! no message content is emitted.
//!
//! Runs in its own test binary so the process-global content gate is read once
//! with the env unset. See `genai_content_helpers`.

#![allow(missing_docs)]

mod genai_content_helpers;

use genai_content_helpers::{attr, run_and_capture_chat_span};

#[tokio::test]
async fn content_absent_when_logging_disabled() {
    // Defensively ensure the gate is off even if the caller's environment set
    // it. Sole test in this binary; the removal precedes any span record.
    // SAFETY: single-threaded remove before any concurrent env access in-process.
    #[allow(unsafe_code)]
    unsafe {
        std::env::remove_var("OTEL_LOG_USER_PROMPTS");
    }

    let chat = run_and_capture_chat_span("hi there", "hello back").await;

    assert!(
        attr(&chat, "gen_ai.input.messages").is_none(),
        "gen_ai.input.messages must be absent when logging is disabled",
    );
    assert!(
        attr(&chat, "gen_ai.output.messages").is_none(),
        "gen_ai.output.messages must be absent when logging is disabled",
    );
    // Sanity: the span itself still exists with its non-content attributes.
    assert!(
        attr(&chat, "gen_ai.request.model").is_some(),
        "the chat span should still carry its request model",
    );
}
