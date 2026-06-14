# Auto-memory (model-written notes) — Design

**Date:** 2026-05-24
**Author:** john.ford2002@gmail.com
**Status:** Implemented
**Sub-project of:** caliban Rust agent harness
**ADR:** `docs/adr/0035-auto-memory.md`

## Goal

Implement Claude Code's "auto memory": the model writes durable
per-project notes about the user, project facts, feedback, and
references; the loader splices a `MEMORY.md` index into the system
prompt and topic files are pulled on demand. The third memory tier
already exists in `caliban-memory` (XML tag `auto-memory-index`); this
spec turns it from "a seed file with conventions" into a fully
operational memory system with model-authored content, type-aware
saving heuristics, and a built-in skill that documents the protocol.

## Non-goals

- **No semantic search / embeddings.** Topic files are pulled by exact
  slug, not by similarity. A `caliban-memory-search` crate may come
  later.
- **No automatic forgetting / pruning.** Memories persist until the
  operator deletes them. Stale memories are a user-facing concern, not
  an automated one.
- **No cross-project memory.** Each project has its own
  `~/.caliban/projects/<sanitized-cwd>/memory/` directory; the only
  cross-project memory is the global `CLAUDE.md` (already in tier 1).
- **No real-time index rewriting.** When a topic file is added or
  edited, the operator (or a hook) re-runs the model to regenerate the
  index summary line — caliban doesn't try to grep + re-summarize on
  every file change.
- **No write to MCP memory backends.** Pluggable storage is out of
  scope. The on-disk format is the only format.

## Architecture

```
caliban-memory
  MemoryConfig (existing)
    auto_memory_dir = ~/.caliban/projects/<sanitized-cwd>/memory/

  loader::load (existing) — now post-processes auto-memory
    ├── reads MEMORY.md (first 200 lines / 25 KB cap)
    ├── strips HTML comments before splice
    └── wraps in <auto-memory>…</auto-memory> block in MemoryPrefix

  auto::TopicLoader (new)
    ├── load_topic(slug) -> Result<TopicFile>
    ├── list_topics()    -> Vec<TopicSummary>
    └── enumerates *.md siblings of MEMORY.md, skipping MEMORY.md itself

caliban-skills
  built-in skill: name = "auto-memory"
    SKILL.md frontmatter: disable_model_invocation: false
    body: when-to-read / when-to-write protocol + format examples
    on invocation: returns the operating manual to the model

caliban-tools-builtin (new tool surface, alongside Read/Write/Edit)
  ReadMemoryTopic(slug)            ← cheap dedicated tool; same as Read but constrained to memory dir
  WriteMemoryTopic(slug, body)     ← writes/updates a topic file + appends index line

agent-core loop
  on SessionStart:
    if MEMORY.md absent → write seed (existing)
  on UserPromptSubmit (heuristic hook surface):
    classify last user message → maybe surface a save prompt
  on conversation end:
    no automatic save; relies on model invoking WriteMemoryTopic
```

## On-disk file layout

```
~/.caliban/
└── projects/
    └── <sanitized-cwd>/         ← e.g. -Users-johnford2002-dev-personal-caliban
        └── memory/
            ├── MEMORY.md        ← index; spliced into prompt (first 200 lines)
            ├── user_role.md     ← topic file (loaded on demand)
            ├── feedback_testing.md
            ├── project_conventions.md
            ├── reference_aws_account.md
            └── …                ← arbitrary slug-named topic files
```

Sanitization follows the existing `caliban-memory::sanitize_workspace`
implementation — backslashes / forward-slashes → `-`, leading dashes
preserved, length-capped at 255.

`auto_memory_directory` setting (and `CALIBAN_MEMORY_DIR` env, already
implemented) overrides the root. The per-project subpath is fixed.

### `MEMORY.md` format

