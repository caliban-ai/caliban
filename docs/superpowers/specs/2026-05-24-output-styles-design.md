# Output styles — Design

**Date:** 2026-05-24
**Status:** Proposed
**Author:** john.ford2002@gmail.com
**Sub-project of:** caliban Rust agent harness
**ADR:** `adrs/0031-output-styles.md`
**Depends on:** `caliban-memory::MemoryPrefix::splice_into` (the splice
pattern is reused), `caliban-skills` (frontmatter parser reused via
`serde_yaml`), `docs/superpowers/specs/2026-05-24-plugin-system-design.md`
(plugin-supplied styles).

## Goal

Ship the four built-in output styles Claude Code provides — `Default`,
`Proactive`, `Explanatory`, `Learning` — plus a custom-style file format
that operators (and plugins) can drop in. An output style splices a
short prompt block into the system prompt to nudge the model toward a
particular mode of response (explanatory commentary, learning-paced
prompts with `TODO(human)` markers, etc.) without touching tools,
permissions, hooks, or messages. Activation is via `/config → Output
style` or the `output_style` setting; switching takes effect after
`/clear` or restart, because system prompts are provider-cached.

## Non-goals

- **Mid-session restyling.** Switching the style mid-session does not
  reissue the cached prompt; operators must `/clear` or restart. We
  surface this in the `/config` overlay rather than silently re-warm
  the cache.
- **Per-turn or per-tool styles.** Output style is global to the
  session.
- **Style composition.** Exactly one style is active. A plugin style
  with `force_for_plugin: true` wins while that plugin is enabled, but
  styles do not stack.
- **Direct manipulation of message formatting.** Styles affect prompt
  text only; they don't intercept assistant text, except for the
  `Learning` style's `TODO(human)` post-processor (covered below).
- **Streaming-time style mutations.** No SSE / streaming inspection;
  the `TODO(human)` post-processor runs on completed assistant turns.

## Architecture

```
build_system_prompt()
  ┌───────────────────────────────────────┐
  │ MemoryPrefix::splice_into(default)    │  (existing; tier blocks)
  │   <global-claude-md>…</global-claude-md>
  │   <project-claude-md>…</project-claude-md>
  │   <auto-memory-index>…</auto-memory-index>
  │ + default system body                 │
  └─────────────┬─────────────────────────┘
                │
                ▼
  OutputStylePrefix::splice_into(prompt)
                │
                ▼  "<output-style name=\"learning\">…</output-style>\n\n<base prompt>"
  final system prompt → provider

post-process loop:
  agent_core::process_assistant_text(text)
    ├─ if style == Learning: insert_todo_human_markers(text)
    └─ otherwise: identity
```

Output styles live in a new crate `caliban-output-styles`, modeled on
`caliban-skills` (same loader pattern, same frontmatter parsing) and
`caliban-memory::MemoryPrefix` (same splice-into-default-body shape).

## Crate structure (delta)

```
crates/caliban-output-styles/        # NEW
├── Cargo.toml
└── src/
    ├── lib.rs              # re-exports
    ├── style.rs            # OutputStyle struct + Frontmatter
    ├── prefix.rs           # OutputStylePrefix::splice_into
    ├── loader.rs           # discovery roots + parse
    ├── builtins/           # embedded built-in styles
    │   ├── default.md
    │   ├── proactive.md
    │   ├── explanatory.md
    │   └── learning.md
    └── learning.rs         # TODO(human) post-processor

crates/caliban-agent-core/src/lib.rs
  + pub trait AssistantPostProcessor (style-driven text mutation)
  + Agent::with_post_processor

caliban/src/main.rs
  + select active style; construct OutputStylePrefix; apply post-processor
caliban/src/tui_overlay_config.rs
  + Output Style picker in /config
```

The built-in style bodies live as `include_str!`'d markdown beside the
loader. Each is short (≤80 lines). They are not concatenated into a
single Rust string literal — keeping them as files lets operators copy
one out as a starting point for their own custom style.

## Built-in styles

Four styles ship embedded via `include_str!`. The bodies are *not*
fully written out in this spec; they live at:

