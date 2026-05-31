//! `caliban perms` — permission rule management CLI (Phase 6, ADR 0029).

use crate::args::PermsCommand;

/// Top-level dispatcher for `caliban perms <verb>`.
pub(crate) fn run(cmd: &PermsCommand) -> i32 {
    match cmd {
        PermsCommand::List {
            scope,
            effective,
            json,
        } => cmd_list(scope.as_deref(), *effective, *json),
        PermsCommand::Test { tool, input } => {
            let fallback = serde_json::Value::default();
            cmd_test(tool, input.as_ref().unwrap_or(&fallback))
        }
        PermsCommand::Explain { tool, input } => {
            let fallback = serde_json::Value::default();
            cmd_explain(tool, input.as_ref().unwrap_or(&fallback))
        }
        PermsCommand::Add {
            pattern,
            action,
            scope,
            comment,
            reason,
        } => cmd_add(
            pattern,
            action,
            scope.as_deref(),
            comment.as_deref(),
            reason.as_deref(),
        ),
        PermsCommand::Remove {
            index,
            pattern,
            scope,
        } => cmd_remove(*index, pattern.as_deref(), scope.as_deref()),
        PermsCommand::Import {
            from,
            scope,
            dry_run,
        } => cmd_import(from, scope.as_deref(), *dry_run),
        PermsCommand::Export { scope, format } => cmd_export(scope.as_deref(), format),
        PermsCommand::Audit {
            since,
            tool,
            action,
            head,
        } => cmd_audit(since.as_deref(), tool.as_deref(), action.as_deref(), *head),
        PermsCommand::Lint { scope } => cmd_lint(scope.as_deref()),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Map a scope string to `caliban_settings::Scope`, defaulting to `project`
/// when `scope_str` is `None`.
fn parse_scope(scope_str: Option<&str>) -> Option<caliban_settings::Scope> {
    match scope_str.unwrap_or("project") {
        "managed" => Some(caliban_settings::Scope::Managed),
        "user" => Some(caliban_settings::Scope::User),
        "project" => Some(caliban_settings::Scope::Project),
        "local" => Some(caliban_settings::Scope::Local),
        "cli" => Some(caliban_settings::Scope::Cli),
        other => {
            eprintln!(
                "[caliban perms] unknown scope {other:?}; \
                 expected one of managed/user/project/local/cli"
            );
            None
        }
    }
}

fn action_str(a: caliban_agent_core::Action) -> &'static str {
    match a {
        caliban_agent_core::Action::Allow => "allow",
        caliban_agent_core::Action::Deny => "deny",
        caliban_agent_core::Action::Ask => "ask",
    }
}

// ---------------------------------------------------------------------------
// Task 6.2: list / test / explain
// ---------------------------------------------------------------------------

fn cmd_list(scope: Option<&str>, effective: bool, json: bool) -> i32 {
    let cwd = std::env::current_dir().unwrap_or_default();
    let mut opts = caliban_settings::LoadOptions::new(cwd);
    opts.schema_validate = false;
    // When a specific scope is requested (and not `--effective`), restrict the
    // load to just that scope.
    if !effective && let Some(s_str) = scope {
        let Some(s) = parse_scope(Some(s_str)) else {
            return 1;
        };
        opts.scope_filter = Some(vec![s]);
    }
    let Ok(loaded) = caliban_settings::load_settings(&opts) else {
        eprintln!("[caliban perms] failed to load settings");
        return 1;
    };
    let rules = loaded.settings.permission_rules();
    if json {
        match serde_json::to_string_pretty(&rules) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("[caliban perms] serialization error: {e}");
                return 1;
            }
        }
    } else {
        if rules.is_empty() {
            println!("(no rules)");
        }
        for (i, r) in rules.iter().enumerate() {
            println!("{:3}  {:<5}  {}", i + 1, action_str(r.action), r.tool);
        }
    }
    0
}

fn cmd_test(tool: &str, input: &serde_json::Value) -> i32 {
    let cwd = std::env::current_dir().unwrap_or_default();
    let mut opts = caliban_settings::LoadOptions::new(cwd);
    opts.schema_validate = false;
    let Ok(loaded) = caliban_settings::load_settings(&opts) else {
        eprintln!("[caliban perms] failed to load settings");
        return 1;
    };
    let rules = loaded.settings.permission_rules();
    let ctx = caliban_agent_core::ToolCtx {
        turn_index: 0,
        tool_use_id: "test",
        tool_name: tool,
        input,
    };
    if let Some(r) = caliban_agent_core::evaluate_rules(&rules, &ctx) {
        println!("MATCH: pattern={} action={}", r.tool, action_str(r.action));
        match r.action {
            caliban_agent_core::Action::Allow => 0,
            caliban_agent_core::Action::Deny => 1,
            caliban_agent_core::Action::Ask => 2,
        }
    } else {
        println!("no match — would fall through");
        0
    }
}

