# Output Styles

Output styles nudge the model toward a particular response shape — more explanatory prose, learning-paced prompts with `TODO(human)` markers, or a proactive fill-in approach — by splicing a block into the system prompt. They are orthogonal to tools, hooks, and permissions: switching styles changes only the system prompt.

## Built-in styles

Caliban ships four built-in styles compiled into the binary:

| Name | Description |
|---|---|
| `default` | No-op — identical to having no style configured (zero prompt-cache impact) |
| `proactive` | Encourages the model to fill in gaps and make decisions rather than pausing to ask |
| `explanatory` | Requests detailed commentary explaining each decision and code change |
| `learning` | Instructs the model to emit `TODO(human): <prompt>` markers on non-trivial decisions; the TUI highlights them |

The `default` style emits no block at all, so switching to it produces the exact same system prompt as having no style — prompt-cache hits are preserved.

## Selecting the active style

**Via environment variable** (the only selector that takes effect today):

```bash
CALIBAN_OUTPUT_STYLE=learning caliban
```

The style is resolved once at startup and spliced into the system prompt, so changing it requires a restart.

```admonish warning title="The output_style setting is not yet read"
The `output_style` settings key parses and appears in `/config`, but style selection does
not consult it yet. Only `CALIBAN_OUTPUT_STYLE` picks the style.
```

**In the TUI**: `/output-style` lists the available styles and marks the active one. It does not switch styles.

## How styles splice into the system prompt

`OutputStylePrefix::splice_into` wraps the active style's body in an `<output-style>` XML element and prepends it to the base system prompt. Memory tier content goes first, then the style block, then the base body:

```text
[memory tiers]

<output-style name="explanatory">
... style body ...
</output-style>

[base system prompt]
```

If the active style has an empty body (the `default` style), `splice_into` returns the base prompt unchanged — no extra tokens, no cache miss.

The frontmatter field `keep_coding_instructions: false` (default `true`) lets a style suppress the default coding-assistant guidance block. Use this for documentation-only or writing-only modes where coding instructions are irrelevant.

## Custom styles

Drop a `.md` file with YAML frontmatter into the appropriate directory. The file stem must match the `name:` field.

```text
.caliban/output-styles/
  brief.md
```

Example `brief.md`:

```text
---
name: brief
description: "Terse responses — one sentence per point, no preamble."
keep_coding_instructions: true
---

Keep all responses as brief as possible. One sentence per point.
No greetings, no summaries, no padding. Respond with the minimum necessary.
```

Required fields: `name` and `description`. Both `snake_case` (`keep_coding_instructions`) and `kebab-case` (`keep-coding-instructions`) are accepted.

## Discovery roots

| Priority | Location | Scope |
|---|---|---|
| 1 (highest) | `<workspace>/.caliban/output-styles/<name>.md` | Project |
| 2 | `$XDG_CONFIG_HOME/caliban/output-styles/<name>.md` | User |
| 3 | `$XDG_DATA_HOME/caliban/plugins/<plugin>/output-styles/<name>.md` | Plugin (namespaced `<plugin>:<name>`) |
| 4 (lowest) | Built-ins (compiled in) | Built-in |

A project style with the same `name` shadows user, plugin, and built-in styles.

## Plugin-supplied styles and `force_for_plugin`

The `force_for_plugin: true` frontmatter field (also spelled `force-for-plugin`) is designed to let a plugin-supplied style override the operator's selection while the plugin is enabled. It is parsed but **not yet enforced**: startup passes an empty enabled-plugin list to style selection, so the flag currently has no effect. `/output-style` labels it inert.

## Related pages

- [Plugins](./plugins.md)
- [Settings Reference](../configuration/reference.md) — `output_style` key
- [Slash Command Index](../reference/slash-index.md) — `/output-style` picker