```markdown
# Memory index

- [user role](user_role.md) — user: senior platform engineer at Amplio
- [personal email](personal-email.md) — feedback: use `john.ford2002@gmail.com` for `~/dev/personal/**`
- [sprint mode](sprint-mode.md) — feedback: prefers one-shot spec+plan+impl; skip plan review
- [parity gap matrix](parity-gap-matrix.md) — project: `docs/parity-gap-matrix.md` is the canonical scoreboard
<!-- caliban: auto-memory conventions follow; do not delete -->
Write to this index when you learn something durable about the user, project, or environment. One topic per file, slug in kebab-case. Do not save transient task state, debug traces, or facts already in the repo. Keep this file ≤ 200 lines.
```

The HTML comment block (`<!-- … -->`) is **stripped pre-splice** by the
loader (so model output doesn't bloat the prompt with conventions it
already learned from the auto-memory skill). The existing
`CONVENTIONS_BLOCK` injection in `caliban-memory::loader` is kept (it
ensures conventions are *appended for the model's view* even if the
operator deleted them from disk) but the same HTML-comment regex
strips them on the next read.

### Topic file format

```markdown
---
name: user-role
description: "User's current role + context — informs assumptions about familiarity with topics."
metadata:
  node_type: memory
  type: user                    # one of: user, feedback, project, reference
  originSessionId: 0193f8a2-…
---

# User role

Senior platform engineer at Amplio. Maintains Helm charts and Rust services.
Familiar with: Kubernetes, Cargo workspaces, AGPL licensing, OpenTelemetry,
Helm chart development, Rust async/Tokio.

Less familiar with: GCP IAM specifics, mobile development, frontend frameworks
beyond basic HTML/CSS.

Cross-references: [[parity-gap-matrix]], [[sprint-mode]].
```

Frontmatter fields (validated by loader):

| Field                 | Required | Notes |
| --------------------- | -------- | ----- |
| `name`                | yes      | Slug (kebab-case); must match filename stem. |
| `description`         | yes      | One-line summary; surfaces into `MEMORY.md` index line + `/memory list`. |
| `metadata.type`       | yes      | `user` \| `feedback` \| `project` \| `reference` |
| `metadata.node_type`  | yes      | Always `memory` (sentinel for tooling). |
| `metadata.originSessionId` | no  | UUID of the session that wrote this; informational. |

`[[name]]`-style cross-references are **purely informational** — the
loader does not auto-load them. They serve as breadcrumbs for the
model to know which sibling topics exist; the model decides when to
`ReadMemoryTopic` them.

### HTML-comment stripping

Pre-splice, any `<!-- … -->` (greedy, multi-line) is stripped from
`MEMORY.md`'s body before it goes into the `<auto-memory>` block.
Topic files (loaded via `ReadMemoryTopic`) are passed through
unmodified — the model already knows how to parse frontmatter.

## The four memory types

Saving heuristics — the auto-memory skill documents these, and the
model decides which type each memory falls under at write time.

| Type        | When to write | Examples |
| ----------- | ------------- | -------- |
| `user`      | Durable facts about the user that affect how to address them or what to assume about familiarity. | role, preferred name, communication style, language preference, technical depth. |
| `feedback`  | User-issued correction or preference that should apply to future interactions. | "use personal email here", "skip plan review", "prefer 4-space indentation in YAML". |
| `project`   | Durable facts about the *project* that aren't already documented in the repo. | location of the canonical scoreboard, naming convention for ADRs, deployment pipeline notes. |
| `reference` | Stable external context that's expensive to re-derive. | AWS account IDs, GCP project IDs, internal portal URLs, API quotas. |

The skill's `body` documents these with examples and **anti-examples**
(e.g. "do NOT save: today's debug trace, current task progress, exact
HEAD SHA").

## Public API sketches

```rust
// crates/caliban-memory/src/auto.rs (new)

/// A topic file loaded on demand.
pub struct TopicFile {
    pub slug: String,
    pub description: String,
    pub kind: TopicKind,                  // user | feedback | project | reference
    pub body: String,                     // markdown body after frontmatter
    pub origin_session_id: Option<Uuid>,
    pub path: PathBuf,
}

pub enum TopicKind { User, Feedback, Project, Reference }

/// Enumerator + loader for topic files. Construct once per session.
pub struct TopicLoader {
    dir: PathBuf,
}

impl TopicLoader {
    pub fn new(dir: PathBuf) -> Self;

    /// List every topic file (sibling of MEMORY.md). Lightweight — reads
    /// frontmatter only.
    pub async fn list(&self) -> Result<Vec<TopicSummary>>;

    /// Read one topic file by slug.
    pub async fn read(&self, slug: &str) -> Result<TopicFile>;

    /// Write a topic file and update MEMORY.md's index line atomically.
    pub async fn write(&self, draft: TopicDraft) -> Result<PathBuf>;

    /// Delete a topic file and remove its index line.
    pub async fn delete(&self, slug: &str) -> Result<()>;
}
```

```rust
// crates/caliban-memory/src/lib.rs (additions)

pub use auto::{TopicDraft, TopicFile, TopicKind, TopicLoader, TopicSummary};
```

```rust
// crates/caliban-tools-builtin/src/memory.rs (new)

pub struct ReadMemoryTopic { loader: Arc<TopicLoader> }
pub struct WriteMemoryTopic { loader: Arc<TopicLoader> }

// Both implement caliban_agent_core::Tool with permission category
// "memory" (allowed by default).
```

## Loader integration

Extending `caliban-memory::loader::load`:

```rust
pub async fn load(config: &MemoryConfig) -> Result<MemoryPrefix> {
    let auto_md = ensure_auto_memory(&config.auto_memory_dir).await?;
    let global  = read_optional(config.global_path.as_deref()).await?;
    let project = read_optional(config.project_path.as_deref()).await?;
    let auto_raw = read_optional_with_caps(Some(&auto_md), AUTO_MAX_LINES, AUTO_MAX_BYTES).await?;
    let auto    = auto_raw.map(|t| post_process_auto(t));   // strip HTML comments, inject conventions
    /* … rest as before */
}

