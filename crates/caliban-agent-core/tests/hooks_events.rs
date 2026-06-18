//! Trait-level integration tests for the expanded Hooks taxonomy (ADR 0024).
//!
//! These tests exercise the in-process trait surface — composite chaining,
//! `UpdatedInput` threading, default no-ops, and per-event dispatch — without
//! touching the external handler types (those have their own test files).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use caliban_agent_core::{
    CompactCtx, CompactOutcome, CompositeHooks, ConfigChangeCtx, CwdChangedCtx, FileChangeKind,
    FileChangedCtx, HookDecision, Hooks, NoopHooks, NotificationCtx, NotificationLevel, PermCtx,
    PromptCtx, Result, SessionCtx, SessionOutcome, SessionStartOutcome, SubagentCtx,
    SubagentOutcome, TaskCtx, TaskOutcome, ToolCtx,
};

/// Recording hook: appends event names + payload summary to a shared log.
#[derive(Default)]
struct RecorderHooks {
    log: Mutex<Vec<String>>,
}

impl RecorderHooks {
    fn snapshot(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
    fn push(&self, s: impl Into<String>) {
        self.log.lock().unwrap().push(s.into());
    }
}

#[async_trait]
impl Hooks for RecorderHooks {
    async fn session_start(&self, ctx: &SessionCtx<'_>) -> Result<SessionStartOutcome> {
        self.push(format!("session_start:{}", ctx.session_id));
        Ok(SessionStartOutcome::default())
    }
    async fn session_end(&self, ctx: &SessionCtx<'_>, outcome: &SessionOutcome) -> Result<()> {
        self.push(format!(
            "session_end:{}:{}",
            ctx.session_id, outcome.turn_count
        ));
        Ok(())
    }
    async fn user_prompt_submit(&self, ctx: &PromptCtx<'_>) -> Result<HookDecision> {
        self.push(format!("user_prompt_submit:{}", ctx.prompt));
        Ok(HookDecision::Allow)
    }
    async fn pre_compact(&self, ctx: &CompactCtx<'_>) -> Result<()> {
        self.push(format!("pre_compact:{}", ctx.strategy));
        Ok(())
    }
    async fn post_compact(&self, _ctx: &CompactCtx<'_>, outcome: &CompactOutcome) -> Result<()> {
        self.push(format!("post_compact:{}", outcome.compacted));
        Ok(())
    }
    async fn config_change(&self, ctx: &ConfigChangeCtx<'_>) -> Result<()> {
        self.push(format!("config_change:{}", ctx.changed_keys.len()));
        Ok(())
    }
    async fn cwd_changed(&self, ctx: &CwdChangedCtx<'_>) -> Result<()> {
        self.push(format!("cwd_changed:{}", ctx.new_cwd.display()));
        Ok(())
    }
    async fn file_changed(&self, ctx: &FileChangedCtx<'_>) -> Result<()> {
        self.push(format!("file_changed:{}:{}", ctx.tool, ctx.kind.as_str()));
        Ok(())
    }
    async fn permission_request(&self, ctx: &PermCtx<'_>) -> Result<()> {
        self.push(format!("permission_request:{}", ctx.tool_name));
        Ok(())
    }
    async fn permission_denied(&self, ctx: &PermCtx<'_>) -> Result<()> {
        self.push(format!("permission_denied:{}", ctx.tool_name));
        Ok(())
    }
    async fn notification(&self, ctx: &NotificationCtx<'_>) -> Result<()> {
        self.push(format!("notification:{}", ctx.level.as_str()));
        Ok(())
    }
    async fn subagent_start(&self, ctx: &SubagentCtx<'_>) -> Result<()> {
        self.push(format!("subagent_start:{}", ctx.task_id));
        Ok(())
    }
    async fn subagent_stop(&self, _ctx: &SubagentCtx<'_>, outcome: &SubagentOutcome) -> Result<()> {
        self.push(format!("subagent_stop:{}", outcome.success));
        Ok(())
    }
    async fn task_created(&self, ctx: &TaskCtx<'_>) -> Result<()> {
        self.push(format!("task_created:{}", ctx.task_id));
        Ok(())
    }
    async fn task_completed(&self, ctx: &TaskCtx<'_>, _outcome: &TaskOutcome) -> Result<()> {
        self.push(format!("task_completed:{}", ctx.task_id));
        Ok(())
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn default_noop_returns_ok_for_every_event() {
    let h = NoopHooks;

    let s_ctx = SessionCtx {
        session_id: "s",
        cwd: &PathBuf::from("/tmp"),
        provider: "mock",
        model: "test",
    };
    h.session_start(&s_ctx).await.unwrap();
    h.session_end(
        &s_ctx,
        &SessionOutcome {
            turn_count: 0,
            input_tokens: 0,
            output_tokens: 0,
        },
    )
    .await
    .unwrap();

    let p_ctx = PromptCtx {
        session_id: "s",
        cwd: &PathBuf::from("/tmp"),
        turn_index: 0,
        prompt: "hi",
        attachments: &[],
    };
    let d = h.user_prompt_submit(&p_ctx).await.unwrap();
    assert!(matches!(d, HookDecision::Allow));

    let c_ctx = CompactCtx {
        session_id: "s",
        token_count_before: 100,
        strategy: "Noop",
    };
    h.pre_compact(&c_ctx).await.unwrap();
    h.post_compact(
        &c_ctx,
        &CompactOutcome {
            token_count_after: 100,
            compacted: false,
        },
    )
    .await
    .unwrap();

    let cc_ctx = ConfigChangeCtx {
        changed_keys: &[],
        new_settings_summary: "{}",
    };
    h.config_change(&cc_ctx).await.unwrap();

    let cwd_ctx = CwdChangedCtx {
        old_cwd: &PathBuf::from("/a"),
        new_cwd: &PathBuf::from("/b"),
    };
    h.cwd_changed(&cwd_ctx).await.unwrap();

    let fc_ctx = FileChangedCtx {
        path: &PathBuf::from("/a"),
        kind: FileChangeKind::Created,
        tool: "Write",
    };
    h.file_changed(&fc_ctx).await.unwrap();

    let perm_ctx = PermCtx {
        turn_index: 0,
        tool_use_id: "t1",
        tool_name: "Bash",
        input: &serde_json::json!({}),
        rule_action: "allow",
        rule_comment: None,
    };
    h.permission_request(&perm_ctx).await.unwrap();
    h.permission_denied(&perm_ctx).await.unwrap();

    let n_ctx = NotificationCtx {
        level: NotificationLevel::Info,
        message: "hi",
    };
    h.notification(&n_ctx).await.unwrap();

    let sub_ctx = SubagentCtx {
        parent_turn_index: 0,
        agent_name: "agent",
        task_id: "task",
    };
    h.subagent_start(&sub_ctx).await.unwrap();
    h.subagent_stop(
        &sub_ctx,
        &SubagentOutcome {
            success: true,
            final_text: "done".into(),
        },
    )
    .await
    .unwrap();

    let task_ctx = TaskCtx {
        task_id: "t",
        content: "do thing",
        status: "pending",
    };
    h.task_created(&task_ctx).await.unwrap();
    h.task_completed(
        &task_ctx,
        &TaskOutcome {
            terminal_status: "completed".into(),
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn composite_fans_session_start_in_order() {
    let r1 = Arc::new(RecorderHooks::default());
    let r2 = Arc::new(RecorderHooks::default());
    let composite = CompositeHooks::new(vec![
        Arc::clone(&r1) as Arc<dyn Hooks>,
        Arc::clone(&r2) as Arc<dyn Hooks>,
    ]);
    let ctx = SessionCtx {
        session_id: "S",
        cwd: &PathBuf::from("/t"),
        provider: "p",
        model: "m",
    };
    composite.session_start(&ctx).await.unwrap();
    assert_eq!(r1.snapshot(), vec!["session_start:S"]);
    assert_eq!(r2.snapshot(), vec!["session_start:S"]);
}

#[tokio::test]
async fn composite_after_turn_runs_lifo() {
    use std::sync::Mutex;
    static ORDER: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

    struct A;
    #[async_trait]
    impl Hooks for A {
        async fn after_turn(
            &self,
            _: &caliban_agent_core::TurnCtx<'_>,
            _: &caliban_agent_core::TurnOutcome,
        ) -> Result<caliban_agent_core::TurnDecision> {
            ORDER.lock().unwrap().push("A");
            Ok(caliban_agent_core::TurnDecision::Continue)
        }
    }
    struct B;
    #[async_trait]
    impl Hooks for B {
        async fn after_turn(
            &self,
            _: &caliban_agent_core::TurnCtx<'_>,
            _: &caliban_agent_core::TurnOutcome,
        ) -> Result<caliban_agent_core::TurnDecision> {
            ORDER.lock().unwrap().push("B");
            Ok(caliban_agent_core::TurnDecision::Continue)
        }
    }
    let composite = CompositeHooks::new(vec![
        Arc::new(A) as Arc<dyn Hooks>,
        Arc::new(B) as Arc<dyn Hooks>,
    ]);
    let cfg = caliban_agent_core::AgentConfig::default();
    let outcome = caliban_agent_core::TurnOutcome {
        assistant_message: caliban_provider::Message::user_text(""),
        tool_results: vec![],
        stop_reason: caliban_provider::StopReason::EndTurn,
        usage: caliban_provider::Usage::default(),
        continue_loop: false,
    };
    let ctx = caliban_agent_core::TurnCtx {
        turn_index: 0,
        messages: &[],
        config: &cfg,
    };
    composite.after_turn(&ctx, &outcome).await.unwrap();
    // LIFO: B before A.
    let order = ORDER.lock().unwrap().clone();
    assert_eq!(order, vec!["B", "A"]);
}

#[tokio::test]
async fn composite_before_tool_short_circuits_on_first_deny() {
    struct Allow;
    #[async_trait]
    impl Hooks for Allow {
        async fn before_tool(&self, _: &ToolCtx<'_>) -> Result<HookDecision> {
            Ok(HookDecision::Allow)
        }
    }
    struct Deny;
    #[async_trait]
    impl Hooks for Deny {
        async fn before_tool(&self, _: &ToolCtx<'_>) -> Result<HookDecision> {
            Ok(HookDecision::Deny("blocked".into()))
        }
    }
    struct Panic;
    #[async_trait]
    impl Hooks for Panic {
        async fn before_tool(&self, _: &ToolCtx<'_>) -> Result<HookDecision> {
            panic!("should not run after deny");
        }
    }
    let composite = CompositeHooks::new(vec![
        Arc::new(Allow) as Arc<dyn Hooks>,
        Arc::new(Deny) as Arc<dyn Hooks>,
        Arc::new(Panic) as Arc<dyn Hooks>,
    ]);
    let input = serde_json::json!({});
    let ctx = ToolCtx {
        turn_index: 0,
        tool_use_id: "x",
        tool_name: "Bash",
        input: &input,
        is_read_only: false,
    };
    let d = composite.before_tool(&ctx).await.unwrap();
    assert!(matches!(d, HookDecision::Deny(_)));
}

#[tokio::test]
async fn composite_threads_updated_input_through_layers() {
    use std::sync::Mutex;
    static OBSERVED_INPUTS: Mutex<Vec<serde_json::Value>> = Mutex::new(Vec::new());

    struct Rewrite;
    #[async_trait]
    impl Hooks for Rewrite {
        async fn before_tool(&self, ctx: &ToolCtx<'_>) -> Result<HookDecision> {
            OBSERVED_INPUTS.lock().unwrap().push(ctx.input.clone());
            Ok(HookDecision::UpdatedInput(serde_json::json!({"v": 1})))
        }
    }
    struct Observe;
    #[async_trait]
    impl Hooks for Observe {
        async fn before_tool(&self, ctx: &ToolCtx<'_>) -> Result<HookDecision> {
            OBSERVED_INPUTS.lock().unwrap().push(ctx.input.clone());
            Ok(HookDecision::Allow)
        }
    }
    let composite = CompositeHooks::new(vec![
        Arc::new(Rewrite) as Arc<dyn Hooks>,
        Arc::new(Observe) as Arc<dyn Hooks>,
    ]);
    let input = serde_json::json!({"original": true});
    let ctx = ToolCtx {
        turn_index: 0,
        tool_use_id: "x",
        tool_name: "Bash",
        input: &input,
        is_read_only: false,
    };
    let d = composite.before_tool(&ctx).await.unwrap();
    assert!(matches!(d, HookDecision::UpdatedInput(_)));
    let obs = OBSERVED_INPUTS.lock().unwrap().clone();
    assert_eq!(obs.len(), 2);
    assert_eq!(obs[0], serde_json::json!({"original": true}));
    assert_eq!(obs[1], serde_json::json!({"v": 1}));
}

#[tokio::test]
async fn composite_empty_returns_allow() {
    let composite = CompositeHooks::new(vec![]);
    assert!(composite.is_empty());
    assert_eq!(composite.len(), 0);
    let input = serde_json::json!({});
    let ctx = ToolCtx {
        turn_index: 0,
        tool_use_id: "x",
        tool_name: "Bash",
        input: &input,
        is_read_only: false,
    };
    let d = composite.before_tool(&ctx).await.unwrap();
    assert!(matches!(d, HookDecision::Allow));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn composite_fans_every_event_to_layers() {
    let r = Arc::new(RecorderHooks::default());
    let composite = CompositeHooks::new(vec![Arc::clone(&r) as Arc<dyn Hooks>]);
    let cwd = PathBuf::from("/t");
    let session_ctx = SessionCtx {
        session_id: "S",
        cwd: &cwd,
        provider: "p",
        model: "m",
    };
    composite.session_start(&session_ctx).await.unwrap();
    composite
        .session_end(
            &session_ctx,
            &SessionOutcome {
                turn_count: 1,
                input_tokens: 0,
                output_tokens: 0,
            },
        )
        .await
        .unwrap();

    let prompt_ctx = PromptCtx {
        session_id: "S",
        cwd: &cwd,
        turn_index: 0,
        prompt: "hello",
        attachments: &[],
    };
    let d = composite.user_prompt_submit(&prompt_ctx).await.unwrap();
    assert!(matches!(d, HookDecision::Allow));

    let cmp_ctx = CompactCtx {
        session_id: "S",
        token_count_before: 1000,
        strategy: "DropOldest",
    };
    composite.pre_compact(&cmp_ctx).await.unwrap();
    composite
        .post_compact(
            &cmp_ctx,
            &CompactOutcome {
                token_count_after: 500,
                compacted: true,
            },
        )
        .await
        .unwrap();

    let cc_ctx = ConfigChangeCtx {
        changed_keys: &["x".into()],
        new_settings_summary: "{}",
    };
    composite.config_change(&cc_ctx).await.unwrap();

    let cwd_ctx = CwdChangedCtx {
        old_cwd: &PathBuf::from("/a"),
        new_cwd: &PathBuf::from("/b"),
    };
    composite.cwd_changed(&cwd_ctx).await.unwrap();

    let fc_ctx = FileChangedCtx {
        path: &PathBuf::from("/a/b.txt"),
        kind: FileChangeKind::Modified,
        tool: "Edit",
    };
    composite.file_changed(&fc_ctx).await.unwrap();

    let perm_ctx = PermCtx {
        turn_index: 0,
        tool_use_id: "t1",
        tool_name: "Bash",
        input: &serde_json::json!({}),
        rule_action: "ask",
        rule_comment: None,
    };
    composite.permission_request(&perm_ctx).await.unwrap();
    composite.permission_denied(&perm_ctx).await.unwrap();

    let n_ctx = NotificationCtx {
        level: NotificationLevel::Warn,
        message: "uh",
    };
    composite.notification(&n_ctx).await.unwrap();

    let sub_ctx = SubagentCtx {
        parent_turn_index: 0,
        agent_name: "agent",
        task_id: "task-x",
    };
    composite.subagent_start(&sub_ctx).await.unwrap();
    composite
        .subagent_stop(
            &sub_ctx,
            &SubagentOutcome {
                success: true,
                final_text: "done".into(),
            },
        )
        .await
        .unwrap();

    let task_ctx = TaskCtx {
        task_id: "T",
        content: "stuff",
        status: "in_progress",
    };
    composite.task_created(&task_ctx).await.unwrap();
    composite
        .task_completed(
            &task_ctx,
            &TaskOutcome {
                terminal_status: "completed".into(),
            },
        )
        .await
        .unwrap();

    let snap = r.snapshot();
    let expected: Vec<&str> = vec![
        "session_start:S",
        "session_end:S:1",
        "user_prompt_submit:hello",
        "pre_compact:DropOldest",
        "post_compact:true",
        "config_change:1",
        "cwd_changed:/b",
        "file_changed:Edit:modified",
        "permission_request:Bash",
        "permission_denied:Bash",
        "notification:warn",
        "subagent_start:task-x",
        "subagent_stop:true",
        "task_created:T",
        "task_completed:T",
    ];
    assert_eq!(snap, expected);
}

#[tokio::test]
async fn composite_user_prompt_submit_rewrites() {
    struct Rewrite;
    #[async_trait]
    impl Hooks for Rewrite {
        async fn user_prompt_submit(&self, _: &PromptCtx<'_>) -> Result<HookDecision> {
            Ok(HookDecision::UpdatedInput(serde_json::Value::String(
                "rewritten".into(),
            )))
        }
    }
    let composite = CompositeHooks::new(vec![Arc::new(Rewrite) as Arc<dyn Hooks>]);
    let cwd = PathBuf::from("/t");
    let ctx = PromptCtx {
        session_id: "s",
        cwd: &cwd,
        turn_index: 0,
        prompt: "original",
        attachments: &[],
    };
    let d = composite.user_prompt_submit(&ctx).await.unwrap();
    match d {
        HookDecision::UpdatedInput(v) => {
            assert_eq!(v, serde_json::Value::String("rewritten".into()));
        }
        _ => panic!(),
    }
}

#[tokio::test]
async fn composite_user_prompt_submit_deny_short_circuits() {
    struct Allow;
    #[async_trait]
    impl Hooks for Allow {
        async fn user_prompt_submit(&self, _: &PromptCtx<'_>) -> Result<HookDecision> {
            Ok(HookDecision::Allow)
        }
    }
    struct Deny;
    #[async_trait]
    impl Hooks for Deny {
        async fn user_prompt_submit(&self, _: &PromptCtx<'_>) -> Result<HookDecision> {
            Ok(HookDecision::Deny("nope".into()))
        }
    }
    let composite = CompositeHooks::new(vec![
        Arc::new(Allow) as Arc<dyn Hooks>,
        Arc::new(Deny) as Arc<dyn Hooks>,
    ]);
    let cwd = PathBuf::from("/t");
    let ctx = PromptCtx {
        session_id: "s",
        cwd: &cwd,
        turn_index: 0,
        prompt: "hi",
        attachments: &[],
    };
    let d = composite.user_prompt_submit(&ctx).await.unwrap();
    assert!(matches!(d, HookDecision::Deny(_)));
}

#[tokio::test]
async fn file_change_kind_str_round_trip() {
    assert_eq!(FileChangeKind::Created.as_str(), "created");
    assert_eq!(FileChangeKind::Modified.as_str(), "modified");
    assert_eq!(FileChangeKind::Deleted.as_str(), "deleted");
}

#[tokio::test]
async fn notification_level_str_round_trip() {
    assert_eq!(NotificationLevel::Info.as_str(), "info");
    assert_eq!(NotificationLevel::Warn.as_str(), "warn");
    assert_eq!(NotificationLevel::Error.as_str(), "error");
}

#[tokio::test]
async fn build_envelope_includes_camel_case_event_name() {
    let env = caliban_agent_core::build_envelope(
        "PreToolUse",
        serde_json::json!({"tool": {"name": "Bash"}}),
    );
    assert_eq!(env["hookEventName"], "PreToolUse");
    assert_eq!(env["tool"]["name"], "Bash");
}

#[tokio::test]
async fn build_envelope_preserves_snake_case_for_other_fields() {
    let env = caliban_agent_core::build_envelope(
        "PreToolUse",
        serde_json::json!({"session_id": "s", "turn_index": 3}),
    );
    assert_eq!(env["session_id"], "s");
    assert_eq!(env["turn_index"], 3);
}

#[tokio::test]
async fn envelope_with_cwd_inserts_cwd_string() {
    let env = caliban_agent_core::envelope_with_cwd(
        "SessionStart",
        &PathBuf::from("/proj"),
        serde_json::Map::new(),
    );
    assert_eq!(env["cwd"], "/proj");
}

// ---------------------------------------------------------------------------
// CompositeHooks all-noop short-circuit (PR-T4-D)
// ---------------------------------------------------------------------------

/// Counting wrapper that asserts no event method is ever called. Used to
/// verify the all-noop short-circuit path doesn't `await` member hooks.
///
/// Unlike `NoopHooks`, this impl reports `is_noop() = true` (so it doesn't
/// flip the composite's flag) but panics if any event method actually runs —
/// proving the composite never delegated.
#[derive(Default)]
struct AssertSilentNoop {
    invocations: std::sync::atomic::AtomicUsize,
}

impl AssertSilentNoop {
    fn invocation_count(&self) -> usize {
        self.invocations.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn bump(&self) {
        self.invocations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[async_trait]
impl Hooks for AssertSilentNoop {
    fn is_noop(&self) -> bool {
        true
    }
    async fn before_run(&self, _: &caliban_agent_core::RunCtx<'_>) -> Result<()> {
        self.bump();
        Ok(())
    }
    async fn after_run(
        &self,
        _: &caliban_agent_core::RunCtx<'_>,
        _: &caliban_agent_core::RunHookOutcome,
    ) -> Result<()> {
        self.bump();
        Ok(())
    }
    async fn before_turn(&self, _: &caliban_agent_core::TurnCtx<'_>) -> Result<()> {
        self.bump();
        Ok(())
    }
    async fn after_turn(
        &self,
        _: &caliban_agent_core::TurnCtx<'_>,
        _: &caliban_agent_core::TurnOutcome,
    ) -> Result<caliban_agent_core::TurnDecision> {
        self.bump();
        Ok(caliban_agent_core::TurnDecision::Continue)
    }
    async fn before_tool(&self, _: &ToolCtx<'_>) -> Result<HookDecision> {
        self.bump();
        Ok(HookDecision::Allow)
    }
    async fn after_tool(
        &self,
        _: &ToolCtx<'_>,
        _: &std::result::Result<
            Vec<caliban_provider::ContentBlock>,
            caliban_agent_core::tool::ToolError,
        >,
    ) -> Result<()> {
        self.bump();
        Ok(())
    }
    async fn session_start(&self, _: &SessionCtx<'_>) -> Result<SessionStartOutcome> {
        self.bump();
        Ok(SessionStartOutcome::default())
    }
    async fn session_end(&self, _: &SessionCtx<'_>, _: &SessionOutcome) -> Result<()> {
        self.bump();
        Ok(())
    }
    async fn user_prompt_submit(&self, _: &PromptCtx<'_>) -> Result<HookDecision> {
        self.bump();
        Ok(HookDecision::Allow)
    }
    async fn pre_compact(&self, _: &CompactCtx<'_>) -> Result<()> {
        self.bump();
        Ok(())
    }
    async fn post_compact(&self, _: &CompactCtx<'_>, _: &CompactOutcome) -> Result<()> {
        self.bump();
        Ok(())
    }
    async fn config_change(&self, _: &ConfigChangeCtx<'_>) -> Result<()> {
        self.bump();
        Ok(())
    }
    async fn cwd_changed(&self, _: &CwdChangedCtx<'_>) -> Result<()> {
        self.bump();
        Ok(())
    }
    async fn file_changed(&self, _: &FileChangedCtx<'_>) -> Result<()> {
        self.bump();
        Ok(())
    }
    async fn permission_request(&self, _: &PermCtx<'_>) -> Result<()> {
        self.bump();
        Ok(())
    }
    async fn permission_denied(&self, _: &PermCtx<'_>) -> Result<()> {
        self.bump();
        Ok(())
    }
    async fn notification(&self, _: &NotificationCtx<'_>) -> Result<()> {
        self.bump();
        Ok(())
    }
    async fn subagent_start(&self, _: &SubagentCtx<'_>) -> Result<()> {
        self.bump();
        Ok(())
    }
    async fn subagent_stop(&self, _: &SubagentCtx<'_>, _: &SubagentOutcome) -> Result<()> {
        self.bump();
        Ok(())
    }
    async fn task_created(&self, _: &TaskCtx<'_>) -> Result<()> {
        self.bump();
        Ok(())
    }
    async fn task_completed(&self, _: &TaskCtx<'_>, _: &TaskOutcome) -> Result<()> {
        self.bump();
        Ok(())
    }
}

#[tokio::test]
async fn composite_all_noop_members_sets_all_noop_true() {
    let composite = CompositeHooks::new(vec![
        Arc::new(NoopHooks) as Arc<dyn Hooks>,
        Arc::new(NoopHooks) as Arc<dyn Hooks>,
        Arc::new(NoopHooks) as Arc<dyn Hooks>,
    ]);
    assert!(composite.all_noop());

    // Empty composite is also all-noop (nothing to call).
    let empty = CompositeHooks::new(vec![]);
    assert!(empty.all_noop());
}

#[tokio::test]
async fn composite_push_non_noop_flips_all_noop_false() {
    let mut composite = CompositeHooks::new(vec![Arc::new(NoopHooks) as Arc<dyn Hooks>]);
    assert!(composite.all_noop());

    // RecorderHooks does not override is_noop -> defaults to false.
    composite.push(Arc::new(RecorderHooks::default()) as Arc<dyn Hooks>);
    assert!(!composite.all_noop());
    assert_eq!(composite.len(), 2);
}

#[tokio::test]
async fn composite_re_adding_noop_after_real_hook_keeps_flag_false() {
    let mut composite = CompositeHooks::new(vec![]);
    assert!(composite.all_noop());

    // Push a real (non-noop) hook -> flag flips false.
    composite.push(Arc::new(RecorderHooks::default()) as Arc<dyn Hooks>);
    assert!(!composite.all_noop());

    // Pushing a NoopHooks afterwards must NOT flip the flag back to true:
    // the composite still contains a real hook that needs dispatching.
    composite.push(Arc::new(NoopHooks) as Arc<dyn Hooks>);
    assert!(!composite.all_noop(), "flag must remain false (monotonic)");
    assert_eq!(composite.len(), 2);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn composite_all_noop_returns_default_without_calling_members() {
    let silent_a = Arc::new(AssertSilentNoop::default());
    let silent_b = Arc::new(AssertSilentNoop::default());
    let composite = CompositeHooks::new(vec![
        Arc::clone(&silent_a) as Arc<dyn Hooks>,
        Arc::clone(&silent_b) as Arc<dyn Hooks>,
    ]);
    assert!(composite.all_noop());

    let cwd = PathBuf::from("/t");
    let cancel = tokio_util::sync::CancellationToken::new();
    let run_ctx = caliban_agent_core::RunCtx {
        session_id: "s",
        workspace_root: &cwd,
        user_message: None,
        prompt_index: 0,
        cancel: cancel.clone(),
    };
    let run_outcome = caliban_agent_core::RunHookOutcome {
        turn_count: 0,
        input_tokens: 0,
        output_tokens: 0,
        success: true,
    };
    composite.before_run(&run_ctx).await.unwrap();
    composite.after_run(&run_ctx, &run_outcome).await.unwrap();

    let cfg = caliban_agent_core::AgentConfig::default();
    let turn_ctx = caliban_agent_core::TurnCtx {
        turn_index: 0,
        messages: &[],
        config: &cfg,
    };
    let turn_outcome = caliban_agent_core::TurnOutcome {
        assistant_message: caliban_provider::Message::user_text(""),
        tool_results: vec![],
        stop_reason: caliban_provider::StopReason::EndTurn,
        usage: caliban_provider::Usage::default(),
        continue_loop: false,
    };
    composite.before_turn(&turn_ctx).await.unwrap();
    composite
        .after_turn(&turn_ctx, &turn_outcome)
        .await
        .unwrap();

    let input = serde_json::json!({});
    let tool_ctx = ToolCtx {
        turn_index: 0,
        tool_use_id: "t",
        tool_name: "Bash",
        input: &input,
        is_read_only: false,
    };
    let bt = composite.before_tool(&tool_ctx).await.unwrap();
    assert!(matches!(bt, HookDecision::Allow));
    let tool_result: std::result::Result<
        Vec<caliban_provider::ContentBlock>,
        caliban_agent_core::tool::ToolError,
    > = Ok(vec![]);
    composite.after_tool(&tool_ctx, &tool_result).await.unwrap();

    let session_ctx = SessionCtx {
        session_id: "s",
        cwd: &cwd,
        provider: "p",
        model: "m",
    };
    let session_outcome = SessionOutcome {
        turn_count: 0,
        input_tokens: 0,
        output_tokens: 0,
    };
    composite.session_start(&session_ctx).await.unwrap();
    composite
        .session_end(&session_ctx, &session_outcome)
        .await
        .unwrap();

    let prompt_ctx = PromptCtx {
        session_id: "s",
        cwd: &cwd,
        turn_index: 0,
        prompt: "hi",
        attachments: &[],
    };
    let ps = composite.user_prompt_submit(&prompt_ctx).await.unwrap();
    assert!(matches!(ps, HookDecision::Allow));

    let compact_ctx = CompactCtx {
        session_id: "s",
        token_count_before: 100,
        strategy: "Noop",
    };
    let compact_outcome = CompactOutcome {
        token_count_after: 100,
        compacted: false,
    };
    composite.pre_compact(&compact_ctx).await.unwrap();
    composite
        .post_compact(&compact_ctx, &compact_outcome)
        .await
        .unwrap();

    let cc_ctx = ConfigChangeCtx {
        changed_keys: &[],
        new_settings_summary: "{}",
    };
    composite.config_change(&cc_ctx).await.unwrap();

    let cwd_ctx = CwdChangedCtx {
        old_cwd: &cwd,
        new_cwd: &cwd,
    };
    composite.cwd_changed(&cwd_ctx).await.unwrap();

    let fc_ctx = FileChangedCtx {
        path: &cwd,
        kind: FileChangeKind::Modified,
        tool: "Write",
    };
    composite.file_changed(&fc_ctx).await.unwrap();

    let perm_ctx = PermCtx {
        turn_index: 0,
        tool_use_id: "t",
        tool_name: "Bash",
        input: &input,
        rule_action: "allow",
        rule_comment: None,
    };
    composite.permission_request(&perm_ctx).await.unwrap();
    composite.permission_denied(&perm_ctx).await.unwrap();

    let notif_ctx = NotificationCtx {
        level: NotificationLevel::Info,
        message: "hi",
    };
    composite.notification(&notif_ctx).await.unwrap();

    let sub_ctx = SubagentCtx {
        parent_turn_index: 0,
        agent_name: "agent",
        task_id: "task",
    };
    let sub_outcome = SubagentOutcome {
        success: true,
        final_text: "done".into(),
    };
    composite.subagent_start(&sub_ctx).await.unwrap();
    composite
        .subagent_stop(&sub_ctx, &sub_outcome)
        .await
        .unwrap();

    let task_ctx = TaskCtx {
        task_id: "t",
        content: "do thing",
        status: "pending",
    };
    let task_outcome = TaskOutcome {
        terminal_status: "completed".into(),
    };
    composite.task_created(&task_ctx).await.unwrap();
    composite
        .task_completed(&task_ctx, &task_outcome)
        .await
        .unwrap();

    // The flag was true, so the composite must have short-circuited every
    // event without ever delegating to either silent wrapper.
    assert_eq!(
        silent_a.invocation_count(),
        0,
        "silent_a should not have been called"
    );
    assert_eq!(
        silent_b.invocation_count(),
        0,
        "silent_b should not have been called"
    );
}

#[tokio::test]
async fn composite_mixed_noop_and_real_calls_the_real_one() {
    // Mixed composition: one NoopHooks + one RecorderHooks. all_noop must be
    // false and the recorder must observe every event the composite fans out.
    let recorder = Arc::new(RecorderHooks::default());
    let composite = CompositeHooks::new(vec![
        Arc::new(NoopHooks) as Arc<dyn Hooks>,
        Arc::clone(&recorder) as Arc<dyn Hooks>,
    ]);
    assert!(
        !composite.all_noop(),
        "mixed composite must not be all-noop"
    );

    let cwd = PathBuf::from("/t");
    let session_ctx = SessionCtx {
        session_id: "S",
        cwd: &cwd,
        provider: "p",
        model: "m",
    };
    composite.session_start(&session_ctx).await.unwrap();

    let notif_ctx = NotificationCtx {
        level: NotificationLevel::Warn,
        message: "test",
    };
    composite.notification(&notif_ctx).await.unwrap();

    let snapshot = recorder.snapshot();
    assert_eq!(
        snapshot,
        vec![
            "session_start:S".to_string(),
            "notification:warn".to_string()
        ],
        "real hook must have been called on every event"
    );
}
