//! #380: with `OTEL_LOG_USER_PROMPTS` **on**, the chat generation span carries
//! `gen_ai.input.messages` (the user prompt) and `gen_ai.output.messages` (the
//! assistant reply) as valid semconv JSON.
//!
//! Runs in its own test binary so the process-global content gate
//! (`LazyLock<bool>` over `OTEL_LOG_USER_PROMPTS`) is read exactly once, with
//! the env set before any span is recorded. See `genai_content_helpers`.

#![allow(missing_docs)]

mod genai_content_helpers;

use genai_content_helpers::{attr, run_and_capture_chat_span};
use opentelemetry::Value;

#[tokio::test]
async fn content_present_when_logging_enabled() {
    // Set the gate BEFORE the first turn (and thus before the LazyLock is read).
    // Sole test in this binary, so no other thread races this mutation.
    // SAFETY: single-threaded set before any concurrent env access in-process.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("OTEL_LOG_USER_PROMPTS", "1");
    }

    let chat = run_and_capture_chat_span("hi there", "hello back").await;

    let input = match attr(&chat, "gen_ai.input.messages") {
        Some(Value::String(s)) => s.to_string(),
        other => panic!("gen_ai.input.messages should be a JSON string, got: {other:?}"),
    };
    let output = match attr(&chat, "gen_ai.output.messages") {
        Some(Value::String(s)) => s.to_string(),
        other => panic!("gen_ai.output.messages should be a JSON string, got: {other:?}"),
    };

    // Both attributes are valid JSON arrays of `{role, parts}` objects.
    let input_json: serde_json::Value = serde_json::from_str(&input).expect("input is valid JSON");
    let output_json: serde_json::Value =
        serde_json::from_str(&output).expect("output is valid JSON");

    assert_eq!(input_json[0]["role"], "user", "input role");
    assert_eq!(input_json[0]["parts"][0]["type"], "text", "input part type");
    assert_eq!(
        input_json[0]["parts"][0]["content"], "hi there",
        "input carries the user prompt",
    );

    assert_eq!(output_json[0]["role"], "assistant", "output role");
    assert_eq!(
        output_json[0]["parts"][0]["type"], "text",
        "output part type"
    );
    assert_eq!(
        output_json[0]["parts"][0]["content"], "hello back",
        "output carries the assistant reply",
    );
}
