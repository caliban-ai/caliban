# Skills

Skills are reusable instruction packages that the model loads on demand. Each skill is a markdown file with YAML frontmatter — the same format as the Anthropic "superpowers" plugin ecosystem, so existing skills port without changes.

Skills are not executed; they inject text into the model's context. A skill can describe a workflow, a style guide, a debugging procedure, or any other multi-step process. Only the `description` line is always visible to the model; the full body is fetched lazily when the model calls the `Skill` tool.

## How the Skill tool works

Caliban registers a single built-in tool named `Skill`. Its description lists every loaded skill by name and one-line description. When the model wants to follow a skill's instructions, it calls `Skill` with the skill's exact name; the harness returns the body as text and the model proceeds accordingly.

This design keeps the token cost bounded: descriptions are always present, bodies are pay-per-use.

## Discovery roots

Caliban scans three roots in priority order. The first match for a given name wins; later roots are shadowed.

| Priority | Location | Scope |
|---|---|---|
| 1 (highest) | `<workspace>/.caliban/skills/` | Project |
| 2 | `~/.config/caliban/skills/` (XDG-aware) | User |
| 3 | `~/.local/share/caliban/plugins/*/skills/` | Plugin-managed |

A project-level skill with the same `name` as a user-level skill silently replaces it. Malformed `SKILL.md` files are logged at `warn` and skipped — loading is best-effort.

## Skill file format

Each skill lives in its own subdirectory. The directory name must match the `name:` frontmatter field exactly.

```text
.caliban/skills/
  my-workflow/
    SKILL.md
```

`SKILL.md` structure:

```text
---
name: my-workflow
description: "One-line summary shown to the model in the Skill tool description."
metadata:
  trigger: pre-implementation   # free-form; passed through unchanged
---

# My Workflow

Full markdown instruction set. Only loaded when the model calls Skill({"name": "my-workflow"}).
```

Required frontmatter fields: `name` and `description`. The `metadata` map is optional.

## Built-in skills

Caliban ships one built-in skill compiled into the binary:

| Name | Purpose |
|---|---|
| `auto-memory` | Protocol for reading and writing the auto-memory tiers |

Built-ins register before the directory scan, so a user or project skill with the same name will shadow them.

## Disabling skills

| Method | Effect |
|---|---|
| `--no-skills` flag | Disables the `Skill` tool entirely; no skills are loaded |
| `CALIBAN_NO_SKILLS=1` | Same, via environment variable |

```admonish tip title="Shadowing a built-in"
To override the built-in `auto-memory` skill, place your own `auto-memory/SKILL.md` in `.caliban/skills/`. It will take priority over the embedded version without any additional configuration.
```

## Related pages

- [Plugins](./plugins.md) — bundle skills alongside hooks, MCP servers, and output styles
- [Slash Command Index](../reference/slash-index.md) — `/skills` overlay shows loaded skills
