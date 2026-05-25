//! Integration test for `CALIBAN_CHECKPOINT_DISABLED`. Runs in its own
//! process so the env-var mutation can't race with other crate tests.

use std::path::Path;

use caliban_agent_core::{Hooks, RunCtx};
use caliban_checkpoint::{CheckpointHook, CheckpointRecorder, CheckpointStore};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn disabled_env_makes_before_run_a_no_op() {
    // Integration tests run as a separate binary, so this env-var set
    // affects only this process tree. We still scope it tightly.
    #[allow(unsafe_code)]
    // SAFETY: integration-test binary; single-threaded with respect to its
    // own env mutations.
    unsafe {
        std::env::set_var("CALIBAN_CHECKPOINT_DISABLED", "1");
    }

    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let canonical_ws = std::fs::canonicalize(&ws).unwrap();
    let root = tmp.path().join("store");
    std::fs::create_dir_all(&root).unwrap();
    let store = CheckpointStore::open_in(&root, &canonical_ws, "sess-1").unwrap();
    let rec = CheckpointRecorder::new(store, canonical_ws.clone());
    let hook = CheckpointHook::new(rec.clone(), canonical_ws);

    let ctx = RunCtx {
        session_id: "sess-1",
        workspace_root: Path::new("/"),
        user_message: None,
        prompt_index: 1,
        cancel: CancellationToken::new(),
    };
    hook.before_run(&ctx).await.unwrap();
    assert!(
        rec.snapshot_manifest().await.is_none(),
        "DISABLED_ENV must skip"
    );

    #[allow(unsafe_code)]
    // SAFETY: cleanup of the env var.
    unsafe {
        std::env::remove_var("CALIBAN_CHECKPOINT_DISABLED");
    }
}
