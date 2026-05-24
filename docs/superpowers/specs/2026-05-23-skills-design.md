# Skills loader + `SkillTool` — Design

**Date:** 2026-05-23
**Status:** Sketch
**Target branch:** `jf/docs/roadmap-post-webfetch`
**Sub-project of:** caliban Rust agent harness
**Depends on:** `caliban-agent-core` (Tool trait), `caliban-provider` (ContentBlock)
**Related ADR:** [`0019-skills-loading`](../../../adrs/0019-skills-loading.md)

## Goal

Let operators drop YAML-frontmatter-keyed `SKILL.md` files into known
directories and have the model discover and invoke them through a
single built-in `Skill` tool. Format and behavior mirror Claude Code's
superpowers plugin so existing skill libraries port with zero changes.

## Non-goals

- Plugin marketplace or skill discovery service.
- Remote skill fetching (HTTP, git, registry).
- Skill versioning, dependency resolution, or update management.
- Skill execution beyond text-injection into the model's context.
- Bundled default skills shipped in-crate — operator brings their own.
- Per-skill permission scopes (skills run with the agent's current
  tool grants; refinement is hooks-layer concern, not skills-layer).

## Skill format

A skill is a directory containing at minimum a `SKILL.md`:

```
brainstorming/
├── SKILL.md           # required: frontmatter + body
├── scripts/           # optional: helper scripts (NOT auto-executed)
└── references/        # optional: reference docs the body may cite
```

`SKILL.md` is YAML frontmatter followed by markdown:

```yaml
---
name: brainstorming
description: "You MUST use this before any creative work ..."
metadata:
  trigger: pre-implementation
  cost: low
---

# Brainstorming Ideas Into Designs
... markdown body ...
```

Required fields: `name` (string, must match directory name), `description`
(string, surfaced to the model). `metadata` is an optional free-form
`Map<String, serde_yaml::Value>` the loader passes through unchanged
for future use (filtering, tagging, UX). Anything after the closing
`---` is the body — pure markdown, no caliban-specific tags.

## Skill directory layout

Discovery happens at startup in priority order; first hit wins per
`name`:

1. `<workspace_root>/.caliban/skills/<name>/SKILL.md`
2. `~/.config/caliban/skills/<name>/SKILL.md`
3. `~/.local/share/caliban/plugins/*/skills/<name>/SKILL.md`

Paths follow the same XDG / `dirs` conventions as MCP config (ADR 0017).
A skill in an earlier location shadows a later one. Directories that
don't exist are silently skipped.

## Loading logic + caching

`SkillLoader::load_all()` walks each discovery root using `ignore`'s
`WalkBuilder`. For each `<dir>/SKILL.md`:

1. Read (lossy UTF-8). Split on the first `\n---\n` after a leading
   `---\n`; parse the YAML chunk with `serde_yaml`, remainder is body.
2. Validate: `name` matches directory name; `description` non-empty.
3. On parse / validation failure, log `warn!` with path and skip —
   loading is best-effort, one bad skill must not block startup.
4. Insert into `HashMap<String, Skill>`. Duplicate `name` from a
   later root → drop new entry, log `debug!` with both paths.

Bodies are held in memory for the session (typical 2–20 KB; hundreds
of skills fit under 1 MB). No on-disk cache; reload-on-edit deferred.

## `SkillTool` schema and output

```rust
pub struct SkillTool { /* HashMap<String, Skill> + cached description */ }

impl SkillTool {
    pub fn new(skills: Vec<Skill>) -> Self;
    pub fn skills(&self) -> &HashMap<String, Skill>;
}
```

### Input schema

```json
{
  "type": "object",
  "properties": {
    "name": {
      "type": "string",
      "description": "Exact name of the skill to load (case-sensitive)."
    }
  },
  "required": ["name"]
}
```

### Description (model-facing)

The `Tool::description()` returns the static intro plus the available-
skills list:

```
Loads a skill's instruction set. Call with the exact skill name to
receive its body as text, then follow the instructions. Available
skills:

- brainstorming: You MUST use this before any creative work ...
- test-driven-development: Use when implementing any feature or bugfix ...
- writing-plans: Use when you have a spec or requirements ...
- ...
```

The list is rebuilt once at construction and cached in the struct.
If the assembled description exceeds **8 KiB**, the loader truncates
the per-skill description text (not the list) to keep total under
budget and appends `… (description truncated)` to each affected line.
A future iteration may switch to a two-tier surface (see ADR 0019).

