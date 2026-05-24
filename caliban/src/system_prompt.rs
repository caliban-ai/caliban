//! Default system prompt + override resolution.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::Context;
use caliban_agent_core::{Todo, TodoStatus};

/// Build the default system prompt from current state.
#[must_use]
pub(crate) fn build_default(cwd: &Path, tool_names: &[&str], no_tools: bool) -> String {
    let cwd_str = cwd.display();

    let tools_section = if no_tools {
        "Tools are disabled for this session.".to_string()
    } else {
        let mut s = String::from("You have access to these tools:\n");
        for name in tool_names {
            let desc = match *name {
                "Read" => "- Read(path, [limit, offset]) — read text files (max 5MB, line-indexed)\n".to_string(),
                "Write" => "- Write(path, content) — create or overwrite files (auto-creates parents)\n".to_string(),
                "Edit" => "- Edit(path, old_string, new_string, [replace_all]) — string replacement in files\n".to_string(),
                "Bash" => "- Bash(command, [timeout_seconds, cwd]) — execute /bin/sh -c \"...\"; captures stdout/stderr\n".to_string(),
                "Glob" => "- Glob(pattern, [path]) — find files matching a glob (.gitignore-aware)\n".to_string(),
                "Grep" => "- Grep(pattern, [path, include, max_matches]) — ripgrep-style content search\n".to_string(),
                other => format!("- {other}\n"),
            };
            s.push_str(&desc);
        }
        s
    };

    format!(
        "You are caliban, an agentic command-line assistant running inside the caliban harness \
        (a from-scratch Rust replacement for Claude Code).\n\
        \n\
        You are operating in the following directory:\n  {cwd_str}\n\
        \n\
        {tools_section}\
        \n\
        Conventions:\n\
        - Use tools when needed; don't claim to have read files you haven't actually Read.\n\
        - File paths can be relative to the working directory above, or absolute.\n\
        - Path arguments to tools also support `~` and `~/...` for the home directory.\n\
        - Bash commands run with /bin/sh -c and timeout after 60s by default.\n\
        - Output is rendered in a terminal UI; prefer concise responses with code blocks for \
        multi-line content rather than long prose paragraphs.\n\
        - When the user asks you to modify a file, Read it first so your edits are accurate.\n\
        \n\
        Ask before destructive operations (rm -rf, force-pushing git, dropping database tables, etc.).\n"
    )
}

/// Resolve the system prompt to use based on CLI args.
///
/// Precedence: `--system` > `--system-file` > default. `--no-system` returns Ok(None).
///
/// # Errors
/// Returns an error if `--system-file` is given but cannot be read.
pub(crate) fn resolve(
    system: Option<&str>,
    system_file: Option<&Path>,
    no_system: bool,
    cwd: &Path,
    tool_names: &[&str],
    no_tools: bool,
) -> anyhow::Result<Option<String>> {
    if no_system {
        return Ok(None);
    }
    if let Some(text) = system {
        return Ok(Some(text.to_string()));
    }
    if let Some(path) = system_file {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading system prompt from {}", path.display()))?;
        return Ok(Some(text));
    }
    Ok(Some(build_default(cwd, tool_names, no_tools)))
}

/// Append a `--- Current todos ---` block to the system prompt when the list
/// is non-empty. Returns the original prompt unchanged when `todos` is empty.
///
/// Status glyphs: `[ ]` pending, `[~]` in-progress, `[x]` completed,
/// `[-]` cancelled.
#[must_use]
pub(crate) fn append_todo_block(prompt: &str, todos: &[Todo]) -> String {
    if todos.is_empty() {
        return prompt.to_string();
    }
    let mut out = String::with_capacity(prompt.len() + 64 + todos.len() * 40);
    out.push_str(prompt);
    if !prompt.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("\n--- Current todos ---\n");
    for t in todos {
        let glyph = match t.status {
            TodoStatus::Pending => "[ ]",
            TodoStatus::InProgress => "[~]",
            TodoStatus::Completed => "[x]",
            TodoStatus::Cancelled => "[-]",
        };
        let _ = writeln!(out, "{glyph} ({}) {}", t.id, t.content);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_prompt_omits_todo_block_when_empty() {
        let base = "You are caliban.\n";
        let out = append_todo_block(base, &[]);
        assert_eq!(out, base);
    }

    #[test]
    fn system_prompt_appends_todo_block_when_non_empty() {
        let base = "You are caliban.\n";
        let todos = vec![
            Todo {
                id: "1".into(),
                content: "first".into(),
                status: TodoStatus::Pending,
            },
            Todo {
                id: "2".into(),
                content: "second".into(),
                status: TodoStatus::InProgress,
            },
            Todo {
                id: "3".into(),
                content: "third".into(),
                status: TodoStatus::Completed,
            },
            Todo {
                id: "4".into(),
                content: "fourth".into(),
                status: TodoStatus::Cancelled,
            },
        ];
        let out = append_todo_block(base, &todos);
        assert!(out.contains("--- Current todos ---"));
        assert!(out.contains("[ ] (1) first"));
        assert!(out.contains("[~] (2) second"));
        assert!(out.contains("[x] (3) third"));
        assert!(out.contains("[-] (4) fourth"));
    }

    #[test]
    fn appends_newline_if_prompt_missing_trailing_nl() {
        let base = "no trailing nl";
        let todos = vec![Todo {
            id: "1".into(),
            content: "x".into(),
            status: TodoStatus::Pending,
        }];
        let out = append_todo_block(base, &todos);
        assert!(out.starts_with("no trailing nl\n\n--- Current todos ---\n"));
    }
}
