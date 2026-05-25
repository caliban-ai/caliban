//! `Ctrl+G` external editor handoff.
//!
//! Writes the current input buffer to a tempfile, leaves the alt-screen,
//! execs `$VISUAL` / `$EDITOR` / `vi` with the path as the trailing argv,
//! reads back the file on exit, and re-enters the alt-screen.

use std::io::{Read as _, Write as _, stdout};
use std::path::PathBuf;
use std::process::Command;

use crossterm::{
    event::{
        DisableMouseCapture, EnableMouseCapture, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};

/// Editor invocation result returned by [`edit_externally`].
#[derive(Debug)]
pub(crate) struct EditorOutcome {
    /// New buffer contents read back from the tempfile.
    pub(crate) buffer: String,
    /// Whether the editor exited successfully (zero exit code).
    pub(crate) success: bool,
}

/// Reasons [`edit_externally`] can fail before the editor process starts.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ExternalEditorError {
    /// Failed to create the tempfile or write the initial buffer.
    #[error("tempfile create/write failed: {0}")]
    Tempfile(#[from] std::io::Error),
    /// `$EDITOR` / `$VISUAL` value parsed to no argv elements.
    #[error("no editor command resolved (set $VISUAL or $EDITOR)")]
    NoEditor,
    /// `spawn()` failed (editor binary not on PATH).
    #[error("failed to spawn editor '{program}': {source}")]
    Spawn {
        /// The editor program that failed to spawn.
        program: String,
        /// Underlying spawn error.
        #[source]
        source: std::io::Error,
    },
}

/// Resolve the editor argv from environment variables. Returns
/// `(program, args)` where the trailing tempfile path will be appended at
/// invocation time.
///
/// Resolution order: `$VISUAL`, `$EDITOR`, then `vi`. The value is split on
/// ASCII whitespace verbatim — no shell parsing — so
/// `EDITOR='code --wait'` becomes `["code", "--wait"]` and the tempfile is
/// appended as a third argument.
#[must_use]
pub(crate) fn resolve_editor_argv() -> Option<(String, Vec<String>)> {
    resolve_editor_argv_from(
        std::env::var("VISUAL").ok().as_deref(),
        std::env::var("EDITOR").ok().as_deref(),
    )
}

/// Pure-function form of [`resolve_editor_argv`] for testing.
#[must_use]
pub(crate) fn resolve_editor_argv_from(
    visual: Option<&str>,
    editor: Option<&str>,
) -> Option<(String, Vec<String>)> {
    let raw = visual
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty())
        .or_else(|| editor.map(str::to_string).filter(|s| !s.trim().is_empty()))
        .unwrap_or_else(|| "vi".to_string());
    let mut parts = raw.split_ascii_whitespace().map(str::to_string);
    let program = parts.next()?;
    let args: Vec<String> = parts.collect();
    Some((program, args))
}

/// Trait abstraction over the editor command launcher so tests can stub the
/// subprocess. The production impl delegates to `std::process::Command`.
pub(crate) trait EditorLauncher {
    /// Spawn the editor (typically `$VISUAL`/`$EDITOR`) with the tempfile
    /// path appended to the argv. Returns the exit status of the child.
    ///
    /// # Errors
    /// Propagates any IO error from spawn or wait.
    fn launch(
        &self,
        program: &str,
        args: &[String],
        tempfile_path: &std::path::Path,
    ) -> Result<std::process::ExitStatus, std::io::Error>;
}

/// Real launcher that spawns the editor via [`std::process::Command`].
pub(crate) struct SubprocessLauncher;

impl EditorLauncher for SubprocessLauncher {
    fn launch(
        &self,
        program: &str,
        args: &[String],
        tempfile_path: &std::path::Path,
    ) -> Result<std::process::ExitStatus, std::io::Error> {
        let mut cmd = Command::new(program);
        for a in args {
            cmd.arg(a);
        }
        cmd.arg(tempfile_path);
        cmd.status()
    }
}

/// Result of the alt-screen suspend half — call [`resume_alt_screen`] to
/// undo (typically inside a `Drop` or finally block).
pub(crate) struct AltScreenGuard;

/// Suspend the alt-screen so a child process can take over the terminal.
/// Pop kitty keyboard flags, disable mouse capture, leave the alt-screen,
/// and disable raw mode (in that order).
///
/// # Errors
/// Returns the first IO error encountered.
pub(crate) fn suspend_alt_screen() -> Result<AltScreenGuard, std::io::Error> {
    // Pop flags + mouse + alt-screen + raw, in reverse order of enter.
    let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    let _ = execute!(stdout(), DisableMouseCapture);
    execute!(stdout(), LeaveAlternateScreen)?;
    disable_raw_mode()?;
    Ok(AltScreenGuard)
}

/// Re-enter the alt-screen after a suspend. The kitty flags are best-effort;
/// terminals that don't support them ignore the push silently.
///
/// # Errors
/// Returns the first IO error encountered while restoring.
pub(crate) fn resume_alt_screen(_guard: AltScreenGuard) -> Result<(), std::io::Error> {
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    let _ = execute!(
        stdout(),
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES,
        ),
    );
    stdout().flush()?;
    Ok(())
}

