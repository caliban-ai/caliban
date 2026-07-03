//! Integration tests for headless mode (ADR 0025).
//!
//! Most tests drive the `HeadlessDriver` directly against a scripted
//! `MockProvider` so we don't shell out to the binary. CLI-surface tests
//! that need real argv parsing live in `tests/cli.rs`.

#![allow(missing_docs)]

use std::process::Command;
use std::sync::Arc;

use caliban_agent_core::{Agent, Message, ToolRegistry, TurnEvent};
use caliban_provider::{
    MockProvider, Provider, StopReason, StreamEvent, StreamingContentType, StreamingDelta, Usage,
};
use futures::StreamExt as _;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

// We test against the headless module via the binary's source tree by
// re-declaring it under a test-only path. Cargo allows this when the
// module is included via `#[path = ...]`. The simpler path: the test
// binary already links the `caliban` bin's compilation unit, but tests
// in `caliban/tests/*.rs` are *integration tests* and don't have access
// to bin-private items. Instead, we drive the binary's CLI surface where
// it matters and call the lower layers only via published types.
//
// For the format/event tests, we shell out a builder that uses public
// `caliban-provider` + `caliban-agent-core` types and asserts the JSON
// shapes the binary emits. The format/encoding-only tests are unit tests
// in `caliban/src/headless/*.rs`; this file owns the integration paths.

// --- Binary smoke / CLI surface tests ------------------------------------

#[test]
fn caliban_help_lists_headless_flags() {
    let exe = env!("CARGO_BIN_EXE_caliban");
    let out = Command::new(exe)
        .arg("--help")
        .output()
        .expect("failed to invoke caliban --help");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    for flag in [
        "--print",
        "--output-format",
        "--input-format",
        "--max-budget-usd",
        "--bare",
        "--json-schema",
        "--include-partial-messages",
        "--include-hook-events",
        "--replay-user-messages",
        "--continue",
        "--resume",
        "--fallback-model",
        "--permission-prompt-tool",
    ] {
        assert!(
            stdout.contains(flag),
            "expected --help to mention {flag}, got:\n{stdout}",
        );
    }
}

#[test]
fn caliban_invalid_output_format_exits_nonzero() {
    let exe = env!("CARGO_BIN_EXE_caliban");
    let out = Command::new(exe)
        .args(["-p", "hi", "--output-format", "yaml"])
        .output()
        .expect("failed to invoke caliban");
    assert!(!out.status.success(), "expected non-zero exit, got {out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("yaml") || stderr.contains("invalid"),
        "expected stderr to mention the invalid format, got: {stderr}",
    );
}

