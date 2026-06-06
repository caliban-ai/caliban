<div align="center" style="margin: 0.5rem 0 1.75rem;">
  <svg viewBox="0 0 120 120" width="104" height="104" fill="none" role="img" aria-label="caliban">
    <circle cx="60" cy="60" r="44" stroke="currentColor" stroke-width="3"/>
    <g stroke="currentColor" stroke-width="7" stroke-linecap="round" transform="translate(120,0) scale(-1,1)">
      <path d="M60 56 C80 47 93 60 85 75" transform="rotate(0 60 60)"/>
      <path d="M60 56 C80 47 93 60 85 75" transform="rotate(72 60 60)"/>
      <path d="M60 56 C80 47 93 60 85 75" transform="rotate(144 60 60)"/>
      <path d="M60 56 C80 47 93 60 85 75" transform="rotate(216 60 60)"/>
      <path d="M60 56 C80 47 93 60 85 75" transform="rotate(288 60 60)"/>
    </g>
    <circle cx="60" cy="60" r="6.5" fill="currentColor"/>
  </svg>
</div>

# Caliban User Guide

Caliban is a Rust-native, provider-agnostic AI agent harness — a replacement for Claude Code
that puts you in control of model routing, memory, permissions, and prompt context.
This guide is for **users** who run `caliban` day-to-day and **operators** who deploy
and configure it for a team or homelab; it describes behavior and workflows, not Rust internals.

## How this guide is organized

| Part | What it covers |
|---|---|
| **Introduction** | What Caliban is, why it exists, and current project status |
| **Getting Started** | [Installation & building](./getting-started/installation.md), your [first session](./getting-started/first-session.md), the [interactive TUI](./getting-started/tui.md), and [headless basics](./getting-started/headless.md) |
| **Providers & Models** | [Supported providers](./providers/overview.md), API key setup, model selection, and the [model router](./providers/router.md) |
| **Configuration** | [Settings layering](./configuration/settings-layering.md) across four scopes, file locations, and the full settings reference |
| **Permissions** | [Core concepts](./permissions/concepts.md), the pattern grammar, permission modes, and rule management |
| **Reference** | [CLI flags](./reference/cli.md), settings schema, slash command index, environment variables, and file paths |

```admonish note title="Project status"
Caliban v0.1.0 is a pre-release. The core feature set is daily-usable on `main` under
[AGPL-3.0](./intro/status.md). See [Project Status](./intro/status.md) for what is
shipped versus planned.
```
