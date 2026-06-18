//! `TuiAskHandler` — bridges `caliban-agent-core`'s `AskHandler` trait into a
//! ratatui modal driven by the TUI event loop.
//!
//! ## Design
//!
//! The agent loop runs in the same tokio runtime as the TUI's event loop;
//! `AskHandler::prompt` is `async` and may park indefinitely. We bridge via
//! an mpsc → oneshot pair:
//!
//! 1. Agent: matched-Ask rule triggers `AskHandler::prompt(...)`.
//! 2. `TuiAskHandler`: builds an `AskRequest { ..., respond: oneshot::Sender }`
//!    and sends it down the mpsc.
//! 3. TUI event loop: drains the mpsc, opens the modal with the request.
//! 4. User: picks an option in the modal; the resolver consumes the
//!    `respond` sender and resolves the oneshot.
//! 5. `TuiAskHandler`: awaits the oneshot and returns a `HookDecision`.
//!
//! Safety: a 10-minute hard timeout on the oneshot resolves to `Deny`. Drop
//! of the sender without a response also resolves to `Deny`.

use std::time::Duration;

use async_trait::async_trait;

/// Hard upper bound on how long we wait for the user to dismiss an Ask
/// modal — matches the longest tool deadline (Bash). After this, the
/// pending request resolves to `Deny`.
#[allow(
    clippy::duration_suboptimal_units,
    reason = "Duration::from_mins is unstable; from_secs(600) keeps the intent legible enough"
)]
const ASK_TIMEOUT: Duration = Duration::from_secs(600);
use caliban_agent_core::{AskHandler, HookDecision, ToolCtx};
use tokio::sync::{mpsc, oneshot};

/// One pending permission prompt waiting on user input.
#[derive(Debug)]
pub(crate) struct AskRequest {
    /// Tool the model is trying to invoke.
    pub(crate) tool_name: String,
    /// Pretty summary of the tool input for display in the modal.
    pub(crate) input_summary: String,
    /// Pattern shown in the "Always" branches so the user knows what
    /// they're committing to. Derived as the narrowest entry produced by
    /// [`derive_suggestions`] — same source of truth used by the
    /// subprompt suggestion list, so the displayed label and the
    /// persisted rule are guaranteed to use the same shape (previously
    /// these were two independent code paths that disagreed on both
    /// shape and key, producing labels like `Edit(/tmp/*)` while saving
    /// `Edit:`).
    pub(crate) always_pattern: String,
    /// Raw tool input value — used by Phase 4 [`derive_suggestions`] to build
    /// the broadest→narrowest suggestion list in the [`AlwaysSubprompt`].
    pub(crate) tool_input: serde_json::Value,
    /// Oneshot to resolve when the user picks an answer.
    pub(crate) respond: oneshot::Sender<AskResponse>,
}

/// User's choice in the Ask modal. Adds the "Always allow / Always
/// reject" branches per the TUI slash UX spec; the event handler turns
/// these into a [`caliban_agent_core::RuntimeRule`] inserted into the
/// session-scoped store before resolving the oneshot.
#[derive(Debug, Clone, Copy)]
pub(crate) enum AskResponse {
    /// Allow this invocation only.
    AllowOnce,
    /// Allow this invocation and append a runtime rule matching the
    /// derived pattern so subsequent matching invocations auto-allow
    /// without prompting.
    AlwaysAllow,
    /// Deny this invocation.
    Deny,
    /// Deny this invocation and append a runtime rule matching the
    /// derived pattern so subsequent matching invocations auto-deny
    /// without prompting.
    AlwaysReject,
}

/// `AskHandler` impl that bridges Ask rules to a ratatui modal via an
/// unbounded mpsc channel. The TUI event loop drains the channel and pumps
/// requests into the modal state.
#[derive(Debug)]
pub(crate) struct TuiAskHandler {
    /// Sender owned by the handler; cloned wherever an `AskHandler` is
    /// needed. The receiver is held by the TUI event loop.
    tx: mpsc::UnboundedSender<AskRequest>,
}