const AUTO_MAX_LINES: usize = 200;
const AUTO_MAX_BYTES: usize = 25 * 1024;   // 25 KB
```

`post_process_auto` runs in this order:

1. **HTML comment strip** — regex `<!--[\s\S]*?-->` removed.
2. **Inject conventions** — append `CONVENTIONS_BLOCK` (already
   defined; doesn't double-inject).
3. **Cap** — clamp body to first 200 lines or 25 KB, whichever hits
   first; surplus tagged `truncated_bytes` (existing field).
4. **Re-estimate tokens.**

The `<auto-memory>` XML tag attribute is updated:

```xml
<auto-memory path="…/MEMORY.md" topic_count="14">
…body…
</auto-memory>
```

`topic_count` is the number of `.md` siblings (excluding `MEMORY.md`)
— gives the model a quick "there are 14 topics available; ask
`ReadMemoryTopic` if relevant" signal.

## The memory-management skill

A built-in skill bundled in `caliban-skills` (alongside any future
built-ins). Lives at
`crates/caliban-skills/src/builtin/auto_memory/SKILL.md`:

```markdown
---
name: auto-memory
description: "Read and write durable per-project memory. Use to recall user/project/feedback/reference facts across sessions."
disable_model_invocation: false
metadata:
  builtin: true
  always_available: true
---

# Auto-memory

You have access to per-project memory under
`~/.caliban/projects/<sanitized-cwd>/memory/`. The index, MEMORY.md,
is already in your system prompt (the `<auto-memory>` block above).

## When to READ a topic file

Use `ReadMemoryTopic(slug)` when:
- The user references a topic you don't fully remember from the index
  ("our email convention", "the deploy steps").
- A `[[slug]]` cross-reference appears in another topic you just read.
- The user mentions a person, system, or convention that might be
  documented.

## When to WRITE a topic file

Use `WriteMemoryTopic(slug, body)` when the user provides:

1. **user** — durable facts about themselves (role, preferences, habits).
2. **feedback** — a correction or rule that should apply to future work
   ("use personal email for `~/dev/personal/**`").
3. **project** — durable project facts not in the repo
   ("PR labels live in `.github/labels.yaml`").
4. **reference** — stable external IDs / URLs (AWS account, GCP project).

### DO NOT save