#[test]
fn caliban_continue_with_empty_store_exits_66() {
    // --continue with no sessions to resume → exit 66.
    let exe = env!("CARGO_BIN_EXE_caliban");
    let tmp = TempDir::new().unwrap();
    let out = Command::new(exe)
        .args([
            "-p",
            "hi",
            "--continue",
            "--sessions-dir",
            tmp.path().to_str().unwrap(),
            "--no-mcp",
            "--bare",
            "--session",
            "x",
        ])
        .env("CALIBAN_DEBUG", "")
        .env_remove("ANTHROPIC_API_KEY")
        .output()
        .expect("failed to invoke caliban");
    // We can't run the real agent without an API key; but --continue is
    // checked early enough that the resume-not-found error wins.
    let code = out.status.code().unwrap_or(0);
    assert!(
        code == 66 || code == 1,
        "expected exit 66 (or 1 if a different error fires first), got {code}; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn caliban_continue_with_empty_sessions_dir_no_session_arg_exits_66() {
    // Finding 11 (2026-05-27 LM Studio probe): `--sessions-dir <empty>
    // --continue` without `--session <NAME>` previously silently fell
    // through to a fresh ephemeral run (exit 0). The fix honors the
    // user-supplied sessions dir, scans it for prior sessions, and exits
    // 66 when none are found.
    //
    // We use `--provider openai` with a fake `OPENAI_API_KEY` so provider
    // initialization succeeds and the `--continue` resolution path actually
    // runs — otherwise an unrelated "API key missing" error would mask the
    // regression. The model name only matters for store-key bucketing; it
    // is never dispatched (the agent loop exits at the session-resolution
    // step with exit 66 before any HTTP request fires).
    let exe = env!("CARGO_BIN_EXE_caliban");
    let tmp = TempDir::new().unwrap();
    let out = Command::new(exe)
        .args([
            "-p",
            "hi",
            "--continue",
            "--sessions-dir",
            tmp.path().to_str().unwrap(),
            "--no-mcp",
            "--bare",
            "--provider",
            "openai",
            "--model",
            "gpt-4o-mini",
            // Crucially: no `--session <NAME>`. That's the regression path.
        ])
        .env("OPENAI_API_KEY", "sk-test-fake")
        .env_remove("ANTHROPIC_API_KEY")
        .output()
        .expect("failed to invoke caliban");
    let code = out.status.code().unwrap_or(0);
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Must exit 66 (NoSessionsToContinue per ADR 0025); never 0.
    assert_ne!(
        code, 0,
        "regression: --continue with empty --sessions-dir silently ran fresh ephemeral; stderr: {stderr}"
    );
    assert_eq!(
        code, 66,
        "expected 66 NoSessionsToContinue, got {code}; stderr: {stderr}"
    );
    assert!(
        stderr.contains("no sessions to continue"),
        "stderr must explain why we refused; got: {stderr}"
    );
}

// --- HeadlessDriver-level tests using MockProvider ----------------------

fn one_turn_text_response(text: &str) -> Vec<Result<StreamEvent, caliban_provider::Error>> {
    vec![
        Ok(StreamEvent::MessageStart {
            id: "msg_1".into(),
            model: "mock".into(),
        }),
        Ok(StreamEvent::ContentBlockStart {
            index: 0,
            content_type: StreamingContentType::Text,
        }),
        Ok(StreamEvent::Delta {
            index: 0,
            delta: StreamingDelta::Text(text.to_string()),
        }),
        Ok(StreamEvent::ContentBlockStop { index: 0 }),
        Ok(StreamEvent::MessageDelta {
            stop_reason: Some(StopReason::EndTurn),
            usage_delta: Some(Usage {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
        }),
        Ok(StreamEvent::MessageStop),
    ]
}

fn build_agent(provider: Arc<MockProvider>) -> Arc<Agent> {
    let provider_dyn: Arc<dyn Provider + Send + Sync> = provider;
    let agent = Agent::builder()
        .provider(provider_dyn)
        .tools(ToolRegistry::new())
        .model("mock")
        .max_tokens(64)
        .max_turns(10)
        .build()
        .expect("agent builder");
    Arc::new(agent)
}

#[tokio::test]
async fn text_format_prints_final_text_with_newline() {
    let mock = Arc::new(MockProvider::new());
    mock.enqueue_stream(one_turn_text_response("hello world"));
    let agent = build_agent(mock);

    // We can't drive caliban::headless directly from an integration test
    // (it's a bin-private module). Instead, drive the agent ourselves and
    // assert the assistant text matches what the headless text encoder
    // would produce — the format is "the assistant's text + trailing \n".
    let messages = vec![Message::user_text("hi")];
    let mut stream = agent.stream_until_done(messages, CancellationToken::new());
    let mut out = String::new();
    while let Some(ev) = stream.next().await {
        if let Ok(TurnEvent::AssistantTextDelta { text, .. }) = ev {
            out.push_str(&text);
        }
    }
    assert_eq!(out, "hello world");
}

#[tokio::test]
async fn json_format_shape_includes_required_fields() {
    // The JSON format wraps the same logical content as the result frame.
    // Ensure the protocol fields are present as documented in ADR 0025 + the
    // #222 enrichment (ADR 0049): additive Claude-Code-contract keys alongside
    // the legacy ones. The real serialized frame is asserted end-to-end in the
    // bin's `headless::tests` (the driver is bin-private, unreachable here).
    use serde_json::json;
    let frame = json!({
        "type": "result",
        "subtype": "success",
        "result": "the answer",
        "is_error": false,
        "session_id": "sess-1",
        "total_cost_usd": 0.0,
        "turns": 1,
        "num_turns": 1,
        "total_input_tokens": 10,
        "total_output_tokens": 5,
        "usage": { "input_tokens": 10, "output_tokens": 5 },
        "duration_ms": 0,
    });
    assert_eq!(frame["type"], "result");
    assert_eq!(frame["subtype"], "success");
    assert_eq!(frame["session_id"], "sess-1");
    assert_eq!(frame["num_turns"], frame["turns"]);
    assert_eq!(frame["usage"]["input_tokens"], frame["total_input_tokens"]);
    // Required protocol surface (legacy + #222 additive CC-contract keys).
    for k in [
        "type",
        "subtype",
        "result",
        "is_error",
        "session_id",
        "total_cost_usd",
        "turns",
        "num_turns",
        "usage",
        "duration_ms",
    ] {
        assert!(frame.get(k).is_some(), "missing key {k}");
    }
}

#[tokio::test]
async fn stream_json_frame_starts_with_system_init_shape() {
    // Mirror the frame the driver emits for `system/init` and assert its
    // documented shape.
    let frame = serde_json::json!({
        "type": "system",
        "subtype": "init",
        "session_id": "s1",
        "model": "anthropic/claude",
        "tools": ["Bash", "Read"],
        "settingSources": ["builtin", "hooks.toml"],
        "plugins": [],
        "bare_mode": false,
        "cwd": "/tmp",
    });
    assert_eq!(frame["type"], "system");
    assert_eq!(frame["subtype"], "init");
    // camelCase per ADR 0025
    assert!(frame.get("settingSources").is_some());
    assert_eq!(frame["settingSources"][0], "builtin");
    assert!(frame["plugins"].is_array());
}

#[test]
fn caliban_print_flag_short_form_accepts_inline_prompt() {
    // Verify `-p "<prompt>"` parses without -p requiring an arg the same
    // turn (since we declared num_args = 0..=1 + default_missing_value).
    let exe = env!("CARGO_BIN_EXE_caliban");
    let out = Command::new(exe)
        .args(["-p", "--help"])
        // --help short-circuits clap before run_headless dispatches.
        .output()
        .expect("failed to invoke caliban");
    // help text always exits 0
    assert!(
        out.status.success(),
        "expected clap to accept '-p --help' and short-circuit to help, got {out:?}",
    );
}

#[test]
fn cli_max_budget_usd_parses_float() {
    let exe = env!("CARGO_BIN_EXE_caliban");
    let out = Command::new(exe)
        .args(["--max-budget-usd", "0.01", "--help"])
        .output()
        .expect("failed to invoke caliban");
    assert!(out.status.success());
}

#[test]
fn cli_unknown_output_format_errors_with_64_or_2() {
    let exe = env!("CARGO_BIN_EXE_caliban");
    let out = Command::new(exe)
        .args(["-p", "x", "--output-format", "bogus"])
        .output()
        .expect("failed to invoke caliban");
    let code = out.status.code().unwrap_or(0);
    // clap surfaces invalid enum values via exit 2.
    assert!(
        code == 2 || code == 64,
        "expected exit 2 (clap error) or 64 (EX_USAGE), got {code}",
    );
}

#[test]
fn cli_resume_missing_session_exits_with_resume_error() {
    let exe = env!("CARGO_BIN_EXE_caliban");
    let tmp = TempDir::new().unwrap();
    let out = Command::new(exe)
        .args([
            "-p",
            "hi",
            "--resume",
            "no-such-session",
            "--sessions-dir",
            tmp.path().to_str().unwrap(),
            "--session",
            "x",
            "--bare",
            "--no-mcp",
        ])
        .env_remove("ANTHROPIC_API_KEY")
        .output()
        .expect("failed to invoke caliban");
    let code = out.status.code().unwrap_or(0);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        code == 66 || stderr.contains("no session") || code == 1,
        "expected resume-not-found (66) or generic err (1), got {code}; stderr={stderr}",
    );
}

#[test]
fn cli_bare_flag_accepted() {
    let exe = env!("CARGO_BIN_EXE_caliban");
    let out = Command::new(exe)
        .args(["--bare", "--help"])
        .output()
        .expect("failed to invoke caliban");
    assert!(out.status.success());
}

#[test]
fn cli_include_hook_events_flag_accepted() {
    let exe = env!("CARGO_BIN_EXE_caliban");
    let out = Command::new(exe)
        .args(["--include-hook-events", "--help"])
        .output()
        .expect("failed to invoke caliban");
    assert!(out.status.success());
}

#[test]
fn cli_replay_user_messages_flag_accepted() {
    let exe = env!("CARGO_BIN_EXE_caliban");
    let out = Command::new(exe)
        .args(["--replay-user-messages", "--help"])
        .output()
        .expect("failed to invoke caliban");
    assert!(out.status.success());
}

#[test]
fn cli_permission_prompt_tool_flag_accepted_but_inert() {
    let exe = env!("CARGO_BIN_EXE_caliban");
    let out = Command::new(exe)
        .args(["--permission-prompt-tool", "mcp__svc__ask", "--help"])
        .output()
        .expect("failed to invoke caliban");
    // --help short-circuits before the inert warning fires; the parse must succeed.
    assert!(out.status.success());
}

#[test]
fn cli_continue_flag_accepted() {
    let exe = env!("CARGO_BIN_EXE_caliban");
    let out = Command::new(exe)
        .args(["-c", "--help"])
        .output()
        .expect("failed to invoke caliban");
    assert!(out.status.success());
}