impl TuiAskHandler {
    /// Build the handler + receiver pair. The receiver should be plumbed into
    /// the TUI event loop's `select!` so requests are surfaced as soon as
    /// the agent triggers an Ask.
    pub(crate) fn pair() -> (Self, mpsc::UnboundedReceiver<AskRequest>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }
}

/// Collapse all whitespace runs (including newlines and tabs) into single
/// spaces, trim the ends, and cap to `max` characters. Keeps a multi-line or
/// huge value rendering as one tidy line in the modal header instead of
/// inflating the modal body height (#58).
fn one_line(s: &str, max: usize) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect()
}

/// Format a tool input value for compact, single-line display in the modal
/// header.
fn input_summary(input: &serde_json::Value) -> String {
    use serde_json::Value;
    match input {
        Value::Object(map) => {
            // Prefer the "command" key for Bash; else "path"; else first key.
            for k in ["command", "path", "url", "pattern"] {
                if let Some(v) = map.get(k).and_then(Value::as_str) {
                    return format!("{k}={}", one_line(v, 160));
                }
            }
            let mut parts: Vec<String> = Vec::new();
            for (k, v) in map {
                let s = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                parts.push(format!("{k}={}", one_line(&s, 40)));
            }
            parts.join(", ")
        }
        Value::String(s) => one_line(s, 160),
        other => one_line(&other.to_string(), 160),
    }
}