- `crates/caliban-output-styles/src/builtins/default.md`
- `crates/caliban-output-styles/src/builtins/proactive.md`
- `crates/caliban-output-styles/src/builtins/explanatory.md`
- `crates/caliban-output-styles/src/builtins/learning.md`

Each file is the same shape as a custom-style file (frontmatter + body).
A sketch of each body:

| Style | Body sketch (one-line intent) |
|---|---|
| `default` | (empty body — no style block is spliced; this is the no-op) |
| `proactive` | "Take initiative. Surface adjacent issues you notice. Suggest follow-ups." |
| `explanatory` | "When you make a change, briefly explain *why*. Reference the standards or patterns you're following." |
| `learning` | "When a non-trivial decision arises, insert `TODO(human): <prompt>` so the user can fill it in. Keep code as scaffolding, not finished." |

The `default` style returns an *empty* prefix — no `<output-style>` block
is emitted. This avoids the prompt-cache invalidation that switching
between "no style" and "some style" would otherwise cause.

## Custom-style file format

A custom style is a single markdown file with YAML frontmatter:

```markdown
---
name: tightlipped
description: "Minimal responses; only essential commentary."
keep_coding_instructions: true
force_for_plugin: false
---

You respond with minimum prose. Output code blocks first, with at most
one sentence of context per block. Never restate the user's request.
```

### Frontmatter fields

| Field | Type | Required | Default | Notes |
|---|---|---|---|---|
| `name` | string | yes | — | Lowercase, `[a-z0-9_-]+`. Must match the filename stem (`tightlipped.md` → `name: tightlipped`). |
| `description` | string | yes | — | Surfaced in the `/config → Output style` picker. |
| `keep_coding_instructions` | bool | no | `true` | When `false`, caliban drops the "you may use tools" / "ask before destructive ops" lines from the base system prompt. Lets a "documentation-only" style suppress code-tool framing. |
| `force_for_plugin` | bool | no | `false` | Plugin-only knob. When a plugin-supplied style has this set and the plugin is enabled, it overrides the operator's `output_style` setting until the plugin is disabled. Sideloaded styles ignore this field. |

Unknown frontmatter fields are preserved into a `serde_yaml::Mapping` for
forward-compat (mirroring `Skill::metadata`).

## Discovery roots

```
1. <workspace_root>/.caliban/output-styles/<name>.md           (project)
2. $XDG_CONFIG_HOME/caliban/output-styles/<name>.md            (user)
3. <plugin>/output-styles/<name>.md                            (plugin, namespaced)
4. embedded built-ins                                          (always present)
```

Priority is project > user > plugin > built-in. A plugin style is named
`<plugin>:<style-name>` in the picker and as the value for
`output_style` (e.g. `output_style = "superpowers:learning"`); built-in
and bare-named user/project styles use the bare name. A style with
`force_for_plugin: true` (and the plugin in `plugins.enabled`) overrides
this resolution while it's active; the `/config` picker surfaces a
"locked by plugin: X" badge.

## Splice into the system prompt

Splicing reuses the `MemoryPrefix::splice_into` pattern. The output
style block goes *after* memory tiers and *before* the base body — so
memory still gets the highest precedence in the cache key, but the
style block is part of the prompt the model actually sees first.

```rust
// caliban-output-styles/src/prefix.rs

pub struct OutputStylePrefix {
    pub active: Option<OutputStyle>,   // None == Default == no block
}

impl OutputStylePrefix {
    /// Wrap the active style's body in `<output-style name="…">` and
    /// prepend to `base`. When `active` is `None` (Default style),
    /// returns `base` unchanged.
    pub fn splice_into(&self, base: &str) -> String { /* … */ }
}
```

The XML tag used is `<output-style name="…">…</output-style>` (mirroring
`<global-claude-md path="…">` for symmetry with memory). The
`MemoryPrefix` block is emitted first, then `OutputStylePrefix`, then
the default body. Concretely:

```
<global-claude-md path="...">…</global-claude-md>

<project-claude-md path="...">…</project-claude-md>

<auto-memory-index path="...">…</auto-memory-index>

<output-style name="learning">
… style body …
</output-style>

You are caliban, an agentic command-line assistant …
```

The two prefixes compose: `caliban/src/system_prompt.rs::build_default`
returns the base body, the memory prefix wraps it, and the output-style
prefix wraps that.

