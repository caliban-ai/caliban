//! `Ctrl+O` transcript viewer overlay — renders `App.messages` directly
//! (i.e., the model-eye view) and offers a few power-user keys for dumping
//! the transcript to scrollback or piping it to `$VISUAL`.

use std::io::Write as _;
use std::path::PathBuf;

use caliban_provider::{ContentBlock, Message, Role};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::tui::external_editor::{
    AltScreenGuard, EditorLauncher, ExternalEditorError, resolve_editor_argv,
};

/// State held by the open transcript-viewer overlay.
#[derive(Debug, Default)]
pub(crate) struct TranscriptViewerState {
    /// Scroll offset (in formatted line indices).
    pub(crate) scroll: u16,
    /// When `true`, render thinking / tool-input blocks. Default off.
    pub(crate) show_all: bool,
    /// When `true`, draw the `?` key-help footer.
    pub(crate) show_help: bool,
}

impl TranscriptViewerState {
    /// Scroll one row up (toward older messages).
    pub(crate) fn up(&mut self, by: u16) {
        self.scroll = self.scroll.saturating_sub(by);
    }

    /// Scroll one row down (toward newer messages); the caller clamps against
    /// the rendered line count.
    pub(crate) fn down(&mut self, by: u16, max: u16) {
        self.scroll = self.scroll.saturating_add(by).min(max);
    }
}

