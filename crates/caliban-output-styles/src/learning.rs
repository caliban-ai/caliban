//! Post-processor for the `Learning` style.
//!
//! Walks the assistant's text and inserts `TODO(human)` markers as comments
//! inside fenced code blocks immediately after each function-definition
//! line. The heuristic is intentionally conservative — we only mark
//! definition lines we can identify by simple lexical patterns. Anything we
//! can't identify is left untouched.
//!
//! Markers the model already emitted on prose lines are preserved verbatim;
//! we do not re-tag them. (A future v2 may add a `<learning-todo>` span for
//! richer TUI highlighting.)

use std::borrow::Cow;

use caliban_agent_core::AssistantPostProcessor;

/// Best-effort `TODO(human)` injector for the Learning style.
///
/// For each fenced code block in `text`, we look for lines that begin a
/// function definition in one of the languages we recognise (Rust, Go,
/// Python, JavaScript/TypeScript, C/C++/Java). When we find one, we
/// emit a `TODO(human): ...` comment line immediately afterward, using
/// the language's comment syntax.
#[derive(Debug, Clone, Default)]
pub struct LearningPostProcessor;

impl LearningPostProcessor {
    /// Construct a new post-processor.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl AssistantPostProcessor for LearningPostProcessor {
    fn process<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let processed = insert_todo_human_markers(text);
        if processed == text {
            Cow::Borrowed(text)
        } else {
            Cow::Owned(processed)
        }
    }
}

/// Inspect `text` (a completed assistant turn) and insert `TODO(human)`
/// markers after each function-definition line inside a fenced code block.
///
/// Returns the original text unchanged when nothing matches.
#[must_use]
pub fn insert_todo_human_markers(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 32);
    let mut in_fence = false;
    let mut fence_lang: Option<String> = None;
    for line in text.split_inclusive('\n') {
        out.push_str(line);

        // Detect fence open / close. The fence marker is "```" optionally
        // followed by a language tag on the same line.
        let trimmed = line.trim_end_matches('\n').trim_start();
        if let Some(rest) = trimmed.strip_prefix("```") {
            if in_fence {
                // closing fence
                in_fence = false;
                fence_lang = None;
            } else {
                in_fence = true;
                let lang = rest.trim().to_string();
                fence_lang = if lang.is_empty() { None } else { Some(lang) };
            }
            continue;
        }

        if !in_fence {
            continue;
        }

        // We are inside a fenced code block. Check whether this line is a
        // function definition we recognise.
        let lang = fence_lang.as_deref().unwrap_or("");
        if is_function_definition(lang, trimmed) {
            // Emit a TODO(human) marker on the next line, using the
            // appropriate comment syntax.
            let comment_prefix = match lang {
                "py" | "python" | "sh" | "bash" | "ruby" | "rb" | "yaml" | "yml" | "toml" => "# ",
                _ => "// ",
            };
            // Preserve the indentation of the function-definition line so the
            // marker lands cleanly inside the body.
            let indent: String = line
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            // Add one extra indent step (4 spaces / 1 tab) so the marker
            // appears inside the function body when the brace-on-same-line
            // convention is used. For languages where the body lives on the
            // next line indented further (Python), the indent below still
            // looks reasonable.
            let extra = if indent.contains('\t') { "\t" } else { "    " };
            out.push_str(&indent);
            out.push_str(extra);
            out.push_str(comment_prefix);
            out.push_str("TODO(human): fill in this implementation\n");
        }
    }

    out
}

/// Cheap lexical check: does `trimmed_line` start a function definition in
/// `lang`?
fn is_function_definition(lang: &str, trimmed_line: &str) -> bool {
    // Strip trailing whitespace so the brace/colon detection is robust.
    let line = trimmed_line.trim_end();
    if line.is_empty() {
        return false;
    }
    match lang {
        // Rust: `fn name(...)` possibly preceded by visibility / `async` /
        // `unsafe` / `const` / `pub(crate)` etc. The line must end with `{`
        // for us to be confident the body opens on the next line.
        "rs" | "rust" => line.contains(" fn ") || line.starts_with("fn ") || line.contains("\tfn "),
        // Go: `func name(...)` or `func (recv T) name(...)`, ending with `{`.
        "go" => line.starts_with("func ") && line.ends_with('{'),
        // Python: `def name(...)` or `async def name(...)`, ending with `:`.
        "py" | "python" => {
            (line.starts_with("def ") || line.starts_with("async def ")) && line.ends_with(':')
        }
        // JavaScript / TypeScript: `function name(...)` or arrow assigned to
        // a `const`/`let`. Conservative — we only catch the `function`
        // keyword form so we don't misfire on object literals.
        "js" | "ts" | "jsx" | "tsx" | "javascript" | "typescript" => {
            (line.starts_with("function ")
                || line.contains(" function ")
                || line.starts_with("async function "))
                && line.ends_with('{')
        }
        // C / C++ / Java: best-effort — a non-comment line ending in `{`
        // that contains `(` and `)` is likely a function head. We require
        // `lang` to be one of these so we don't misfire on Rust blocks.
        "c" | "cpp" | "cc" | "h" | "hpp" | "java" | "kotlin" | "kt" => {
            line.ends_with('{') && line.contains('(') && line.contains(')')
        }
        _ => {
            // Default: only fire on an unambiguous Rust-style `fn` keyword
            // even when no language tag is present, since `fn` is rare
            // enough in prose that false positives are unlikely.
            line.starts_with("fn ") && (line.ends_with('{') || line.ends_with("{ "))
        }
    }
}

/// Identity post-processor — used by all non-Learning styles. Returns the
/// input unchanged via [`Cow::Borrowed`].
#[derive(Debug, Clone, Copy, Default)]
pub struct IdentityPostProcessor;

impl IdentityPostProcessor {
    /// Construct a new identity post-processor.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl AssistantPostProcessor for IdentityPostProcessor {
    fn process<'a>(&self, text: &'a str) -> Cow<'a, str> {
        Cow::Borrowed(text)
    }
}
