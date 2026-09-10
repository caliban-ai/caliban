//! Declarative assembly of the built-in tool set (#541).
//!
//! Replaces the hand-wired branch-per-tool block in the binary's composition
//! root with a descriptor table: each built-in declares its own availability
//! predicate and factory over a [`ToolBuildCtx`] of already-resolved inputs.
//! Adding a built-in that lives in this crate means adding one
//! [`BuiltinToolDescriptor`] to [`builtin_tool_descriptors`] — no edit to the
//! composition root.
//!
//! Deliberately **not** covered here, and why:
//! - `SkillTool` lives in `caliban-skills` behind a side-effecting discovery
//!   loader (frontmatter validation, stderr warnings, built-in registration);
//!   that is composition-root logic, so the caller registers it separately.
//! - MCP tools register dynamically through their activation set, after the
//!   built-in set is assembled.
//!
//! Both still land in the same [`ToolRegistry`]; this module owns only the
//! static built-in table so the crate layering stays pointed downward — the
//! descriptors depend on no bin-crate types (`Args`) and no `caliban-settings`
//! shapes, only on already-resolved values the caller places in the ctx.

use std::sync::Arc;

use caliban_agent_core::{SharedPlanMode, SharedTodos, Tool, ToolRegistry};
use caliban_memory::{TopicBackend, TopicLoader};
use caliban_sandbox::SandboxedShim;

use crate::{
    BashOutputTool, BashTool, EditTool, EnterPlanModeTool, ExitPlanModeTool, GlobTool, GrepTool,
    KillShellTool, MultiEditTool, NotebookEditTool, ReadMemoryTopicTool, ReadTool, TodoWriteTool,
    WebFetchTool, WebSearchTool, WorkspaceRoot, WriteMemoryTopicTool, WriteTool,
};

/// Already-resolved inputs a built-in tool factory may need.
///
/// The composition root resolves all policy (path fence, sandbox backend, the
/// gate flags) into plain data here, so the descriptors depend on nothing
/// upward: no `Args`, no `caliban-settings` types.
pub struct ToolBuildCtx {
    /// Workspace root the file tools operate against — already resolved to the
    /// restricted or unrestricted view by the caller.
    pub root: WorkspaceRoot,
    /// Shared TODO list backing `TodoWrite`.
    pub todos: SharedTodos,
    /// Shared plan-mode flag backing the plan-mode tools.
    pub plan_mode: SharedPlanMode,
    /// HTTP client for the web tools (built once by the caller).
    pub web_client: reqwest::Client,
    /// Resolved Bash sandbox: `Some` wraps each command in the OS write-fence;
    /// `None` runs Bash unfenced (equivalent to `BashTool::new`).
    pub bash_sandbox: Option<Arc<SandboxedShim>>,
    /// Backend for the auto-memory topic tools.
    pub topic_backend: Arc<dyn TopicBackend>,
    /// Whether the auto-memory tools are enabled (kill switch off, ADR 0035).
    pub auto_memory_enabled: bool,
    /// `--bare`: suppress the auto-memory tools (and, in the caller, skills).
    pub bare: bool,
}

/// One built-in tool — or a tightly-coupled group — declared for assembly.
pub struct BuiltinToolDescriptor {
    /// Whether this descriptor contributes to a given run.
    pub available: fn(&ToolBuildCtx) -> bool,
    /// Factory producing the tool(s) to register. A descriptor may yield more
    /// than one tool (the auto-memory pair), so this returns a `Vec`.
    pub build: fn(&ToolBuildCtx) -> Vec<Arc<dyn Tool>>,
}

/// Predicate: available whenever tools are on at all.
fn always(_: &ToolBuildCtx) -> bool {
    true
}

/// Predicate: the auto-memory tools — enabled and not in `--bare` mode.
fn auto_memory(ctx: &ToolBuildCtx) -> bool {
    ctx.auto_memory_enabled && !ctx.bare
}