fn cmd_explain(tool: &str, input: &serde_json::Value) -> i32 {
    let cwd = std::env::current_dir().unwrap_or_default();
    let mut opts = caliban_settings::LoadOptions::new(cwd);
    opts.schema_validate = false;
    let Ok(loaded) = caliban_settings::load_settings(&opts) else {
        eprintln!("[caliban perms] failed to load settings");
        return 1;
    };
    let rules = loaded.settings.permission_rules();
    let ctx = caliban_agent_core::ToolCtx {
        turn_index: 0,
        tool_use_id: "test",
        tool_name: tool,
        input,
    };
    println!("Rule list (source order; first match wins):");
    for (i, r) in rules.iter().enumerate() {
        let matched = caliban_agent_core::permissions_matcher::matches(&r.tool, &ctx);
        let mark = if matched { "MATCH" } else { "     " };
        println!(
            "  {:3} {} {:<7} {}",
            i + 1,
            mark,
            action_str(r.action),
            r.tool
        );
    }
    0
}

// ---------------------------------------------------------------------------
// Task 6.3: add / remove
// ---------------------------------------------------------------------------

fn cmd_add(
    pattern: &str,
    action: &str,
    scope: Option<&str>,
    comment: Option<&str>,
    reason: Option<&str>,
) -> i32 {
    let Some(s) = parse_scope(scope.or(Some("project"))) else {
        return 1;
    };
    let cwd = std::env::current_dir().unwrap_or_default();
    let Some(target) =
        caliban_settings::scope_path(s, caliban_settings::FileKind::Permissions, &cwd)
    else {
        eprintln!("[caliban perms] no writable path for scope {s:?}");
        return 1;
    };
    let rule = caliban_settings::RuleSpec {
        pattern: pattern.to_owned(),
        action: action.to_owned(),
        comment: comment.map(str::to_owned),
        reason: reason.map(str::to_owned),
        expires_at: None,
        tool: None,
    };
    match caliban_settings::append_rule_to_file(&target, &rule) {
        Ok(()) => {
            println!("added rule to {}", target.display());
            0
        }
        Err(e) => {
            eprintln!("[caliban perms] failed to write rule: {e}");
            1
        }
    }
}

fn cmd_remove(index: Option<usize>, pattern: Option<&str>, scope: Option<&str>) -> i32 {
    let Some(s) = parse_scope(scope.or(Some("project"))) else {
        return 1;
    };
    let cwd = std::env::current_dir().unwrap_or_default();
    let Some(target) =
        caliban_settings::scope_path(s, caliban_settings::FileKind::Permissions, &cwd)
    else {
        eprintln!("[caliban perms] no writable path for scope {s:?}");
        return 1;
    };
    if let Some(pat) = pattern {
        match caliban_settings::delete_rule_at(&target, pat) {
            Ok(true) => {
                println!(
                    "removed rule matching pattern {:?} from {}",
                    pat,
                    target.display()
                );
                0
            }
            Ok(false) => {
                eprintln!("[caliban perms] no matching rule found for pattern {pat:?}");
                1
            }
            Err(e) => {
                eprintln!("[caliban perms] failed to remove rule: {e}");
                1
            }
        }
    } else if index.is_some() {
        // Index-based removal reserved for v3; for v2 require --pattern.
        eprintln!("[caliban perms] --index removal not supported in v2; use --pattern");
        2
    } else {
        eprintln!("[caliban perms] must specify --pattern or --index");
        2
    }
}

// ---------------------------------------------------------------------------
// Task 6.4: import
// ---------------------------------------------------------------------------

fn cmd_import(src: &std::path::Path, scope: Option<&str>, dry_run: bool) -> i32 {
    let Some(s) = parse_scope(scope.or(Some("user"))) else {
        return 1;
    };
    let cwd = std::env::current_dir().unwrap_or_default();
    let Some(dst) = caliban_settings::scope_path(s, caliban_settings::FileKind::Permissions, &cwd)
    else {
        eprintln!("[caliban perms] no writable destination for scope {s:?}");
        return 1;
    };
    if dry_run {
        println!("would import {} -> {}", src.display(), dst.display());
        return 0;
    }
    match caliban_settings::import::import_permissions_to_toml(src, &dst) {
        Ok(n) => {
            println!("imported {n} rule(s) to {}", dst.display());
            0
        }
        Err(e) => {
            eprintln!("[caliban perms] import failed: {e}");
            1
        }
    }
}

// ---------------------------------------------------------------------------
// Task 6.5: export / audit (stub) / lint
// ---------------------------------------------------------------------------

