# Design: Caliban User Guide (mdBook on GitHub Pages)

- **Date:** 2026-06-01
- **Status:** Approved
- **Author:** John Ford <john.ford2002@gmail.com>
- **Branch:** `worktree-docs-user-guide`

## Goal

Produce a comprehensive, accurate **user guide** for Caliban, hostable on GitHub
Pages, covering every user- and operator-facing aspect of the tool. Architecture
and rationale stay in the existing 46 ADRs; the guide links to them rather than
duplicating them.

## Audience

**Users + operators.**

- *Users* run `caliban` day-to-day: the TUI, prompts, sessions, slash commands.
- *Operators* configure it: settings layering, permissions, providers, the model
  router, MCP servers, hooks, plugins, telemetry.

Contributors are explicitly **out of scope** — the ADRs and crate-level docs serve
that audience. The guide may reference the crate map in an appendix but will not
document internals or "how to add a provider".

## Toolchain

**mdBook + preprocessors**, chosen for being pure-Rust (one toolchain, no
Python/Node), low-maintenance, and idiomatic for a Rust project's manual.

| Component | Purpose |
|-----------|---------|
| `mdbook` | Core static-site generator; `SUMMARY.md` nav, client-side search, light/dark themes. |
| `mdbook-admonish` | Colored callout boxes (`note`/`warning`/`tip`/`danger`) for visual flair. |
| `mdbook-mermaid` | Text-defined diagrams (architecture layers, permission flow, router fallback). |
| `mdbook-catppuccin` | Modern color palette/theme for additional polish. |

All four are installable via `cargo install`. CI pins versions for reproducibility.

## Layout

- **Book root:** `docs/guide/` (`book.toml` + `src/`), kept apart from the existing
  loose design notes in `docs/`.
- **Build output:** `docs/guide/book/` — gitignored.
- **Nav:** single `src/SUMMARY.md` tree.

## Deployment

A dedicated `.github/workflows/docs.yml`, separate from the Rust CI:

- Triggers on push to `main`, paths-filtered to `docs/guide/**` and the workflow
  file itself; plus `workflow_dispatch` for manual runs.
- Installs `mdbook` + the three preprocessors (cargo-binstall or `cargo install`,
  with a cargo cache).
- Runs `mdbook build docs/guide`.
- Deploys `docs/guide/book/` to GitHub Pages via the official `actions/deploy-pages`
  flow (upload-pages-artifact + deploy-pages), with the `pages`/`id-token`
  permissions and a `github-pages` environment.
- Goes live once Pages is enabled in repo settings (Source = GitHub Actions).

## Content structure (`SUMMARY.md`)

1. **Introduction** — what Caliban is, philosophy / why-vs-Claude-Code, project status & maturity.
2. **Getting Started** — install & build (incl. cloud-transport feature flags), first one-shot (`-p`), the TUI, headless basics.
3. **Interactive Use** — sessions & persistence (`-c`/`-r`/`--session`), the TUI in depth (overlays, keybindings, view/editor modes), prompts, attachments & images.
4. **Slash Commands** — reference for the built-in command set.
5. **Providers & Models** — Anthropic/OpenAI/Google/Ollama, API keys & `api_key_helper`, model selection & defaults, **Model Router v2** (purpose routing, fallback, hedging, circuit breakers, capability filters).
6. **Configuration** — settings layering (managed ▸ user ▸ project ▸ local), per-OS file locations, TOML-primary / JSON-import, live reload, `config`/`settings` commands, settings reference.
7. **Permissions** — concepts, pattern grammar, the 6 modes, the Ask modal, `caliban perms` CLI, headless opt-in, audit log & hardening.
8. **Tools** — built-in tool reference, path matching/restrictions, parallel dispatch, result capping, the OS sandbox.
9. **Extending Caliban** — skills, custom slash commands, hooks (events/handlers), MCP servers (transports/OAuth/resources/elicitation), plugins, output styles.
10. **Sub-agents & Background Work** — `AgentTool`, the background fleet + `caliband` supervisor, git-worktree isolation, `caliban agents` commands.
11. **Memory & Context** — 3-tier memory, `CLAUDE.md` ancestry/imports, auto-memory, checkpoints & `/rewind`, context window & compaction.
12. **Automation & Headless** — print mode / output formats, the stream-json protocol, structured output (`--json-schema`), budgets, `--bare`, CI patterns.
13. **Observability** — telemetry / OpenTelemetry, cost accounting, `caliban doctor`.
14. **Reference** — full CLI reference, settings schema, slash-command index, environment variables, file & directory locations.
15. **Troubleshooting** — `doctor`, provider-specific gotchas (Qwen3/Ollama), known limitations.
16. **Appendix** — glossary, parity-vs-Claude-Code summary, crate map, links into the ADRs for architecture/internals.

## Accuracy principles

- Content is **derived from the live codebase** — `caliban/src/args.rs`, the
  `caliban-settings` schema, the ADRs, and `docs/parity-gap-matrix.md` — not from
  aspiration.
- Anything still marked 🟡/🔴 in the parity matrix is documented as
  **experimental / planned**, clearly flagged, not as a shipped feature.
- Per-OS path tables (config, cache, state, sessions) are spelled out explicitly
  because they differ across macOS/Linux/Windows.

## Non-goals

- No contributor/internals documentation (ADRs own that).
- No doc *versioning* (single `main`-tracked site for now).
- No rewrite of the existing `docs/*.md` design notes; they stay as-is.

## Execution

New git worktree (`docs-user-guide`) → scaffold book + preprocessors + CI → write
all chapters (fanned out across parallel sub-agents, then reviewed/stitched for
consistency) → `mdbook build` to verify compilation and link resolution → commit
(personal email) → PR.