- Transient task state ("currently debugging foo.rs:42").
- Facts already in the repo (don't duplicate CLAUDE.md).
- Single-session debug traces.
- PII the user didn't ask you to remember.

### Format

Topic file frontmatter:
```yaml
---
name: <slug>
description: "<one-line summary, ≤120 chars>"
metadata:
  node_type: memory
  type: user|feedback|project|reference
---
```
Body: markdown. Use `[[other-slug]]` to cross-reference siblings.

After writing, also append a one-line entry to MEMORY.md:
`- [<title>](<slug>.md) — <type>: <one-line summary>`
```

The skill is loaded into the system prompt automatically (since
`always_available: true` and `disable_model_invocation: false`), so
the model has the protocol in context even before the user mentions
memory.

## Settings / env

| Setting / env                          | Default                                       | Effect |
| -------------------------------------- | --------------------------------------------- | ------ |
| `caliban_auto_memory_enabled` (setting)| `true`                                        | Master switch. `false` skips MEMORY.md splice AND hides the auto-memory skill from the model. |
| `CALIBAN_DISABLE_AUTO_MEMORY` (env)    | _unset_                                       | If set to `1` / `true`, force-disable. |
| `auto_memory_directory` (setting)      | `~/.caliban/projects/<sanitized-cwd>/memory/` | Override the root. |
| `CALIBAN_MEMORY_DIR` (env)             | _unset_                                       | Already supported by `MemoryConfig::from_env`. |
| `CALIBAN_AUTO_MEMORY_MAX_LINES` (env)  | `200`                                         | Splice cap. |
| `CALIBAN_AUTO_MEMORY_MAX_BYTES` (env)  | `25600`                                       | Splice cap. |

`/memory` slash command extensions:

```
/memory                  → show all three tiers' summary (existing)
/memory list             → enumerate topic files (slug + type + description)
/memory show <slug>      → render a topic file in the transcript
/memory edit <slug>      → open in $EDITOR / external editor
/memory rm <slug>        → delete topic + remove index line (with confirm)
/memory rebuild-index    → re-write MEMORY.md by enumerating siblings
```

## Hook integration

The auto-memory write path benefits from (but does not require) the
expanded hook surface — `UserPromptSubmit` and `SessionStart` (both
🔴 in the matrix, deferred to a separate ADR). Suggested triggers
when those hooks land:

- **`SessionStart`** — log topic count to debug log; opportunity to
  warn if MEMORY.md is unreadable.
- **`UserPromptSubmit`** — opportunity for a classifier to surface
  "this looks like feedback — save it?" (not implemented in this spec;
  the auto-memory skill body alone is enough for the model to make the
  call inline).

This spec **does not** add automatic write-trigger heuristics that
fire outside the model's control. The model is the sole decider.

## Public surface deltas

### `caliban-memory`

```
+ src/auto.rs                    new
+ MemoryPrefix.topic_count: Option<usize>     new field on Auto tier
~ loader::load:                  reads + strips HTML comments + caps to 200 lines / 25 KB
~ TierKind::Auto::tag:           "auto-memory" (was "auto-memory-index"; we keep the alias)
```

### `caliban-skills`

```
+ src/builtin/                   new module
+ src/builtin/auto_memory/SKILL.md   bundled skill (included_str! at build)
~ Loader registers builtin skills before scanning the user skill dir
```

### `caliban-tools-builtin`

```
+ src/memory.rs                  ReadMemoryTopic + WriteMemoryTopic tools
~ registry:                      register both tools in default registry
```

## Testing strategy

15 enumerated tests:

1. `TopicLoader::list` enumerates `.md` siblings, excludes `MEMORY.md`.
2. `TopicLoader::read("user-role")` parses frontmatter + body.
3. `TopicLoader::read` rejects a file missing required frontmatter fields with a typed error.
4. `TopicLoader::write` creates a new topic + appends an index line, atomically (tempfile + rename).
5. `TopicLoader::write` updating an existing topic replaces the index line in place (one entry, not two).
6. `TopicLoader::delete` removes the file + the index line.
7. HTML-comment stripping: `<!-- conventions -->` removed; `body without comments` preserved.
8. HTML-comment stripping: multi-line `<!-- foo\nbar -->` removed.
9. HTML-comment stripping: comment inside a fenced code block stays in (we strip blindly — document the limitation).
10. `MEMORY.md` cap at 200 lines: line 201 onwards reported as `truncated_bytes > 0`.
11. `MEMORY.md` cap at 25 KB: byte cap wins when lines are very long.
12. `topic_count` populated in `MemoryPrefix.auto` when topic files exist.
13. `CALIBAN_DISABLE_AUTO_MEMORY=1` skips MEMORY.md splice AND drops the auto-memory skill.
14. Built-in `auto-memory` skill loads with `name=auto-memory`, `disable_model_invocation=false`, body containing "When to WRITE".
15. `WriteMemoryTopic` tool rejects slugs containing `/`, `\\`, `..`, or starting with `.` (path-traversal guard).

Integration test (`tests/auto_memory_roundtrip.rs`):

- Build a `MemoryConfig` pointing at a tempdir.
- `load()` seeds `MEMORY.md` (existing seed behavior).
- Call `WriteMemoryTopic` programmatically (`user-role` topic).
- `load()` again → `MemoryPrefix.auto` body now contains the new
  index line; `TopicLoader::read("user-role")` round-trips.

## Risks

- **Stale index.** The model might write a topic but forget the
  MEMORY.md index line. Mitigation: `WriteMemoryTopic` does both in a
  single atomic step server-side — the model can't forget.
- **Slug collisions across types.** Operator manually creates
  `feedback.md` and the model writes `feedback.md` → silent overwrite.
  Mitigation: `WriteMemoryTopic` reads existing frontmatter first; if
  the existing `metadata.type` differs from the new draft's, return a
  typed error and let the model resolve.
- **HTML-comment stripping is greedy.** A code fence containing
  `<!-- -->` will get its comment stripped on splice. Mitigation:
  documented limitation; the strip only affects what's spliced into
  the prompt, not the on-disk file or any `Read`/`ReadMemoryTopic` of
  it.
- **Cap thrash.** A 1000-line MEMORY.md gets capped to 200 — operators
  with rich memory feel like content vanished. Mitigation: `/memory`
  shows the full line count and truncated-bytes warning; `/memory
  rebuild-index` can prune dead links.
- **Cross-project leakage.** A repo moved to a new path under
  `~/dev/` gets a fresh empty memory dir. Mitigation: documented;
  `/memory import <other-cwd>` could be added later if real demand
  emerges.
- **Skill body drift.** The auto-memory protocol embedded in the
  skill body diverges from what this spec says. Mitigation: the spec
  is the source; CI test (#14) asserts the skill body contains the
  required headings and DO-NOT-save list.

## Acceptance criteria

- `cargo build --workspace` clean; `clippy --workspace --all-targets -- -D warnings` clean; `fmt --check` clean.
- All 15 unit tests + the round-trip integration test pass.
- `MemoryPrefix.auto` splice contains the HTML-comment-stripped body
  with `topic_count="N"` attribute on the `<auto-memory>` tag.
- `/memory list`, `/memory show <slug>`, `/memory edit <slug>`,
  `/memory rm <slug>`, `/memory rebuild-index` all functional in TUI.
- Built-in `auto-memory` skill appears in `/skills` with `(builtin)`
  badge and is invocable.
- `ReadMemoryTopic` and `WriteMemoryTopic` registered in the default
  tool registry; both honor the "memory" permission category (allowed
  by default; user can deny `memory.*` globally).
- `CALIBAN_DISABLE_AUTO_MEMORY=1` produces a prefix with no
  `<auto-memory>` block and no auto-memory skill loaded.
- `docs/parity-gap-matrix.md` row under **C. Memory & checkpointing**
  — `Auto-memory (model-written notes per project)` — moves 🔴 → ✅.
- README's Memory section gains a subsection "Auto-memory" with a
  worked example of a topic file + the four types.
- ADR 0035 in `accepted` status (this spec's prerequisite).
