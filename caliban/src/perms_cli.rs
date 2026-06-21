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

/// Append the built-in `default_rules()` tail to a config rule list, mirroring
/// the runtime gate (`startup.rs`). The predictors (`test`/`explain`, and
/// `list --effective`) must evaluate the same list the runtime does, otherwise
/// a tool covered only by the defaults (e.g. `Bash` under a Read-only config)
/// is mispredicted as "no match" when the runtime would `Ask` (#179).
fn with_default_rules(mut rules: Vec<caliban_agent_core::Rule>) -> Vec<caliban_agent_core::Rule> {
    rules.extend(caliban_agent_core::default_rules());
    rules
}

/// Render a lossless, order-preserving export of `rules` in `toml` or `json`.
/// Both formats emit the canonical `permissions.rules` ordered array
/// (`RuleSpec`), preserving `comment`/`reason`/`expires_at` and source order so
/// export→import round-trips (#179). Returns `Err` for an unknown format.
fn render_export(rules: &[caliban_agent_core::Rule], format: &str) -> Result<String, String> {
    let specs: Vec<caliban_settings::RuleSpec> = rules
        .iter()
        .map(|r| caliban_settings::RuleSpec {
            pattern: r.tool.clone(),
            action: action_str(r.action).to_owned(),
            comment: r.comment.clone(),
            reason: r.reason.clone(),
            expires_at: r.expires_at,
            tool: None,
        })
        .collect();
    match format {
        "toml" => {
            use std::fmt::Write as _;
            let mut out = String::new();
            for s in &specs {
                out.push_str("\n[[permissions.rules]]\n");
                let _ = writeln!(out, "pattern = {}", toml_quote(&s.pattern));
                let _ = writeln!(out, "action  = {}", toml_quote(&s.action));
                if let Some(c) = &s.comment {
                    let _ = writeln!(out, "comment = {}", toml_quote(c));
                }
                if let Some(r) = &s.reason {
                    let _ = writeln!(out, "reason  = {}", toml_quote(r));
                }
                if let Some(e) = &s.expires_at {
                    let _ = writeln!(out, "expires_at = {}", toml_quote(&e.to_rfc3339()));
                }
            }
            Ok(out)
        }
        "json" => {
            let root = serde_json::json!({ "permissions": { "rules": specs } });
            serde_json::to_string_pretty(&root).map_err(|e| format!("serialization error: {e}"))
        }
        other => Err(format!(
            "unknown format {other:?}; expected 'toml' or 'json'"
        )),
    }
}

/// Quote and escape a string as a TOML basic string.
fn toml_quote(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
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
    // `--effective` mirrors the runtime gate, which appends default_rules().
    let rules = if effective {
        with_default_rules(loaded.settings.permission_rules())
    } else {
        loaded.settings.permission_rules()
    };
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
    let rules = with_default_rules(loaded.settings.permission_rules());
    let ctx = caliban_agent_core::ToolCtx {
        session_id: "",
        turn_index: 0,
        tool_use_id: "test",
        tool_name: tool,
        input,
        // `caliban perms` evaluates static rules; plan-mode gating (the only
        // consumer of is_read_only) is not in play here.
        is_read_only: false,
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
    let rules = with_default_rules(loaded.settings.permission_rules());
    let ctx = caliban_agent_core::ToolCtx {
        session_id: "",
        turn_index: 0,
        tool_use_id: "test",
        tool_name: tool,
        input,
        // `caliban perms` evaluates static rules; plan-mode gating (the only
        // consumer of is_read_only) is not in play here.
        is_read_only: false,
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
    match render_export(&rules, format) {
        Ok(s) => {
            println!("{s}");
            0
        }
        Err(e) if e.starts_with("unknown format") => {
            eprintln!("[caliban perms] {e}");
            2
        }
        Err(e) => {
            eprintln!("[caliban perms] {e}");
            1
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

#[cfg(test)]
mod tests {
    use super::*;
    use caliban_agent_core::{Action, Rule};

    fn rule(tool: &str, action: Action) -> Rule {
        Rule {
            tool: tool.into(),
            action,
            comment: None,
            reason: None,
            expires_at: None,
        }
    }

    #[test]
    fn predictor_appends_default_rules_so_bash_predicts_ask() {
        // #179: a Read-only config must still predict the runtime decision for
        // an uncovered tool — the default_rules() `*` catch-all Asks.
        let rules = with_default_rules(vec![rule("Read", Action::Allow)]);
        let ctx = caliban_agent_core::ToolCtx {
            session_id: "test-session",
            turn_index: 0,
            tool_use_id: "t",
            tool_name: "Bash",
            input: &serde_json::json!({"command": "ls"}),
            is_read_only: false,
        };
        let matched =
            caliban_agent_core::evaluate_rules(&rules, &ctx).expect("default catch-all must match");
        assert_eq!(
            matched.action,
            Action::Ask,
            "Bash under a Read-only config should predict Ask, not fall through"
        );
    }

    fn sample_rules() -> Vec<Rule> {
        vec![
            Rule {
                tool: "Bash:rm *".into(),
                action: Action::Deny,
                comment: Some("dangerous".into()),
                reason: Some("no destructive shell".into()),
                expires_at: None,
            },
            Rule {
                tool: "Read".into(),
                action: Action::Allow,
                comment: None,
                reason: None,
                expires_at: Some(
                    chrono::DateTime::parse_from_rfc3339("2030-01-02T03:04:05Z")
                        .unwrap()
                        .with_timezone(&chrono::Utc),
                ),
            },
        ]
    }

    #[derive(serde::Deserialize)]
    struct Root {
        permissions: caliban_settings::Permissions,
    }

    #[test]
    fn export_json_roundtrips_fields_and_order() {
        let out = render_export(&sample_rules(), "json").unwrap();
        let root: Root = serde_json::from_str(&out).unwrap();
        let specs = &root.permissions.rules;
        assert_eq!(specs.len(), 2);
        // Order preserved: Deny before Allow.
        assert_eq!(specs[0].pattern, "Bash:rm *");
        assert_eq!(specs[0].action, "deny");
        assert_eq!(specs[0].comment.as_deref(), Some("dangerous"));
        assert_eq!(specs[0].reason.as_deref(), Some("no destructive shell"));
        assert_eq!(specs[1].pattern, "Read");
        assert!(specs[1].expires_at.is_some(), "expires_at must survive");
    }

    #[test]
    fn export_toml_roundtrips_fields_and_order() {
        let out = render_export(&sample_rules(), "toml").unwrap();
        let root: Root = toml::from_str(&out).unwrap();
        let specs = &root.permissions.rules;
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].pattern, "Bash:rm *");
        assert_eq!(specs[0].action, "deny");
        assert_eq!(specs[0].comment.as_deref(), Some("dangerous"));
        assert_eq!(specs[0].reason.as_deref(), Some("no destructive shell"));
        assert_eq!(specs[1].pattern, "Read");
        assert_eq!(
            specs[1].expires_at.unwrap().to_rfc3339(),
            "2030-01-02T03:04:05+00:00"
        );
    }

    #[test]
    fn export_unknown_format_errs() {
        assert!(render_export(&[], "yaml").is_err());
    }
}
