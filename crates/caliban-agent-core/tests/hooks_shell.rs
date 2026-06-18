//! Integration tests for `ShellCommandHook` (ADR 0024).

#![cfg(unix)]

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use caliban_agent_core::{HookDecision, Hooks, ShellCommandHook, ToolCtx};
use tempfile::TempDir;

fn write_script(dir: &TempDir, name: &str, body: &str) -> std::path::PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, body).unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

fn ctx<'a>(name: &'a str, input: &'a serde_json::Value) -> ToolCtx<'a> {
    ToolCtx {
        turn_index: 0,
        tool_use_id: "t1",
        tool_name: name,
        input,
    }
}

#[tokio::test]
async fn exit_zero_is_allow() {
    let dir = TempDir::new().unwrap();
    let script = write_script(&dir, "ok.sh", "#!/bin/sh\nexit 0\n");
    let hook = ShellCommandHook {
        if_pattern: None,
        asynchronous: false,
        command: script.display().to_string(),
        args: vec![],
        timeout: Duration::from_secs(5),
        env: BTreeMap::new(),
        matcher: "*".into(),
        event_name: "PreToolUse".into(),
    };
    let input = serde_json::json!({});
    let d = hook.before_tool(&ctx("Bash", &input)).await.unwrap();
    assert!(matches!(d, HookDecision::Allow));
}

#[tokio::test]
async fn exit_two_is_deny_with_stderr_reason() {
    let dir = TempDir::new().unwrap();
    let script = write_script(
        &dir,
        "deny.sh",
        "#!/bin/sh\necho 'blocked by site policy' >&2\nexit 2\n",
    );
    let hook = ShellCommandHook {
        if_pattern: None,
        asynchronous: false,
        command: script.display().to_string(),
        args: vec![],
        timeout: Duration::from_secs(5),
        env: BTreeMap::new(),
        matcher: "*".into(),
        event_name: "PreToolUse".into(),
    };
    let input = serde_json::json!({});
    let d = hook.before_tool(&ctx("Bash", &input)).await.unwrap();
    match d {
        HookDecision::Deny(msg) => assert!(msg.contains("blocked"), "msg = {msg}"),
        d => panic!("unexpected: {d:?}"),
    }
}

#[tokio::test]
async fn stdout_json_deny_parses() {
    let dir = TempDir::new().unwrap();
    let body = r#"#!/bin/sh
cat <<'EOF'
{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"no rm"}}
EOF
"#;
    let script = write_script(&dir, "deny_json.sh", body);
    let hook = ShellCommandHook {
        if_pattern: None,
        asynchronous: false,
        command: script.display().to_string(),
        args: vec![],
        timeout: Duration::from_secs(5),
        env: BTreeMap::new(),
        matcher: "*".into(),
        event_name: "PreToolUse".into(),
    };
    let input = serde_json::json!({});
    let d = hook.before_tool(&ctx("Bash", &input)).await.unwrap();
    match d {
        HookDecision::Deny(msg) => assert!(msg.contains("no rm")),
        d => panic!("unexpected: {d:?}"),
    }
}

// Previously flaky on loaded Linux CI runners with `unexpected: Allow`:
// `Command::spawn()` intermittently failed with a transient EAGAIN (fork
// hit a temporary resource limit) or ETXTBSY (the just-written script was
// still being closed), and the dispatch path swallowed that as `Allow`.
// Fixed by `spawn_with_retry` in `hooks_router.rs`, which retries transient
// spawn failures with backoff (caliban-ai/caliban#41).
#[tokio::test]
async fn stdout_json_updated_input_parses() {
    let dir = TempDir::new().unwrap();
    let body = r#"#!/bin/sh
cat <<'EOF'
{"hookSpecificOutput":{"hookEventName":"PreToolUse","updatedInput":{"command":"echo safe"}}}
EOF
"#;
    let script = write_script(&dir, "rewrite.sh", body);
    let hook = ShellCommandHook {
        if_pattern: None,
        asynchronous: false,
        command: script.display().to_string(),
        args: vec![],
        timeout: Duration::from_secs(5),
        env: BTreeMap::new(),
        matcher: "*".into(),
        event_name: "PreToolUse".into(),
    };
    let input = serde_json::json!({"command": "rm -rf /"});
    let d = hook.before_tool(&ctx("Bash", &input)).await.unwrap();
    match d {
        HookDecision::UpdatedInput(v) => assert_eq!(v["command"], "echo safe"),
        d => panic!("unexpected: {d:?}"),
    }
}

#[tokio::test]
async fn timeout_treats_as_allow() {
    let dir = TempDir::new().unwrap();
    let script = write_script(&dir, "slow.sh", "#!/bin/sh\nsleep 5\n");
    let hook = ShellCommandHook {
        if_pattern: None,
        asynchronous: false,
        command: script.display().to_string(),
        args: vec![],
        timeout: Duration::from_millis(150),
        env: BTreeMap::new(),
        matcher: "*".into(),
        event_name: "PreToolUse".into(),
    };
    let input = serde_json::json!({});
    let start = std::time::Instant::now();
    let d = hook.before_tool(&ctx("Bash", &input)).await.unwrap();
    let elapsed = start.elapsed();
    assert!(matches!(d, HookDecision::Allow));
    // Should not have actually slept 5s.
    assert!(elapsed < Duration::from_secs(3), "elapsed = {elapsed:?}");
}

