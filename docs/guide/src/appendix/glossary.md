# Glossary

Concise definitions for terms used throughout this guide. Each links to the chapter where the concept is covered in depth.

---

**agent harness**
The runtime that drives the model → tool → model loop: reads user input, calls the provider, dispatches tool calls, feeds results back, and repeats until a terminal condition. Caliban is an agent harness. See [What Is Caliban?](../intro/what-is-caliban.md).

**auto-memory**
Per-project notes written by the model itself into a designated memory file. Injected into the system prompt on subsequent sessions. See [Auto-Memory](../memory/auto-memory.md).

**checkpoint**
A snapshot of the conversation state (messages + file-tool pre-images) taken before each prompt. Used by `/rewind` to restore a prior state. See [Checkpoints & Rewind](../memory/checkpoints.md).

**compaction**
The process of summarising or truncating conversation history when the context window approaches its limit, allowing the session to continue. See [Context & Compaction](../memory/context-compaction.md).

**headless / print mode**
Non-interactive operation via `-p` / `--print`. Caliban drives the agent without a TUI and emits text or structured JSON output to stdout. See [Print Mode](../automation/print-mode.md) and [The stream-json Protocol](../automation/stream-json.md).

**hook**
An event-driven callback executed by an external command, HTTP endpoint, MCP tool, or in-process handler at defined points in the agent lifecycle (e.g. `before_tool`, `SessionStart`). See [Hooks](../extending/hooks.md).

**MCP server**
A Model Context Protocol server that exposes additional tools to caliban over stdio, HTTP/SSE, or streamable-HTTP transports. Caliban discovers and manages MCP servers via its settings. See [MCP Servers](../extending/mcp.md).

**memory tier**
One of the three layers of context prepended to the system prompt: global (`~/.claude/CLAUDE.md`), project (`<workspace>/CLAUDE.md`), and auto-memory (model-written notes). See [Memory Tiers](../memory/tiers.md).

**message IR**
The provider-neutral internal representation of conversation messages used by `caliban-common`. All providers translate to and from this IR so the agent core stays provider-agnostic. See [Architecture & ADRs](./adrs.md) (ADR 0006).

**output style**
A named instruction set (Default, Proactive, Explanatory, Learning, or custom) that shapes how the model formats and explains its responses. See [Output Styles](../extending/output-styles.md).

**permission mode**
A named preset that sets the default disposition for tool-call permission checks. Modes include `default`, `acceptEdits`, `plan`, `auto`, `dontAsk`, and `bypassPermissions`. See [Permission Modes](../permissions/modes.md).

**plugin**
A self-contained bundle of skills, hooks, agents, MCP server configs, and output styles distributed as a directory with a `plugin.json` manifest. See [Plugins](../extending/plugins.md).

**provider**
An adapter that translates caliban's message IR to and from a specific model API (Anthropic, OpenAI, Ollama, Google, Bedrock, Vertex). See [Supported Providers](../providers/overview.md).

**router**
The `caliban-model-router` layer that selects a provider+model for each request based on configured rules, purpose keys, fallback chains, circuit breakers, and capability requirements. See [The Model Router](../providers/router.md).

**sandbox**
An OS-level confinement layer (macOS Seatbelt or Linux bubblewrap) applied to shell and file tools to restrict what they can access on the host. See [The OS Sandbox](../tools/sandbox.md).

**session**
A persisted conversation: a named JSON file on disk containing the full message history for a continuous exchange. See [Sessions & Persistence](../interactive/sessions.md).

**skill**
A markdown file with YAML frontmatter that the model can invoke as a tool. Skills encapsulate reusable workflows without requiring code. See [Skills](../extending/skills.md).

**sub-agent**
A nested caliban instance spawned by the parent agent to execute a delegated task, optionally in an isolated git worktree. See [Sub-agents](../subagents/overview.md).

**tool**
A capability the model can invoke during a turn — built-in tools include Read, Write, Bash, Glob, Grep, Edit, WebSearch, and AgentTool. See [Built-in Tools](../tools/builtin.md) and [Tool Execution](../tools/execution.md).
