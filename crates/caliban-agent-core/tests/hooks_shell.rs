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