fn cmd_export(scope: Option<&str>, format: &str) -> i32 {
    let Some(s) = parse_scope(scope.or(Some("project"))) else {
        return 1;
    };
    let cwd = std::env::current_dir().unwrap_or_default();
    let mut opts = caliban_settings::LoadOptions::new(cwd);
    opts.scope_filter = Some(vec![s]);
    opts.schema_validate = false;
    let Ok(loaded) = caliban_settings::load_settings(&opts) else {
        eprintln!("[caliban perms] failed to load settings");
        return 1;
    };
    let rules = loaded.settings.permission_rules();
    match format {
        "toml" => {
            for r in &rules {
                println!();
                println!("[[permissions.rules]]");
                println!("pattern = \"{}\"", r.tool.replace('"', "\\\""));
                println!("action  = \"{}\"", action_str(r.action));
            }
            0
        }
        "json" => {
            let by_action = serde_json::json!({
                "permissions": {
                    "allow": rules.iter()
                        .filter(|r| r.action == caliban_agent_core::Action::Allow)
                        .map(|r| r.tool.clone())
                        .collect::<Vec<_>>(),
                    "ask": rules.iter()
                        .filter(|r| r.action == caliban_agent_core::Action::Ask)
                        .map(|r| r.tool.clone())
                        .collect::<Vec<_>>(),
                    "deny": rules.iter()
                        .filter(|r| r.action == caliban_agent_core::Action::Deny)
                        .map(|r| r.tool.clone())
                        .collect::<Vec<_>>(),
                }
            });
            match serde_json::to_string_pretty(&by_action) {
                Ok(s) => {
                    println!("{s}");
                    0
                }
                Err(e) => {
                    eprintln!("[caliban perms] serialization error: {e}");
                    1
                }
            }
        }
        other => {
            eprintln!("[caliban perms] unknown format {other:?}; expected 'toml' or 'json'");
            2
        }
    }
}

/// Read and filter the JSONL audit log.
///
/// Filters by `--since` (RFC3339 timestamp), `--tool` (tool name), `--action`
/// (allow/deny/ask), and `--head` (max lines to print). Prints one line per
/// matching entry in `ts action tool_name input_excerpt` format.
fn cmd_audit(
    since: Option<&str>,
    tool: Option<&str>,
    action: Option<&str>,
    head: Option<usize>,
) -> i32 {
    let Some(path) = caliban_agent_core::decision_log::decision_log_path() else {
        eprintln!("[caliban perms audit] no audit log path available");
        return 1;
    };
    let Ok(body) = std::fs::read_to_string(&path) else {
        println!("(empty)");
        return 0;
    };
    let since_dt = since.and_then(|s| {
        chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.with_timezone(&chrono::Utc))
    });
    let mut count = 0usize;
    for line in body.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if tool.is_some_and(|t| v["tool_name"].as_str() != Some(t)) {
            continue;
        }
        if action.is_some_and(|a| v["action"].as_str() != Some(a)) {
            continue;
        }
        if let Some(s) = since_dt
            && let Some(ts) = v["ts"]
                .as_str()
                .and_then(|x| chrono::DateTime::parse_from_rfc3339(x).ok())
            && ts.with_timezone(&chrono::Utc) < s
        {
            continue;
        }
        println!(
            "{} {} {} {}",
            v["ts"].as_str().unwrap_or(""),
            v["action"].as_str().unwrap_or(""),
            v["tool_name"].as_str().unwrap_or(""),
            v["input_excerpt"].as_str().unwrap_or(""),
        );
        count += 1;
        if head.is_some_and(|h| count >= h) {
            break;
        }
    }
    if count == 0 {
        println!("(empty)");
    }
    0
}

/// Detect duplicate patterns in a scope's permission rules.
fn cmd_lint(scope: Option<&str>) -> i32 {
    let Some(s) = parse_scope(scope.or(Some("project"))) else {
        return 1;
    };
    let cwd = std::env::current_dir().unwrap_or_default();
    let mut opts = caliban_settings::LoadOptions::new(cwd);
    opts.scope_filter = Some(vec![s]);
    opts.schema_validate = false;
    let Ok(loaded) = caliban_settings::load_settings(&opts) else {
        eprintln!("[caliban perms] failed to load settings");
        return 1;
    };
    let rules = loaded.settings.permission_rules();
    let mut seen = std::collections::HashSet::new();
    let mut dupes: usize = 0;
    for r in &rules {
        // Deduplicate on (pattern, action-string) pair; `Action` doesn't impl `Hash`.
        let a = action_str(r.action);
        if !seen.insert((r.tool.clone(), a)) {
            println!("duplicate: pattern={:?} action={a}", r.tool);
            dupes += 1;
        }
    }
    if dupes == 0 {
        println!("OK (no duplicate patterns)");
        0
    } else {
        1
    }
}
