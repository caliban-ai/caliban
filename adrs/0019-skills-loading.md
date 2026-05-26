# ADR 0019 · Skills loading

- **Status:** accepted
- **Date:** 2026-05-23

## Context

The `/skills` overlay in the TUI is currently a stub (see ADR 0013).
Skills are priority #4 on the post-WebFetch roadmap: they let the
operator drop in reusable instruction-and-procedure packages — Claude
Code's "superpowers" model — without recompiling caliban or shipping
prompts in-crate. The full implementation spec lives at
`docs/superpowers/specs/2026-05-23-skills-design.md`; this ADR records
the architectural commitments only.

## Decision

### Skills are file-based, frontmatter-keyed

A skill is a directory `<skill-name>/SKILL.md`. The file is YAML
frontmatter followed by a markdown body:

```
---
name: brainstorming
description: "You MUST use this before any creative work ..."
metadata:
  trigger: pre-implementation
---

# Brainstorming Ideas Into Designs
...
```

`name` and `description` are required; `metadata.*` is a free-form map
the loader passes through unchanged. The body is the model-facing
instruction set — no execution, no scripts auto-run, no sandbox. This
format matches the superpowers plugin so existing skills can be
copied in unchanged.

### Skills surface as a single built-in `Skill` tool

A built-in `Skill` tool with `invoke({"name": "<x>"})` loads
`<x>/SKILL.md` and returns its body as a `ContentBlock::Text`. Loaded
skills are NOT registered individually — that would explode the
tool-use schema. The `Skill` tool's description carries a bulleted
`<name>: <description>` list of every loaded skill, so the model
knows the menu and can call `Skill` with the right name.

### Skills are NOT auto-loaded into the system prompt

Loading every body upfront burns thousands of tokens per turn at any
nontrivial skill count. Only `description` lines hit the prompt
(via the tool description above); bodies load on-demand.

### Discovery locations (priority order)

1. `<workspace_root>/.caliban/skills/` — project-pinned skills
2. `~/.config/caliban/skills/` — per-user skills
3. `~/.local/share/caliban/plugins/*/skills/` — global plugin dir,
   mirrors how Claude Code resolves plugin skills

A skill in an earlier location shadows a later one with the same
`name`. Paths are XDG-aware on Linux and use `cache_dir`/`data_dir`
analogues on macOS, matching the MCP config conventions in ADR 0017.

### No skill execution sandbox

Skills are text injected into the model's context. They are not
executable code. The `scripts/` and `references/` subdirectories that
appear in some Claude Code skills are loadable only by the model
through existing `Read` / `Bash` tools — caliban itself does not
execute anything skill-side. This keeps the trust model identical to
"the operator wrote this file."

### New crate: `caliban-skills`

Skills logic lives in a new workspace crate `crates/caliban-skills/`
exporting `SkillLoader`, `Skill`, and `SkillTool`. It depends on
`caliban-agent-core` (for the `Tool` trait), `serde` + `serde_yaml`
for the frontmatter, `ignore` (already in the workspace) for
directory walking, and `thiserror`. The `caliban` binary constructs
one `SkillTool` at startup, registers it with `ToolRegistry`, and
wires the loaded skills into the `/skills` overlay.

## Consequences

- **Positive:** Existing superpowers-format skills port with zero
  changes. Token cost stays bounded — only descriptions hit the
  prompt; bodies are pay-per-use. Skills are uniform with every
  other tool (same registry, same hooks, same audit log).
- **Negative:** The `Skill` tool's description grows with skill
  count; ~50 skills crowds the schema budget (truncation policy
  is spec-level concern). Frontmatter parse failures are per-file
  warnings — silent skill loss if the operator doesn't watch logs.
  No versioning: an update is a directory-replace.
- **Revisit if:** Description-list growth crowds the schema —
  consider a two-tier surface (frequent inline, rare via a
  `ListSkills` tool). If operators want bundled defaults, add an
  opt-in `--with-default-skills` flag (currently a non-goal).