## `Learning` style — `TODO(human)` post-processor

The `Learning` style is the only style that touches assistant output.
After each assistant turn completes, a post-processor runs:

```rust
// caliban-output-styles/src/learning.rs

pub fn insert_todo_human_markers(text: &str) -> String;
```

The post-processor scans the assistant's text for `TODO(human)` markers
the model emitted (per its instructions) and *preserves* them verbatim;
when the model emits a `TODO(human): <prompt>` line, the TUI renders it
with a highlighted background so the operator can find the holes.

The hook attaches via the `AssistantPostProcessor` trait added to
`caliban-agent-core`:

```rust
// caliban-agent-core
pub trait AssistantPostProcessor: Send + Sync {
    /// Mutate (or pass through) the final text of an assistant turn.
    /// Called once per assistant message after streaming completes.
    fn process(&self, text: &str) -> String;
}
```

`Default`, `Proactive`, `Explanatory` all install an identity
post-processor. The TUI highlight is implemented by post-processor
*tagging* (wrap `TODO(human): …` lines in a `<learning-todo>` span the
TUI renders specially); the agent core itself doesn't render anything.

## Settings keys

```toml
# ~/.config/caliban/settings.toml or .caliban/settings.toml

# String value: name of the active output style. Default style is the
# no-op. Plugin-supplied styles use "<plugin>:<style>" form.
output_style = "default"
```

Mid-session changes via `/config` write to the project settings file
and update the in-memory `OutputStylePrefix`; the *next* full session
restart picks up the new system prompt. The `/config` overlay surfaces
a "applies after /clear or restart" hint so operators aren't surprised.

## `/config → Output style` overlay

```
┌─ Output Style ──────────────────────────────────────────────────┐
│ ● default           No commentary; minimal preamble.            │
│ ○ proactive         Surface adjacent issues; suggest follow-ups.│
│ ○ explanatory       Explain the why behind each change.         │
│ ○ learning          Insert TODO(human) markers; scaffold only.  │
│ ○ tightlipped       (user) Minimal responses; essentials only.  │
│ ○ superpowers:learning (plugin, locked while enabled)           │
└─────────────────────────────────────────────────────────────────┘
[esc] close   [↑/↓] navigate   [enter] select
note: takes effect after /clear or restart
```

The `(locked while enabled)` badge appears for plugin styles with
`force_for_plugin: true` whose plugin is enabled — selecting any other
style shows a confirmation that the plugin override remains active.

## Public API sketches

```rust
// caliban-output-styles/src/lib.rs

pub use loader::{default_roots, load_styles, load_one};
pub use prefix::OutputStylePrefix;
pub use style::{OutputStyle, Frontmatter};
pub use learning::insert_todo_human_markers;

// caliban-output-styles/src/style.rs

#[derive(Debug, Clone)]
pub struct OutputStyle {
    pub name: String,                          // "learning" or "<plugin>:learning"
    pub description: String,
    pub body: String,                          // empty for the Default style
    pub keep_coding_instructions: bool,
    pub force_for_plugin: bool,
    pub source: OutputStyleSource,             // BuiltIn | User | Project | Plugin{plugin_name}
}

pub enum OutputStyleSource {
    BuiltIn,
    User    { path: PathBuf },
    Project { path: PathBuf },
    Plugin  { plugin_name: String, path: PathBuf },
}

// caliban-output-styles/src/loader.rs

pub fn default_roots(workspace_root: &Path) -> Vec<PathBuf>;

pub fn load_styles(roots: &[PathBuf], plugin_roots: &[(String, PathBuf)]) -> Vec<OutputStyle>;

pub fn load_one(path: &Path) -> Result<OutputStyle, OutputStyleError>;

/// Selects the active style based on settings + plugin force flags.
pub fn select_active(
    all: &[OutputStyle],
    requested: &str,
    enabled_plugins: &[String],
) -> Option<OutputStyle>;
```

## Tests

1. **`default_style_returns_no_block`** — `OutputStylePrefix { active:
   None }.splice_into("BASE")` returns `"BASE"` unchanged.
2. **`builtin_styles_load_via_include_str`** — `load_styles(&[], &[])`
   returns exactly the four built-ins (`default`, `proactive`,
   `explanatory`, `learning`).