/// Format the message history for display in the overlay (and the
/// dump-to-scrollback / `$VISUAL` paths). Honors `show_all` — when `false`,
/// `Thinking` and verbose `ToolUse` inputs are hidden to keep the view tight.
#[allow(clippy::too_many_lines)]
pub(crate) fn format_history(messages: &[Message], show_all: bool) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    for m in messages {
        let role_label = match m.role {
            Role::User => Span::styled(
                "user: ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Role::Assistant => Span::styled(
                "assistant: ",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Role::System => Span::styled(
                "system: ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        };
        let mut first = true;
        for cb in &m.content {
            match cb {
                ContentBlock::Text(t) => {
                    for (i, line) in t.text.split('\n').enumerate() {
                        if first && i == 0 {
                            out.push(Line::from(vec![
                                role_label.clone(),
                                Span::raw(line.to_string()),
                            ]));
                        } else {
                            out.push(Line::from(vec![
                                Span::raw("    "),
                                Span::raw(line.to_string()),
                            ]));
                        }
                    }
                    first = false;
                }
                ContentBlock::Thinking(t) => {
                    if !show_all {
                        out.push(Line::styled(
                            "    [thinking hidden — press Ctrl+E to reveal]".to_string(),
                            Style::default().add_modifier(Modifier::DIM | Modifier::ITALIC),
                        ));
                        continue;
                    }
                    for line in t.thinking.split('\n') {
                        out.push(Line::styled(
                            format!("    \u{25B8} {line}"),
                            Style::default().add_modifier(Modifier::DIM | Modifier::ITALIC),
                        ));
                    }
                }
                ContentBlock::ToolUse(u) => {
                    let input_str = serde_json::to_string(&u.input).unwrap_or_default();
                    let summary: String = if show_all {
                        input_str
                    } else {
                        input_str.chars().take(80).collect()
                    };
                    out.push(Line::styled(
                        format!("    \u{1F527} {}({summary})", u.name),
                        Style::default().fg(Color::Yellow),
                    ));
                }
                ContentBlock::ToolResult(r) => {
                    let prefix = if r.is_error { "(error) " } else { "" };
                    let body = r
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text(t) => Some(t.text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    for line in body.split('\n').take(if show_all { usize::MAX } else { 8 }) {
                        out.push(Line::styled(
                            format!("    \u{2192} {prefix}{line}"),
                            Style::default().add_modifier(Modifier::DIM),
                        ));
                    }
                }
                ContentBlock::Image(img) => {
                    let descr = match &img.source {
                        caliban_provider::ImageSource::Base64 { media_type, data } => {
                            format!("[image: media_type={} bytes={}]", media_type, data.len())
                        }
                        caliban_provider::ImageSource::Url { url } => {
                            format!("[image url: {url}]")
                        }
                    };
                    out.push(Line::styled(
                        format!("    {descr}"),
                        Style::default().add_modifier(Modifier::DIM),
                    ));
                }
            }
        }
        out.push(Line::raw(""));
    }
    out
}

/// Render the static key-help footer for the overlay.
pub(crate) fn help_lines() -> Vec<Line<'static>> {
    let style = Style::default().add_modifier(Modifier::DIM);
    [
        " keys:",
        "   j/k or ↑↓     scroll one row",
        "   PgUp/PgDn     scroll one page",
        "   g / G         top / bottom",
        "   Ctrl+E        toggle show-all (thinking + tool inputs)",
        "   [             dump transcript to scrollback (below alt-screen)",
        "   v             open transcript in $VISUAL / $EDITOR",
        "   q, Esc        close",
        "   ?             toggle this help",
    ]
    .iter()
    .map(|s| Line::styled((*s).to_string(), style))
    .collect()
}

/// Write the formatted transcript to `writer` as plain lines (no styling).
/// Used by the dump-to-scrollback path so we can test it without driving the
/// terminal.
///
/// # Errors
/// Returns IO errors from `writer.write_all`.
pub(crate) fn dump_plain<W: std::io::Write>(
    writer: &mut W,
    messages: &[Message],
    show_all: bool,
) -> std::io::Result<()> {
    for line in format_history(messages, show_all) {
        for span in &line.spans {
            writer.write_all(span.content.as_bytes())?;
        }
        writer.write_all(b"\n")?;
    }
    writer.flush()
}

/// Bridge "dump viewport to scrollback" by suspending the alt-screen, writing
/// the formatted history to stdout, then re-entering the alt-screen.
///
/// `suspend`/`resume` are injected so tests can stub the terminal state
/// transitions.
///
/// # Errors
/// Returns the first IO error.
pub(crate) fn dump_to_scrollback<W, S, R>(
    writer: &mut W,
    messages: &[Message],
    show_all: bool,
    suspend: S,
    resume: R,
) -> std::io::Result<()>
where
    W: std::io::Write,
    S: FnOnce() -> std::io::Result<AltScreenGuard>,
    R: FnOnce(AltScreenGuard) -> std::io::Result<()>,
{
    let guard = suspend()?;
    dump_plain(writer, messages, show_all)?;
    writer.flush()?;
    resume(guard)
}

/// Open the formatted transcript in the user's `$VISUAL` / `$EDITOR`. Suspends
/// the alt-screen for the duration of the launch.
///
/// # Errors
/// Returns the appropriate [`ExternalEditorError`] variant.
pub(crate) fn open_in_visual(
    messages: &[Message],
    show_all: bool,
    launcher: &dyn EditorLauncher,
) -> Result<PathBuf, ExternalEditorError> {
    let (program, args) = resolve_editor_argv().ok_or(ExternalEditorError::NoEditor)?;
    open_in_editor(messages, show_all, &program, &args, launcher)
}

/// Pure-function form of [`open_in_visual`] for testing — the editor argv is
/// supplied explicitly rather than read from the environment.
pub(crate) fn open_in_editor(
    messages: &[Message],
    show_all: bool,
    program: &str,
    args: &[String],
    launcher: &dyn EditorLauncher,
) -> Result<PathBuf, ExternalEditorError> {
    let mut tmp = tempfile::Builder::new()
        .prefix("caliban-transcript-")
        .suffix(".md")
        .tempfile()?;
    let mut buf: Vec<u8> = Vec::new();
    dump_plain(&mut buf, messages, show_all)?;
    tmp.write_all(&buf)?;
    tmp.flush()?;
    let path = tmp.path().to_path_buf();
    // Let the launcher take over.
    let _status =
        launcher
            .launch(program, args, &path)
            .map_err(|source| ExternalEditorError::Spawn {
                program: program.to_string(),
                source,
            })?;
    // tmp goes out of scope and is unlinked.
    drop(tmp);
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use caliban_provider::tool::{ToolResultBlock, ToolUseBlock};
    use caliban_provider::{Message, TextBlock};
    use std::path::Path;
    use std::process::ExitStatus;

    fn user(text: &str) -> Message {
        Message::user_text(text)
    }
    fn assistant(text: &str) -> Message {
        Message::assistant_text(text)
    }
    fn tool_use(name: &str, input: serde_json::Value) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse(ToolUseBlock {
                id: "tu1".into(),
                name: name.into(),
                input,
            })],
        }
    }
    fn tool_result(text: &str, is_err: bool) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult(ToolResultBlock {
                tool_use_id: "tu1".into(),
                content: vec![ContentBlock::Text(TextBlock {
                    text: text.into(),
                    cache_control: None,
                })],
                is_error: is_err,
            })],
        }
    }

    #[test]
    fn format_renders_user_and_assistant() {
        let msgs = vec![user("hi"), assistant("hello")];
        let lines = format_history(&msgs, false);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<&str>>()
            .join("|");
        assert!(joined.contains("user:"));
        assert!(joined.contains("hi"));
        assert!(joined.contains("assistant:"));
        assert!(joined.contains("hello"));
    }

    #[test]
    fn format_hides_thinking_when_show_all_false() {
        use caliban_provider::thinking::ThinkingBlock;
        let msgs = vec![Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Thinking(ThinkingBlock {
                    thinking: "secret deliberation".into(),
                    signature: None,
                }),
                ContentBlock::Text(TextBlock {
                    text: "spoken".into(),
                    cache_control: None,
                }),
            ],
        }];
        let hidden = format_history(&msgs, false);
        let joined: String = hidden
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join("|");
        assert!(!joined.contains("secret deliberation"));
        let shown = format_history(&msgs, true);
        let joined_shown: String = shown
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join("|");
        assert!(joined_shown.contains("secret deliberation"));
    }

    #[test]
    fn format_renders_tool_use_and_result() {
        let msgs = vec![
            tool_use("Bash", serde_json::json!({"command": "ls"})),
            tool_result("file1\nfile2", false),
        ];
        let lines = format_history(&msgs, true);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join("|");
        assert!(joined.contains("Bash"));
        assert!(joined.contains("file1"));
    }

    #[test]
    fn dump_plain_emits_visible_text() {
        let msgs = vec![user("the quick brown fox"), assistant("jumps over")];
        let mut buf: Vec<u8> = Vec::new();
        dump_plain(&mut buf, &msgs, false).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("the quick brown fox"));
        assert!(s.contains("jumps over"));
    }

    #[test]
    fn dump_to_scrollback_invokes_suspend_and_resume() {
        let msgs = vec![user("hello scrollback")];
        let mut buf: Vec<u8> = Vec::new();
        let suspend_called = std::cell::Cell::new(false);
        let resume_called = std::cell::Cell::new(false);
        dump_to_scrollback(
            &mut buf,
            &msgs,
            false,
            || {
                suspend_called.set(true);
                Ok(AltScreenGuard)
            },
            |_g| {
                resume_called.set(true);
                Ok(())
            },
        )
        .unwrap();
        assert!(suspend_called.get());
        assert!(resume_called.get());
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("hello scrollback"));
    }

    /// Test launcher for the `v` flow that writes a marker and returns 0.
    struct FixtureEditor;
    impl EditorLauncher for FixtureEditor {
        fn launch(
            &self,
            _p: &str,
            _a: &[String],
            tempfile_path: &Path,
        ) -> Result<ExitStatus, std::io::Error> {
            // Touch the file (already exists) and return success via /bin/sh.
            let _ = tempfile_path.exists();
            #[cfg(unix)]
            {
                std::process::Command::new("/bin/sh")
                    .arg("-c")
                    .arg("exit 0")
                    .status()
            }
            #[cfg(not(unix))]
            {
                std::process::Command::new("cmd")
                    .arg("/C")
                    .arg("exit /b 0")
                    .status()
            }
        }
    }

    #[test]
    fn open_in_editor_creates_tempfile_and_launches() {
        let msgs = vec![user("hi"), assistant("there")];
        let path = open_in_editor(&msgs, false, "echo", &[], &FixtureEditor).unwrap();
        // The launcher returned; the tempfile should have been created
        // (path is no longer guaranteed to exist after `tmp` is dropped).
        assert!(path.to_string_lossy().contains("caliban-transcript-"));
    }

    #[test]
    fn scroll_helpers_clamp_at_zero_and_max() {
        let mut s = TranscriptViewerState::default();
        s.up(5);
        assert_eq!(s.scroll, 0);
        s.down(3, 2);
        assert_eq!(s.scroll, 2);
        s.down(100, 2);
        assert_eq!(s.scroll, 2);
    }

    #[test]
    fn help_lines_mention_all_keys() {
        let lines = help_lines();
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join("\n");
        for needle in ["[", "v", "Esc", "Ctrl+E"] {
            assert!(joined.contains(needle), "help missing: {needle}");
        }
    }
}
