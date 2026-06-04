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