#[async_trait]
impl AskHandler for TuiAskHandler {
    async fn prompt(&self, ctx: &ToolCtx<'_>) -> HookDecision {
        let (respond_tx, respond_rx) = oneshot::channel();
        let req = AskRequest {
            tool_name: ctx.tool_name.to_string(),
            input_summary: input_summary(ctx.input),
            always_pattern: derive_suggestions(ctx.tool_name, ctx.input)
                .last()
                .cloned()
                .unwrap_or_else(|| ctx.tool_name.to_string()),
            tool_input: ctx.input.clone(),
            respond: respond_tx,
        };
        if self.tx.send(req).is_err() {
            // TUI gone — fall back to Deny, matching CLI behavior.
            return HookDecision::Deny(format!(
                "permission denied for tool '{}': Ask modal unavailable",
                ctx.tool_name
            ));
        }
        // 10-minute hard timeout (matches the longest Bash deadline).
        let result = tokio::time::timeout(ASK_TIMEOUT, respond_rx).await;
        match result {
            Ok(Ok(AskResponse::AllowOnce | AskResponse::AlwaysAllow)) => HookDecision::Allow,
            Ok(Ok(AskResponse::Deny | AskResponse::AlwaysReject) | Err(_)) => {
                HookDecision::Deny(format!("permission denied for tool '{}'", ctx.tool_name))
            }
            Err(_elapsed) => HookDecision::Deny(format!(
                "permission denied for tool '{}': ask modal timed out",
                ctx.tool_name
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// AlwaysSubprompt — state for the always-allow / always-deny sub-prompt
// ---------------------------------------------------------------------------

/// Sub-prompt opened when the operator hits `a` or `d` in the Ask modal.
/// Picks one of the suggested patterns (or a custom one) and a write scope.
///
/// Navigation model: `selected` indexes `0..=suggestions.len()`. The last
/// position (`selected == suggestions.len()`) is the `[custom]` row where
/// the operator can type their own pattern (kept in `custom`). Any other
/// position selects one of the pre-derived suggestions.
#[derive(Debug, Clone)]
pub(crate) struct AlwaysSubprompt {
    /// Suggested patterns, broadest → narrowest. The last one is always
    /// the literal exact-input pattern.
    pub(crate) suggestions: Vec<String>,
    /// Currently-selected row index. Range: `0..=suggestions.len()`.
    /// Initialised to the narrowest real suggestion (last index of
    /// `suggestions`); the `[custom]` row is one past that.
    pub(crate) selected: usize,
    /// When the operator navigated onto the `[custom]` row, the free-form
    /// pattern they're typing accumulates here. `None` until they first
    /// land on the custom row.
    pub(crate) custom: Option<String>,
    /// Live preview: does `selected_pattern()` match the pending input?
    /// Set to `true` by default when exact match is selected; updated by the
    /// render layer once pattern-matching is wired (Phase 5).
    #[allow(dead_code)]
    pub(crate) preview_matches: bool,
    /// Scope picker.
    pub(crate) scope: caliban_settings::Scope,
    /// Optional operator comment.
    pub(crate) comment: String,
    /// Optional deny-only reason (only populated for the deny variant).
    pub(crate) reason: String,
    /// Allow or Deny — set when the sub-prompt was opened.
    pub(crate) action: caliban_agent_core::Action,
}

impl AlwaysSubprompt {
    /// True when the `[custom]` row is the current selection (one past
    /// the end of `suggestions`).
    pub(crate) fn is_custom_selected(&self) -> bool {
        self.selected >= self.suggestions.len()
    }

    /// Selected pattern — either the indexed real suggestion or the
    /// `custom` buffer.
    pub(crate) fn selected_pattern(&self) -> &str {
        if self.is_custom_selected() {
            self.custom.as_deref().unwrap_or("")
        } else {
            &self.suggestions[self.selected]
        }
    }
}

/// Derived suggestions for the sub-prompt. Order: broadest → narrowest.
/// The default selection is the LAST (narrowest) — see `AlwaysSubprompt::selected`.
pub(crate) fn derive_suggestions(tool: &str, input: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    match tool {
        "Bash" => {
            let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let toks: Vec<&str> = cmd.split_whitespace().collect();
            if let Some(first) = toks.first() {
                out.push(format!("Bash:{first} *"));
            }
            if toks.len() >= 2 {
                out.push(format!("Bash:{} {}*", toks[0], toks[1]));
            }
            out.push(format!("Bash:{cmd}")); // exact
        }
        "Edit" | "Read" | "Write" | "MultiEdit" | "NotebookEdit" => {
            // Route through the canonical accessor in caliban-common so
            // the modal, the matcher, and any future caller agree on
            // which JSON key holds the path. Hand-rolled lookups here
            // previously diverged from the real schema (`file_path` vs
            // `path`) and produced unmatchable `Tool:` rules.
            let path = caliban_common::glob_match::first_arg(tool, input).unwrap_or_default();
            if path.is_empty() {
                // Schema lookup failed (real input didn't carry the
                // expected key). Emit only the bare-tool suggestion so
                // the operator can't accidentally commit a broken
                // `Tool:` rule with no args, and log so any future
                // schema drift surfaces in the debug log.
                tracing::warn!(
                    target: "caliban::tui::ask",
                    tool = %tool,
                    "permissions modal: no path arg in file-edit input; falling back to bare-tool suggestion"
                );
                out.push(tool.to_string());
            } else {
                let p = std::path::Path::new(&path);
                if let Some(parent) = p.parent().and_then(|p| p.to_str()) {
                    out.push(format!("{tool}:{parent}/**"));
                    out.push(format!("{tool}:{parent}/*"));
                }
                out.push(format!("{tool}:{path}")); // exact
            }
        }
        other if other.starts_with("mcp__") => {
            out.push(other.to_string());
            if let Some(obj) = input.as_object() {
                for (k, v) in obj.iter().take(2) {
                    if let Some(s) = v.as_str() {
                        out.push(format!("{other}:{k}={s}"));
                    }
                }
            }
        }
        _ => out.push(tool.to_string()),
    }
    out
}

// ---------------------------------------------------------------------------
// render_always_subprompt — ratatui rendering for the sub-prompt modal
// ---------------------------------------------------------------------------

#[allow(
    clippy::too_many_lines,
    reason = "single cohesive ratatui render fn; splitting would scatter the layout"
)]
pub(crate) fn render_always_subprompt(
    f: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    sp: &AlwaysSubprompt,
    tool: &str,
    input_excerpt: &str,
) {
    use caliban_settings::Scope;
    use ratatui::layout::{Constraint, Direction, Layout};
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Borders, Paragraph};

    /// Cap on the pending-call excerpt height (see body below).
    const MAX_EXCERPT_LINES: usize = 6;

    let title = match sp.action {
        caliban_agent_core::Action::Allow => " Always allow ",
        caliban_agent_core::Action::Deny => " Always deny ",
        caliban_agent_core::Action::Ask => " Always ",
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Cap the pending-call excerpt to a few lines so a long, multi-line
    // pasted command can't dominate the layout and squeeze the interactive
    // rows (suggestions / scope / footer) down to their headers (#58).
    let total_excerpt = input_excerpt.lines().count();
    let shown: Vec<&str> = input_excerpt.lines().take(MAX_EXCERPT_LINES).collect();
    let truncated = total_excerpt > shown.len();
    let body_rows = u16::try_from(shown.len() + usize::from(truncated)).unwrap_or(u16::MAX);
    let suggestion_count = u16::try_from(sp.suggestions.len()).unwrap_or(u16::MAX);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2 + body_rows),
            Constraint::Length(1),
            // header + one row per suggestion + custom row + preview row.
            Constraint::Length(suggestion_count + 3),
            Constraint::Length(1),
            Constraint::Length(5), // scope picker (4 options + header)
            Constraint::Length(1),
            Constraint::Length(2), // comment + reason
            Constraint::Min(0),
            Constraint::Length(1), // footer
        ])
        .split(inner);

