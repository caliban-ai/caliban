//! Default system prompt + override resolution.

use std::path::Path;

use anyhow::Context;

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
