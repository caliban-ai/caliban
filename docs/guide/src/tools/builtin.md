# Built-in Tools

Caliban ships a fixed set of built-in tools that cover the most common agentic tasks: reading and writing files, executing shell commands, searching code, fetching web content, and coordinating work. Every tool is permission-gated (see [Permissions](../permissions/concepts.md)) and subject to the execution policies described in [Tool Execution](./execution.md).

Pass `--no-tools` to disable all tools and run caliban in chat-only mode.

## Tool reference

| Tool | Category | Purpose |
|------|----------|---------|
| `Read` | Filesystem | Read a UTF-8 file, with optional `offset` + `limit` for pagination. Files larger than 5 MB must be read in chunks. |
| `Write` | Filesystem | Write content to a file, creating missing parent directories. Overwrites existing content. |
| `Edit` | Filesystem | Replace occurrences of `old_string` with `new_string` in a file. Expects exactly one match by default; set `replace_all=true` to replace all. |
| `MultiEdit` | Filesystem | Apply a sequence of `{old_string, new_string}` replacements to a single file atomically. If any replacement fails to match, the whole operation is rolled back. |
| `NotebookEdit` | Filesystem | Add, edit, or delete cells in a Jupyter `.ipynb` notebook (nbformat v4). Preserves cell metadata and outputs; writes atomically via tmpfile + rename. |
| `Bash` | Shell | Run a shell command and capture stdout + stderr. Supports `timeout_seconds`, an optional `cwd`, and a `background` flag for long-running processes. |
| `BashBg` | Shell | Companion tools for background Bash jobs: read buffered output (`BashOutput`) or terminate a job (`KillShell`). Background jobs use a 5 GiB ring buffer. |
| `Glob` | Search | Find files by name pattern relative to the workspace root. |
| `Grep` | Search | Search file contents with a regex, powered by the ripgrep library. Returns up to 100 matches by default (max 500). |
| `WebFetch` | Web | GET a URL and return the body as markdown or plain text. HTML is converted via `htmd`. 10 MB body cap, 60 s default timeout (configurable up to 300 s). |
| `WebSearch` | Web | Query a web search API and return ranked results. See backend details below. |
| `TodoWrite` | Agent | Replace the session's shared task list with a new list of `{id, content, status}` items. The list is re-injected into the system prompt each turn. Max 100 items. |
| `AgentTool` | Agent | Spawn an in-process sub-agent with a task prompt and an optional tool allowlist. Output is capped at 5,000 characters. See [Sub-agents](../subagents/overview.md). |
| `EnterPlanMode` | Plan | Switch the session into plan mode. While active, only read-only tools may run; destructive tools are blocked until the operator confirms the plan. |
| `ExitPlanMode` | Plan | Confirm or abandon the current plan and return to normal execution. |
| `ReadMemoryTopic` | Memory | Read one auto-memory topic file by slug. See [Memory Tiers](../memory/tiers.md). |
| `WriteMemoryTopic` | Memory | Write or update an auto-memory topic file and update the `MEMORY.md` index entry atomically. Topic type must be one of `user`, `feedback`, `project`, or `reference`. See [Memory Tiers](../memory/tiers.md). |

## WebSearch backends

`WebSearch` delegates to one of three search APIs, selected by the `CALIBAN_WEBSEARCH_PROVIDER` environment variable:

| Value | API key env var | Default? |
|-------|-----------------|----------|
| `brave` | `BRAVE_API_KEY` | Yes |
| `tavily` | `TAVILY_API_KEY` | No |
| `exa` | `EXA_API_KEY` | No |

If the selected provider's API key is missing, the tool returns a structured error naming the missing variable so the agent can try a different approach rather than failing silently.

```admonish tip title="Chat-only mode"
`--no-tools` disables all built-in tools (and MCP tools) for the session.
This is useful when you want a pure conversation without any side effects —
for example, drafting a message or brainstorming before running anything.
```

## Filesystem tool conflict resolution

`Edit`, `Write`, `MultiEdit`, `NotebookEdit`, and `WriteMemoryTopic` all declare a **conflict key** based on their target path (or memory slug). When the model emits two write operations targeting the same file in a single turn, caliban serializes those calls in submission order rather than letting them interleave. Calls targeting different files still execute in parallel. See [Tool Execution](./execution.md) for details.