    // Pending call summary
    let mut summary = vec![Line::from(format!("Pending tool call: {tool}"))];
    for l in &shown {
        summary.push(Line::from(format!("  {l}")));
    }
    if truncated {
        summary.push(Line::from(format!(
            "  … ({} more line(s))",
            total_excerpt - shown.len()
        )));
    }
    f.render_widget(Paragraph::new(summary), chunks[0]);

    // Suggestions — `selected` is the single source of truth for which row
    // is highlighted. The custom row sits one index past `suggestions.len()`.
    let mut suggestion_lines = vec![Line::from(
        "Suggested patterns (↑/↓ to move, type to edit comment / custom pattern):",
    )];
    for (i, p) in sp.suggestions.iter().enumerate() {
        let marker = if i == sp.selected { "(•)" } else { "( )" };
        suggestion_lines.push(Line::from(format!("  {marker} {p}")));
    }
    let custom_marker = if sp.is_custom_selected() {
        "(•)"
    } else {
        "( )"
    };
    suggestion_lines.push(Line::from(format!(
        "  {custom_marker} [custom] {}",
        sp.custom.as_deref().unwrap_or("(empty — type to fill)")
    )));
    let preview = if sp.preview_matches {
        "✓ would match pending input"
    } else {
        "✗ would NOT match pending input"
    };
    suggestion_lines.push(Line::from(Span::styled(
        preview,
        Style::default().fg(if sp.preview_matches {
            Color::Green
        } else {
            Color::Red
        }),
    )));
    f.render_widget(Paragraph::new(suggestion_lines), chunks[2]);

    // Scope picker
    let scopes = [
        (Scope::Cli, "session  (in-memory; gone on restart)"),
        (
            Scope::Project,
            "project  (.caliban/permissions.toml; commit-friendly)",
        ),
        (Scope::User, "user     (~/.config/caliban/permissions.toml)"),
        (
            Scope::Local,
            "local    (.caliban/permissions.local.toml; gitignored)",
        ),
    ];
    let mut scope_lines = vec![Line::from("Save to:")];
    for (scope, label) in scopes {
        let marker = if sp.scope == scope { "(•)" } else { "( )" };
        scope_lines.push(Line::from(format!("  {marker} {label}")));
    }
    f.render_widget(Paragraph::new(scope_lines), chunks[4]);

    // Comment + reason
    let mut cr_lines = vec![Line::from(format!("Comment: {}", sp.comment))];
    if sp.action == caliban_agent_core::Action::Deny {
        cr_lines.push(Line::from(format!("Reason : {}", sp.reason)));
    }
    f.render_widget(Paragraph::new(cr_lines), chunks[6]);