#[tokio::test]
async fn matcher_skips_non_matching_tools() {
    let hook = ShellCommandHook {
        if_pattern: None,
        asynchronous: false,
        command: "/bin/false".into(),
        args: vec![],
        timeout: Duration::from_secs(5),
        env: BTreeMap::new(),
        matcher: "WebFetch".into(),
        event_name: "PreToolUse".into(),
    };
    let input = serde_json::json!({});
    let d = hook.before_tool(&ctx("Bash", &input)).await.unwrap();
    // Bash doesn't match WebFetch matcher → handler skipped → Allow.
    assert!(matches!(d, HookDecision::Allow));
}

#[tokio::test]
async fn event_filter_skips_wrong_event() {
    let hook = ShellCommandHook {
        if_pattern: None,
        asynchronous: false,
        command: "/bin/false".into(), // Would Allow per exit-code fallback (-> non-2 = Allow)
        args: vec![],
        timeout: Duration::from_secs(5),
        env: BTreeMap::new(),
        matcher: "*".into(),
        event_name: "PostToolUse".into(),
    };
    let input = serde_json::json!({});
    // before_tool fires on PreToolUse only; PostToolUse hook should skip.
    let d = hook.before_tool(&ctx("Bash", &input)).await.unwrap();
    assert!(matches!(d, HookDecision::Allow));
}

#[tokio::test]
async fn missing_command_returns_allow() {
    let hook = ShellCommandHook {
        if_pattern: None,
        asynchronous: false,
        command: "/nonexistent/binary/that/does/not/exist".into(),
        args: vec![],
        timeout: Duration::from_secs(5),
        env: BTreeMap::new(),
        matcher: "*".into(),
        event_name: "PreToolUse".into(),
    };
    let input = serde_json::json!({});
    let d = hook.before_tool(&ctx("Bash", &input)).await.unwrap();
    assert!(matches!(d, HookDecision::Allow));
}

// --- #171 regression tests --------------------------------------------------

#[tokio::test]
async fn if_pattern_gates_firing() {
    // A handler scoped `if = "Bash:rm *"` must NOT fire for `Bash {ls}`,
    // and MUST fire for `Bash {rm foo}`.
    let dir = TempDir::new().unwrap();
    let script = write_script(&dir, "deny.sh", "#!/bin/sh\nexit 2\n");
    let hook = ShellCommandHook {
        command: script.display().to_string(),
        args: vec![],
        timeout: Duration::from_secs(5),
        env: BTreeMap::new(),
        matcher: "*".into(),
        if_pattern: Some("Bash:rm *".into()),
        asynchronous: false,
        event_name: "PreToolUse".into(),
    };
    let ls = serde_json::json!({"command": "ls"});
    let d = hook.before_tool(&ctx("Bash", &ls)).await.unwrap();
    assert!(
        matches!(d, HookDecision::Allow),
        "if-pattern must suppress firing for a non-matching command, got {d:?}"
    );
    let rm = serde_json::json!({"command": "rm foo"});
    let d = hook.before_tool(&ctx("Bash", &rm)).await.unwrap();
    assert!(
        matches!(d, HookDecision::Deny(_)),
        "if-pattern must allow firing for a matching command, got {d:?}"
    );
}

#[tokio::test]
async fn json_without_decision_plus_exit2_denies() {
    // A hook that prints informational JSON with no permissionDecision AND
    // exits 2 must Deny — the exit code is not swallowed by the JSON blob.
    let dir = TempDir::new().unwrap();
    let script = write_script(
        &dir,
        "info-deny.sh",
        "#!/bin/sh\necho '{\"foo\":1}'\nexit 2\n",
    );
    let hook = ShellCommandHook {
        command: script.display().to_string(),
        args: vec![],
        timeout: Duration::from_secs(5),
        env: BTreeMap::new(),
        matcher: "*".into(),
        if_pattern: None,
        asynchronous: false,
        event_name: "PreToolUse".into(),
    };
    let input = serde_json::json!({});
    let d = hook.before_tool(&ctx("Bash", &input)).await.unwrap();
    assert!(
        matches!(d, HookDecision::Deny(_)),
        "informational JSON + exit 2 must Deny, got {d:?}"
    );
}

#[tokio::test]
async fn async_deny_does_not_block() {
    // An async=true handler is fire-and-forget: even if it would deny, the
    // tool is not blocked (decision ignored).
    let dir = TempDir::new().unwrap();
    let script = write_script(&dir, "slow-deny.sh", "#!/bin/sh\nexit 2\n");
    let hook = ShellCommandHook {
        command: script.display().to_string(),
        args: vec![],
        timeout: Duration::from_secs(5),
        env: BTreeMap::new(),
        matcher: "*".into(),
        if_pattern: None,
        asynchronous: true,
        event_name: "PreToolUse".into(),
    };
    let input = serde_json::json!({});
    let d = hook.before_tool(&ctx("Bash", &input)).await.unwrap();
    assert!(
        matches!(d, HookDecision::Allow),
        "async=true deny must not block the tool, got {d:?}"
    );
}
