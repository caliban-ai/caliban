//! End-to-end worker smoke test. Ignored by default (needs a live
//! provider key). Run: `cargo test -p caliban --test agent_worker -- --ignored`
//! with `ANTHROPIC_API_KEY` set.

#[tokio::test]
#[ignore = "needs a live provider key"]
async fn worker_runs_and_writes_ndjson() {
    let dir = tempfile::tempdir().unwrap();
    let store = caliban_supervisor::store::AgentStore::new(dir.path().join("agents"));
    let rec = caliban_supervisor::proto::AgentRecord {
        id: "smoke".into(),
        name: "smoke".into(),
        status: caliban_supervisor::proto::AgentStatus::Spawning,
        started_at: "2026-06-09T00:00:00Z".into(),
        session_dir: store.session_dir("smoke"),
        socket_path: dir.path().join("smoke.sock"),
        spec: caliban_supervisor::proto::SpawnSpec {
            label: None,
            frontmatter_path: None,
            initial_prompt: "Say the single word: pong".into(),
            model: None,
            provider: None,
            tool_allowlist: None,
            isolation_worktree: false,
            inherit_hooks: true,
            interactive: false,
            inherited_hooks_config: None,
        },
    };
    store.write_manifest(&rec).unwrap();
    let manifest = store.session_dir("smoke").join("manifest.json");
    let socket = rec.socket_path.clone();
    let exe = env!("CARGO_BIN_EXE_caliban");
    let status = tokio::process::Command::new(exe)
        .arg("__agent-worker")
        .arg("--manifest")
        .arg(&manifest)
        .arg("--socket")
        .arg(&socket)
        .status()
        .await
        .unwrap();
    assert!(status.success());
    let ndjson = store.session_dir("smoke").join("stdout.ndjson");
    assert!(ndjson.exists(), "worker should write stdout.ndjson");
    assert!(std::fs::metadata(&ndjson).unwrap().len() > 0);
}

/// An unknown `SpawnSpec.provider` must fail fast with `EX_USAGE` (64), not
/// hang or silently fall back to anthropic (#93). This needs no provider
/// key: the worker rejects the provider before constructing any provider.
#[tokio::test]
async fn worker_rejects_unknown_provider_with_exit_64() {
    let dir = tempfile::tempdir().unwrap();
    let store = caliban_supervisor::store::AgentStore::new(dir.path().join("agents"));
    let rec = caliban_supervisor::proto::AgentRecord {
        id: "badprov".into(),
        name: "badprov".into(),
        status: caliban_supervisor::proto::AgentStatus::Spawning,
        started_at: "2026-06-13T00:00:00Z".into(),
        session_dir: store.session_dir("badprov"),
        socket_path: dir.path().join("badprov.sock"),
        spec: caliban_supervisor::proto::SpawnSpec {
            label: None,
            frontmatter_path: None,
            initial_prompt: "hi".into(),
            model: None,
            provider: Some("definitely-not-a-provider".into()),
            tool_allowlist: None,
            isolation_worktree: false,
            inherit_hooks: true,
            interactive: false,
            inherited_hooks_config: None,
        },
    };
    store.write_manifest(&rec).unwrap();
    let manifest = store.session_dir("badprov").join("manifest.json");
    let exe = env!("CARGO_BIN_EXE_caliban");
    let status = tokio::process::Command::new(exe)
        .arg("__agent-worker")
        .arg("--manifest")
        .arg(&manifest)
        .arg("--socket")
        .arg(rec.socket_path)
        .status()
        .await
        .unwrap();
    assert_eq!(
        status.code(),
        Some(64),
        "unknown provider must exit EX_USAGE (64)"
    );
}
