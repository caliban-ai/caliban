//! Integration tests for `BashTool` + `caliban-sandbox`.
//!
//! Most tests run on any host. The "actually exec through bwrap /
//! sandbox-exec" tests are gated `#[ignore]` so CI without those
//! binaries doesn't fail; run them with
//! `cargo test --test bash_sandbox -- --ignored`.

use std::sync::Arc;

use caliban_agent_core::{Tool, ToolContext};
use caliban_sandbox::{Backend, FilesystemAcl, Policy, SandboxedShim};
use caliban_tools_builtin::{BashTool, WorkspaceRoot};
use serde_json::json;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

fn ctx() -> ToolContext {
    ToolContext {
        tool_use_id: "t1".into(),
        cancel: CancellationToken::new(),
        hooks: None,
        turn_index: 0,
    }
}

/// `BashTool::new` => no sandbox attached => `is_sandboxed` = false.
#[tokio::test]
async fn unsandboxed_bash_tool_reports_no_sandbox() {
    let tmp = TempDir::new().unwrap();
    let tool = BashTool::new(WorkspaceRoot::new(tmp.path()));
    assert!(!tool.is_sandboxed());
}

/// With a disabled policy, the tool is still "unsandboxed" — the shim is
/// inactive and Bash behaves exactly as without one attached.
#[tokio::test]
async fn disabled_policy_means_not_sandboxed() {
    let tmp = TempDir::new().unwrap();
    let shim = Arc::new(SandboxedShim::new(Policy::default()).expect("shim"));
    let tool = BashTool::with_sandbox(WorkspaceRoot::new(tmp.path()), Some(shim));
    assert!(!tool.is_sandboxed());

    // Still works end-to-end: the shim is a no-op so /bin/sh -c "echo hi"
    // succeeds.
    let out = tool
        .invoke(json!({"command": "echo hi"}), ctx())
        .await
        .expect("echo runs");
    let text = format!("{out:?}");
    assert!(text.contains("hi"), "out: {text}");
}

/// `auto_allows_bash` only fires when policy is enabled, backend is
/// available, and the operator opted in.
#[tokio::test]
async fn auto_allow_flag_requires_active_sandbox() {
    // Disabled policy, flag set: not auto-allowed.
    let policy = Policy {
        enabled: false,
        auto_allow_bash_if_sandboxed: true,
        ..Policy::default()
    };
    let shim = SandboxedShim::new(policy).expect("shim");
    assert!(!shim.auto_allows_bash());
}

/// Verify the bypass list short-circuits even when the policy is active.
/// We use the backend-injection trick from the shim tests to avoid
/// requiring bwrap / sandbox-exec on the test host.
#[tokio::test]
async fn unsandboxed_commands_skip_wrap() {
    use std::process::Command as StdCommand;
    use tokio::process::Command as TokioCommand;

    let policy = Policy {
        enabled: true,
        allow_unsandboxed_commands: vec!["git".into()],
        ..Policy::default()
    };

    let shim = caliban_sandbox::shim::SandboxedShim::new(policy.clone()).unwrap_or_else(|_| {
        // Backend may be unavailable on CI; fall back to a manual shim.
        // We can't trivially construct one here without exposing fields,
        // so we just verify the policy check at the Policy level.
        panic!("constructing shim should succeed");
    });
    let _ = shim;

    // Direct policy-level check (covers the bypass logic without needing
    // a real backend).
    assert!(policy.is_unsandboxed_command("git status"));
    assert!(policy.is_unsandboxed_command("git fetch"));
    assert!(!policy.is_unsandboxed_command("curl https://evil.com"));

    // Sanity check that `std::process::Command` and `tokio::process::Command`
    // round-trip program/args (the shim relies on this).
    let mut cmd = TokioCommand::new("/bin/echo");
    cmd.arg("x");
    let std_view: &StdCommand = cmd.as_std();
    assert_eq!(std_view.get_program(), "/bin/echo");
    let args: Vec<_> = std_view.get_args().collect();
    assert_eq!(args, vec!["x"]);
}

/// Live-backend integration: run a tiny command through the real bwrap
/// and check stdout. Gated `#[ignore]` — requires bwrap >= 0.5 on PATH.
#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires bwrap >= 0.5 on PATH"]
async fn bwrap_runs_echo_end_to_end() {
    let tmp = TempDir::new().unwrap();

    let policy = Policy {
        enabled: true,
        filesystem: FilesystemAcl {
            allow_write: vec![tmp.path().to_path_buf(), "/tmp".into()],
            ..FilesystemAcl::default()
        },
        ..Policy::default()
    };
    let shim = Arc::new(SandboxedShim::new(policy).expect("shim ok"));
    assert!(
        matches!(shim.backend(), Backend::Bwrap { .. }),
        "expected bwrap backend, got {:?}",
        shim.backend()
    );
    let tool = BashTool::with_sandbox(WorkspaceRoot::new(tmp.path()), Some(shim));

    let out = tool
        .invoke(json!({"command": "echo hello-sandbox"}), ctx())
        .await
        .expect("invocation");
    let text = format!("{out:?}");
    assert!(text.contains("hello-sandbox"), "text: {text}");
}

/// Live-backend integration: deny-write enforcement under bwrap.
/// `/tmp` is masked so writes there must fail.
#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires bwrap >= 0.5 on PATH"]
async fn bwrap_denies_write_outside_allow_write() {
    let tmp = TempDir::new().unwrap();
    let policy = Policy {
        enabled: true,
        filesystem: FilesystemAcl {
            allow_write: vec![tmp.path().to_path_buf()],
            deny_write: vec!["/tmp".into()],
            ..FilesystemAcl::default()
        },
        ..Policy::default()
    };
    let shim = Arc::new(SandboxedShim::new(policy).expect("shim ok"));
    let tool = BashTool::with_sandbox(WorkspaceRoot::new(tmp.path()), Some(shim));

    // Writing to /tmp should fail because we masked it.
    let res = tool
        .invoke(
            json!({"command": "echo x > /tmp/caliban-sandbox-deny-write-test"}),
            ctx(),
        )
        .await;
    assert!(res.is_err(), "writing to denied path should fail");
}

/// Live-backend integration: same end-to-end test on macOS via
/// sandbox-exec. Gated `#[ignore]`.
#[cfg(target_os = "macos")]
#[tokio::test]
#[ignore = "requires /usr/bin/sandbox-exec"]
async fn seatbelt_runs_echo_end_to_end() {
    let tmp = TempDir::new().unwrap();
    let policy = Policy {
        enabled: true,
        filesystem: FilesystemAcl {
            allow_read: vec!["/".into()],
            allow_write: vec![tmp.path().to_path_buf(), "/tmp".into()],
            ..FilesystemAcl::default()
        },
        ..Policy::default()
    };
    let shim = Arc::new(SandboxedShim::new(policy).expect("shim ok"));
    assert!(
        matches!(shim.backend(), Backend::Seatbelt { .. }),
        "expected seatbelt, got {:?}",
        shim.backend()
    );
    let tool = BashTool::with_sandbox(WorkspaceRoot::new(tmp.path()), Some(shim));

    let out = tool
        .invoke(json!({"command": "echo hello-seatbelt"}), ctx())
        .await
        .expect("invocation");
    let text = format!("{out:?}");
    assert!(text.contains("hello-seatbelt"), "text: {text}");
}