    // Footer
    f.render_widget(
        Paragraph::new(
            "↑/↓ move   [tab] cycle scope   [enter] save   [esc] cancel (allow once, no rule)",
        )
        .style(Style::default().add_modifier(Modifier::REVERSED)),
        chunks[8],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use caliban_agent_core::AskHandler;

    fn ctx<'a>(name: &'a str, input: &'a serde_json::Value) -> ToolCtx<'a> {
        ToolCtx {
            turn_index: 0,
            tool_use_id: "t1",
            tool_name: name,
            input,
            is_read_only: false,
        }
    }

    #[tokio::test]
    async fn allow_once_resolves_to_allow() {
        let (handler, mut rx) = TuiAskHandler::pair();
        let input = serde_json::json!({"command": "ls"});
        // Spawn the modal responder: take the request, AllowOnce.
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.respond.send(AskResponse::AllowOnce);
            }
        });
        let dec = handler.prompt(&ctx("Bash", &input)).await;
        assert!(matches!(dec, HookDecision::Allow));
    }

    #[tokio::test]
    async fn deny_resolves_to_deny() {
        let (handler, mut rx) = TuiAskHandler::pair();
        let input = serde_json::json!({"command": "rm -rf"});
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.respond.send(AskResponse::Deny);
            }
        });
        let dec = handler.prompt(&ctx("Bash", &input)).await;
        assert!(matches!(dec, HookDecision::Deny(_)));
    }

    #[tokio::test]
    async fn dropped_sender_resolves_to_deny() {
        let (handler, mut rx) = TuiAskHandler::pair();
        let input = serde_json::json!({"command": "x"});
        tokio::spawn(async move {
            // Drop without responding — simulates Esc.
            if let Some(req) = rx.recv().await {
                drop(req.respond);
            }
        });
        let dec = handler.prompt(&ctx("Bash", &input)).await;
        assert!(matches!(dec, HookDecision::Deny(_)));
    }

    #[test]
    fn summary_prefers_command_for_bash() {
        let v = serde_json::json!({"command": "git status", "cwd": "/tmp"});
        assert!(input_summary(&v).starts_with("command=git status"));
    }

    #[test]
    fn summary_handles_long_strings() {
        let long = "a".repeat(500);
        let v = serde_json::json!({"command": long});
        assert!(input_summary(&v).len() < 250);
    }

    #[test]
    fn render_always_subprompt_does_not_panic() {
        use ratatui::{Terminal, backend::TestBackend};
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        let sp = AlwaysSubprompt {
            suggestions: vec![
                "Bash:cargo *".into(),
                "Bash:cargo test*".into(),
                "Bash:cargo test --all".into(),
            ],
            selected: 2,
            custom: None,
            preview_matches: true,
            scope: caliban_settings::Scope::Project,
            comment: String::new(),
            reason: String::new(),
            action: caliban_agent_core::Action::Allow,
        };
        term.draw(|f| {
            let area = f.area();
            render_always_subprompt(f, area, &sp, "Bash", "command: cargo test --all");
        })
        .unwrap();
    }

    /// Regression for #58: a long, multi-line input excerpt must not squeeze
    /// the sub-prompt's interactive rows. The footer controls, the selected
    /// suggestion, and the scope options must all stay visible.
    #[test]
    fn always_subprompt_keeps_controls_visible_with_long_input() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut term = Terminal::new(TestBackend::new(80, 40)).unwrap();
        let sp = AlwaysSubprompt {
            suggestions: vec![
                "Bash:cargo *".into(),
                "Bash:cargo test*".into(),
                "Bash:cargo test --all".into(),
            ],
            selected: 2,
            custom: None,
            preview_matches: true,
            scope: caliban_settings::Scope::Project,
            comment: String::new(),
            reason: String::new(),
            action: caliban_agent_core::Action::Allow,
        };
        // A pasted multi-line command far taller than the modal.
        let long_excerpt = (0..80)
            .map(|i| format!("line {i} of a very long pasted command"))
            .collect::<Vec<_>>()
            .join("\n");
        term.draw(|f| {
            let area = f.area();
            render_always_subprompt(f, area, &sp, "Bash", &long_excerpt);
        })
        .unwrap();
        let text: String = term
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        for needle in [
            "[enter] save",          // footer controls
            "Bash:cargo test --all", // the selected suggestion
            "[custom]",              // the custom-pattern row
            "project",               // a scope option
        ] {
            assert!(
                text.contains(needle),
                "sub-prompt must keep {needle:?} visible with long input; rendered:\n{text}"
            );
        }
    }

    #[test]
    fn input_summary_collapses_newlines_to_single_line() {
        let v = serde_json::json!({"command": "echo one\nrm -rf two\nthree"});
        let s = input_summary(&v);
        assert!(
            !s.contains('\n'),
            "input summary must be single-line (newlines collapsed); got: {s:?}"
        );
    }

    #[test]
    fn derive_suggestions_bash_orders_broadest_to_narrowest() {
        let i = serde_json::json!({"command": "cargo test --all"});
        let s = derive_suggestions("Bash", &i);
        assert_eq!(s[0], "Bash:cargo *");
        assert_eq!(s[1], "Bash:cargo test*");
        assert_eq!(s.last().unwrap(), "Bash:cargo test --all");
    }

    #[test]
    fn derive_suggestions_edit_emits_dir_globs_and_exact() {
        // Input shape must match the real Edit schema (`path`), not the
        // historical wrong key (`file_path`).
        let i = serde_json::json!({"path": "/repo/src/foo.rs"});
        let s = derive_suggestions("Edit", &i);
        assert!(s.iter().any(|x| x == "Edit:/repo/src/**"));
        assert!(s.iter().any(|x| x == "Edit:/repo/src/*"));
        assert!(s.last().unwrap().ends_with("foo.rs"));
    }

    #[test]
    fn derive_suggestions_multi_edit_emits_dir_globs_and_exact() {
        // Regression: prior to the schema-key fix, MultiEdit (and
        // NotebookEdit) produced `MultiEdit:` — empty args — which
        // could never match and accumulated as duplicate rules in
        // permissions.toml every time the operator hit "always allow".
        let i = serde_json::json!({"path": "/repo/src/foo.rs", "edits": []});
        let s = derive_suggestions("MultiEdit", &i);
        assert!(s.iter().any(|x| x == "MultiEdit:/repo/src/**"));
        assert!(s.iter().any(|x| x == "MultiEdit:/repo/src/*"));
        assert_eq!(s.last().unwrap(), "MultiEdit:/repo/src/foo.rs");
    }

    #[test]
    fn derive_suggestions_notebook_edit_emits_dir_globs_and_exact() {
        let i = serde_json::json!({"path": "/repo/nb.ipynb", "cell_id": "x", "new_source": ""});
        let s = derive_suggestions("NotebookEdit", &i);
        assert!(s.iter().any(|x| x == "NotebookEdit:/repo/**"));
        assert!(s.iter().any(|x| x == "NotebookEdit:/repo/*"));
        assert_eq!(s.last().unwrap(), "NotebookEdit:/repo/nb.ipynb");
    }

    #[test]
    fn derive_suggestions_falls_back_when_path_arg_missing() {
        // Defensive guard: if a future tool refactor renames the path
        // arg without updating this lookup, we must NOT emit a `Tool:`
        // suggestion with empty args — that's what caused the bug this
        // test is guarding against. Bare-tool fallback is acceptable
        // (operator can still pick `[custom]` for something narrower).
        let i = serde_json::json!({"file_path": "/oops/wrong/key.rs"});
        let s = derive_suggestions("Edit", &i);
        assert_eq!(s, vec!["Edit".to_string()]);
        assert!(
            !s.iter().any(|x| x == "Edit:"),
            "must not emit broken `Edit:` suggestion when path lookup fails"
        );
    }
}