/// The built-in tool table. Iteration order is registration order, matching
/// the historical hand-wired order so behavior is unchanged.
pub fn builtin_tool_descriptors() -> Vec<BuiltinToolDescriptor> {
    vec![
        BuiltinToolDescriptor {
            available: always,
            build: |c| vec![Arc::new(ReadTool::new(c.root.clone()))],
        },
        BuiltinToolDescriptor {
            available: always,
            build: |c| vec![Arc::new(WriteTool::new(c.root.clone()))],
        },
        BuiltinToolDescriptor {
            available: always,
            build: |c| vec![Arc::new(EditTool::new(c.root.clone()))],
        },
        BuiltinToolDescriptor {
            available: always,
            build: |c| vec![Arc::new(MultiEditTool::new(c.root.clone()))],
        },
        BuiltinToolDescriptor {
            available: always,
            build: |c| vec![Arc::new(NotebookEditTool::new(c.root.clone()))],
        },
        BuiltinToolDescriptor {
            available: always,
            // Bash needs the OS sandbox to be fenced — a path-prefix check
            // can't contain an arbitrary shell command (ADR 0032, #328). The
            // caller resolves the fence into `bash_sandbox`; `None` == unfenced.
            build: |c| {
                vec![Arc::new(BashTool::with_sandbox(
                    c.root.clone(),
                    c.bash_sandbox.clone(),
                ))]
            },
        },
        BuiltinToolDescriptor {
            available: always,
            build: |c| vec![Arc::new(GlobTool::new(c.root.clone()))],
        },
        BuiltinToolDescriptor {
            available: always,
            build: |c| vec![Arc::new(GrepTool::new(c.root.clone()))],
        },
        BuiltinToolDescriptor {
            available: always,
            build: |c| vec![Arc::new(WebFetchTool::new(c.web_client.clone()))],
        },
        BuiltinToolDescriptor {
            available: always,
            build: |c| vec![Arc::new(WebSearchTool::new(c.web_client.clone()))],
        },
        BuiltinToolDescriptor {
            available: always,
            build: |_| vec![Arc::new(BashOutputTool::with_global_registry())],
        },
        BuiltinToolDescriptor {
            available: always,
            build: |_| vec![Arc::new(KillShellTool::with_global_registry())],
        },
        BuiltinToolDescriptor {
            available: always,
            build: |c| vec![Arc::new(TodoWriteTool::new(c.todos.clone()))],
        },
        BuiltinToolDescriptor {
            available: always,
            build: |c| vec![Arc::new(EnterPlanModeTool::new(Arc::clone(&c.plan_mode)))],
        },
        BuiltinToolDescriptor {
            available: always,
            build: |c| vec![Arc::new(ExitPlanModeTool::new(Arc::clone(&c.plan_mode)))],
        },
        // Auto-memory tools — kill switch via env per ADR 0035; also skipped in
        // `--bare`. The skill body documents the protocol, so the two gate
        // together (see the caller's skills step). Registered as a pair.
        BuiltinToolDescriptor {
            available: auto_memory,
            build: |c| {
                let loader = Arc::new(TopicLoader::with_backend_arc(Arc::clone(&c.topic_backend)));
                vec![
                    Arc::new(ReadMemoryTopicTool::new(Arc::clone(&loader))),
                    Arc::new(WriteMemoryTopicTool::new(loader)),
                ]
            },
        },
    ]
}

/// Assemble a [`ToolRegistry`] from the built-in descriptor table: iterate the
/// descriptors, register each whose availability predicate holds. This is the
/// whole of built-in assembly — no per-tool branching.
#[must_use]
pub fn build_builtin_registry(ctx: &ToolBuildCtx) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    for desc in builtin_tool_descriptors() {
        if (desc.available)(ctx) {
            for tool in (desc.build)(ctx) {
                registry.register(tool);
            }
        }
    }
    registry
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    fn ctx(auto_memory_enabled: bool, bare: bool) -> ToolBuildCtx {
        ToolBuildCtx {
            root: WorkspaceRoot::new(PathBuf::from(".")),
            todos: caliban_agent_core::new_shared_todos(),
            plan_mode: caliban_agent_core::new_shared_plan_mode(),
            web_client: reqwest::Client::new(),
            bash_sandbox: None,
            topic_backend: Arc::new(caliban_memory::FsTopicBackend::new(PathBuf::from("."))),
            auto_memory_enabled,
            bare,
        }
    }

    fn names(ctx: &ToolBuildCtx) -> BTreeSet<String> {
        build_builtin_registry(ctx)
            .names()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn memory_pair_present_when_enabled_and_not_bare() {
        let n = names(&ctx(true, false));
        assert!(n.contains("ReadMemoryTopic"));
        assert!(n.contains("WriteMemoryTopic"));
        // A core tool is always present as a sanity anchor.
        assert!(n.contains("Bash"));
    }

    #[test]
    fn memory_pair_absent_when_kill_switch_set() {
        // The auto_memory gate is resolved data here, so this covers the
        // CALIBAN_CHECKPOINT_DISABLED-equivalent kill switch without mutating
        // process-global env (#541).
        let n = names(&ctx(false, false));
        assert!(!n.contains("ReadMemoryTopic"));
        assert!(!n.contains("WriteMemoryTopic"));
        assert!(
            n.contains("Bash"),
            "core tools stay when only memory is off"
        );
    }

    #[test]
    fn memory_pair_absent_in_bare_mode() {
        let n = names(&ctx(true, true));
        assert!(!n.contains("ReadMemoryTopic"));
        assert!(!n.contains("WriteMemoryTopic"));
    }

    #[test]
    fn bash_is_unfenced_by_default_and_present() {
        // `bash_sandbox: None` == unfenced Bash (equivalent to BashTool::new);
        // the tool is still registered.
        assert!(names(&ctx(true, false)).contains("Bash"));
    }
}