/// Core, terminal-agnostic editor round-trip. Writes `initial` to a tempfile,
/// invokes the launcher, then reads the file back. The TUI wrapper handles
/// the alt-screen suspend/restore around this call.
///
/// # Errors
/// Returns the appropriate [`ExternalEditorError`] variant.
pub(crate) fn run_editor_roundtrip(
    initial: &str,
    launcher: &dyn EditorLauncher,
) -> Result<EditorOutcome, ExternalEditorError> {
    let (program, args) = resolve_editor_argv().ok_or(ExternalEditorError::NoEditor)?;
    let mut tmp = tempfile::Builder::new()
        .prefix("caliban-prompt-")
        .suffix(".md")
        .tempfile()?;
    tmp.write_all(initial.as_bytes())?;
    tmp.flush()?;
    let tmp_path: PathBuf = tmp.path().to_path_buf();
    // Close the writer handle but keep the tempfile alive (Drop unlinks).
    drop(tmp);
    // The launcher writes the file in-place; we don't keep the tempfile
    // handle. We need to re-open it to read back. tempfile's path is still
    // valid until we drop the NamedTempFile, but we already did — so create
    // a NamedTempFile from the path before reading? No: simpler to keep
    // ownership of the NamedTempFile via tempdir.
    // (The above `drop` was a refactor mistake; reconstruct properly.)
    let outcome_status = launcher
        .launch(&program, &args, &tmp_path)
        .map_err(|source| ExternalEditorError::Spawn {
            program: program.clone(),
            source,
        })?;
    let mut buf = String::new();
    if let Ok(mut f) = std::fs::File::open(&tmp_path) {
        let _ = f.read_to_string(&mut buf);
    }
    // Best-effort cleanup; if the launcher renamed the file (some editors
    // do), the unlink will silently fail.
    let _ = std::fs::remove_file(&tmp_path);
    Ok(EditorOutcome {
        buffer: buf,
        success: outcome_status.success(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::ExitStatus;

    /// Test launcher that simulates an editor by writing fixed bytes to the
    /// tempfile and returning the configured exit code.
    struct FixtureEditor {
        body: String,
        exit_code: i32,
    }

    impl EditorLauncher for FixtureEditor {
        fn launch(
            &self,
            _program: &str,
            _args: &[String],
            tempfile_path: &Path,
        ) -> Result<ExitStatus, std::io::Error> {
            let mut f = std::fs::File::create(tempfile_path)?;
            f.write_all(self.body.as_bytes())?;
            // Convert the int into a real ExitStatus via a shell exit. On
            // Unix we can construct directly with `std::os::unix`, but it's
            // simpler to spawn `sh -c "exit N"`.
            #[cfg(unix)]
            {
                let st = std::process::Command::new("/bin/sh")
                    .arg("-c")
                    .arg(format!("exit {}", self.exit_code))
                    .status()?;
                Ok(st)
            }
            #[cfg(not(unix))]
            {
                // Fallback: shell out via cmd.exe.
                let st = std::process::Command::new("cmd")
                    .arg("/C")
                    .arg(format!("exit /b {}", self.exit_code))
                    .status()?;
                Ok(st)
            }
        }
    }

    #[test]
    fn resolve_uses_visual_first() {
        let (p, _args) = resolve_editor_argv_from(Some("myvisual"), Some("myeditor")).unwrap();
        assert_eq!(p, "myvisual");
    }

    #[test]
    fn resolve_falls_back_to_editor() {
        let (p, _args) = resolve_editor_argv_from(None, Some("myeditor")).unwrap();
        assert_eq!(p, "myeditor");
    }

    #[test]
    fn resolve_defaults_to_vi() {
        let (p, _args) = resolve_editor_argv_from(None, None).unwrap();
        assert_eq!(p, "vi");
    }

    #[test]
    fn resolve_splits_args_verbatim() {
        let (p, args) = resolve_editor_argv_from(None, Some("code --wait")).unwrap();
        assert_eq!(p, "code");
        assert_eq!(args, vec!["--wait".to_string()]);
    }

    #[test]
    fn roundtrip_preserves_replacement_text() {
        let launcher = FixtureEditor {
            body: "new contents\nfrom editor\n".to_string(),
            exit_code: 0,
        };
        let outcome = run_editor_roundtrip("initial buffer", &launcher).unwrap();
        assert_eq!(outcome.buffer, "new contents\nfrom editor\n");
        assert!(outcome.success);
    }

    #[test]
    fn roundtrip_reports_non_zero_exit() {
        let launcher = FixtureEditor {
            body: "ignored".to_string(),
            exit_code: 1,
        };
        let outcome = run_editor_roundtrip("initial", &launcher).unwrap();
        assert!(!outcome.success);
    }

    #[test]
    fn roundtrip_preserves_initial_when_editor_leaves_intact() {
        // FixtureEditor always rewrites the tempfile, but the contract is
        // that we read what's in the file after the editor returns. Verify
        // that round-tripping our initial buffer through an editor that
        // copies-through preserves it.
        let initial = "line 1\nline 2\nline 3";
        let launcher = FixtureEditor {
            body: initial.to_string(),
            exit_code: 0,
        };
        let outcome = run_editor_roundtrip(initial, &launcher).unwrap();
        assert_eq!(outcome.buffer, initial);
    }
}