### Output

On match: a single `ContentBlock::Text` whose content is the skill's
markdown body, prefixed with one header line `→ Skill <name>` for
parity with other tools.

On miss: `ToolError::InvalidInput("no skill named '<x>' (available: ...)")`
with the first ~10 available names enumerated.

## System-prompt surface

The system-prompt builder in `caliban/src/system_prompt.rs` already
enumerates tool names. The default `Skill` tool description (with
its embedded skill list) becomes part of the registered-tool catalog
the prompt builder reads, so no special-case is needed there —
the existing tool-listing pathway pulls in the skill menu for free.

## `/skills` slash command

The current overlay is a stub. With the loader in place it renders:

```
Skills (3 loaded)

  brainstorming
    ~/.config/caliban/skills/brainstorming/SKILL.md
    You MUST use this before any creative work ...

  test-driven-development
    ~/.config/caliban/skills/test-driven-development/SKILL.md
    Use when implementing any feature or bugfix ...

  writing-plans
    <workspace>/.caliban/skills/writing-plans/SKILL.md
    Use when you have a spec or requirements ...
```

Each entry shows `name`, source path, and the first line of
`description`. Lower-priority polish — searchability, body preview,
scrolling — is v2.

## Crate structure

New workspace member `crates/caliban-skills/`:

```
crates/caliban-skills/
├── Cargo.toml
└── src/
    ├── lib.rs       # re-exports
    ├── skill.rs     # struct Skill { name, description, body, metadata, source_path }
    ├── loader.rs    # SkillLoader::load_all() + discovery roots
    └── tool.rs      # SkillTool impl Tool
```

`Cargo.toml` additions:

```toml
[dependencies]
caliban-agent-core = { workspace = true }
caliban-provider   = { workspace = true }
async-trait        = { workspace = true }
serde              = { workspace = true, features = ["derive"] }
serde_yaml         = "0.9"
serde_json         = { workspace = true }
ignore             = { workspace = true }
thiserror          = { workspace = true }
tracing            = { workspace = true }
dirs               = { workspace = true }

[dev-dependencies]
tempfile           = { workspace = true }
tokio              = { workspace = true, features = ["macros", "rt"] }
```

`serde_yaml` is the only new workspace dep. The `caliban` binary
constructs a `SkillTool` at startup and registers it in `ToolRegistry`.

## Testing

Unit + integration tests in `crates/caliban-skills/`:

1. `loads_well_formed_skill` — frontmatter + body round-trip.
2. `rejects_missing_frontmatter` — file with no `---` block → warning, skipped.
3. `rejects_mismatched_name` — `name: foo` in `bar/SKILL.md` → warning, skipped.
4. `priority_workspace_shadows_user` — same name in both roots, workspace wins.
5. `description_truncation_under_budget` — assembled description ≤ 8 KiB.
6. `skill_tool_returns_body_on_match` — invoke with valid name → body text.
7. `skill_tool_invalid_input_on_miss` — invoke with unknown name → `InvalidInput`.
8. `skill_tool_description_lists_all_skills` — every loaded `name` appears.
9. `metadata_passthrough` — arbitrary metadata round-trips into `Skill`.
10. `missing_discovery_root_is_ok` — non-existent dir → empty load, no error.

Target ~10 new tests.

## Risks

- **Silent parse failures.** Broken skills are logged, not surfaced.
  Mitigation: `/skills` overlay shows a "failed to load: N skills"
  line with paths.
- **`serde_yaml` unmaintained.** Still works for our format;
  `serde_yml` / `serde_yaml_ng` are one-line swaps if needed.
- **Skill content trust.** Skills are arbitrary markdown influencing
  the model — same trust model as `BashTool`. Documented in loader.
- **Description budget.** Dozens of verbose skills hit the 8 KiB cap
  and degrade the menu. Two-tier surface is the planned v2 fix.

## Acceptance criteria

- `cargo build --workspace` clean; clippy + fmt clean.
- `cargo test --workspace` passes — adds ≥ 10 new tests in
  `caliban-skills`.
- `SkillTool` registered in the `caliban` binary; running with skills
  in any discovery root makes them invocable by the model.
- `/skills` overlay shows real loaded skills, not the stub text.
- No new architectural commitments beyond ADR 0019.
