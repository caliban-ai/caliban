//! Plan-mode prompts emit empty-manifest checkpoint markers.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use caliban_agent_core::{Hooks, RunCtx, RunHookOutcome};
use caliban_checkpoint::{CheckpointHook, CheckpointRecorder, CheckpointStore, ManifestKind};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn plan_mode_records_empty_manifest_with_plan_kind() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let canonical_ws = std::fs::canonicalize(&ws).unwrap();
    let root = tmp.path().join("store");
    std::fs::create_dir_all(&root).unwrap();
    let store = CheckpointStore::open_in(&root, &canonical_ws, "sess-1").unwrap();
    let rec = CheckpointRecorder::new(store, canonical_ws.clone());
    let flag: Arc<AtomicBool> = Arc::new(AtomicBool::new(true));
    let hook = CheckpointHook::new(rec.clone(), canonical_ws).with_plan_mode(flag);

    let ctx = RunCtx {
        session_id: "sess-1",
        workspace_root: Path::new("/"),
        user_message: None,
        prompt_index: 1,
        cancel: CancellationToken::new(),
    };
    hook.before_run(&ctx).await.unwrap();
    let m = rec.snapshot_manifest().await.unwrap();
    assert_eq!(m.kind, ManifestKind::Plan);
    assert!(m.entries.is_empty());

    let outcome = RunHookOutcome {
        turn_count: 1,
        input_tokens: 0,
        output_tokens: 0,
        success: true,
    };
    hook.after_run(&ctx, &outcome).await.unwrap();
    // Flushed marker.
    let on_disk = rec.store().load_manifest(1).unwrap();
    assert_eq!(on_disk.kind, ManifestKind::Plan);
}