3. **`builtin_default_has_empty_body`** — `default.md` body must parse
   to empty string (it's a no-op).
4. **`custom_user_style_parses`** — frontmatter with `name`,
   `description` plus body round-trips.
5. **`style_name_must_match_filename`** — `name: foo` in `bar.md`
   errors.
6. **`user_style_shadows_builtin_with_same_name`** — `~/.config/caliban/
   output-styles/learning.md` wins over the embedded `learning`.
7. **`project_style_shadows_user_style`** — project root wins over user.
8. **`plugin_style_namespaced_in_listing`** — plugin-supplied
   `learning.md` from `superpowers` appears as
   `superpowers:learning` in `load_styles` output.
9. **`force_for_plugin_overrides_requested`** — when a plugin style has
   `force_for_plugin: true` and the plugin is enabled, `select_active`
   returns it regardless of `requested`.
10. **`sideload_force_for_plugin_ignored`** — `force_for_plugin: true`
    on a non-plugin (user-level) style is ignored by `select_active`.
11. **`splice_into_emits_xml_tag_with_name`** — output contains
    `<output-style name="learning">…</output-style>`.
12. **`splice_into_composes_with_memory_prefix`** — `MemoryPrefix`
    block, then `OutputStylePrefix` block, then base body, in that
    order.
13. **`keep_coding_instructions_false_suppresses_tools_section`** —
    when style sets `keep_coding_instructions: false`,
    `build_system_prompt` omits the "Tools" / "ask before destructive
    ops" paragraphs.
14. **`learning_post_processor_preserves_todo_human_markers`** — input
    containing `TODO(human): consider X` round-trips unchanged.
15. **`learning_post_processor_wraps_marker_in_span`** — `TODO(human):
    foo` lines get tagged with `<learning-todo>foo</learning-todo>` for
    the TUI renderer (or equivalent; concrete wire format chosen by
    `caliban-agent-core`).
16. **`identity_post_processor_for_non_learning_styles`** —
    `Proactive`'s post-processor is identity.
17. **`select_active_falls_back_to_default_on_unknown_name`** —
    `requested = "does-not-exist"` returns the built-in default with a
    warning logged.

## Risks

- **Prompt-cache invalidation surprises.** Switching styles mutates the
  system prompt, which invalidates the prompt cache on Anthropic /
  OpenAI prefix-cache providers. Mitigation: document explicitly,
  surface a "applies after /clear or restart" hint, default to
  `default` (no block).
- **`TODO(human)` markers in code blocks vs prose.** The post-processor
  must not mangle code blocks where `TODO(human)` is a legitimate
  comment. Mitigation: only wrap markers that are on their own line and
  not inside a fenced code block; document the line-level heuristic.
- **Style sprawl.** Once styles are easy, operators ship dozens with
  overlapping intents. Mitigation: `/config → Output style` shows
  descriptions; surface the source (user / project / plugin) in the
  picker so the operator can spot duplicates.
- **`force_for_plugin: true` UX coercion.** A plugin can silently
  override the operator's selection. Mitigation: lock-badge in the
  picker, plus `caliban plugin disable <name>` releases the lock and
  the picker selection returns.
- **Frontmatter parser duplication with skills.** Both crates parse YAML
  frontmatter from a markdown file. Mitigation: extract a `frontmatter`
  helper into `caliban-core` in a follow-up; copy-paste is acceptable
  in v1 to avoid blocking on a cross-crate refactor.

## Acceptance criteria

- `cargo build --workspace`, `cargo clippy --workspace --all-targets --
  -D warnings`, `cargo fmt --all -- --check` all clean.
- ≥15 tests passing in `caliban-output-styles`, plus an
  end-to-end test in `caliban/tests/` that runs the binary with
  `output_style = "explanatory"` and asserts the system prompt sent to
  the provider contains `<output-style name="explanatory">`.
- caliban binary honors `output_style`; `/config → Output style`
  overlay renders and selects.
- Plugin-supplied styles surface in the picker with namespace prefix.
- Matrix L row 🔴 → ✅ (both "Default/Proactive/Explanatory/Learning"
  and "Custom output-style files").
- ADR 0031 in `accepted` status.
