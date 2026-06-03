//! Tool registry — maps tool name → impl.

use std::collections::HashMap;
use std::sync::Arc;

use crate::tool::Tool;

/// Registry of tools by name.
#[derive(Default, Clone)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistry")
            .field("tools", &self.tools.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl ToolRegistry {
    /// Create a new empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tool. If a tool with the same name already exists,
    /// this replaces it and logs a `tracing::warn!`.
    pub fn register(&mut self, tool: Arc<dyn Tool>) -> &mut Self {
        let name = tool.name().to_string();
        if self.tools.contains_key(&name) {
            tracing::warn!("ToolRegistry::register replacing existing tool '{name}'");
        }
        self.tools.insert(name, tool);
        self
    }

    /// Remove a tool by name. Returns the removed tool if it was present.
    pub fn unregister(&mut self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.remove(name)
    }

    /// Look up a tool by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(name)
    }

    /// Iterator over registered names.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.tools.keys().map(String::as_str)
    }

    /// Snapshot the registry as a `Vec` of [`caliban_provider::Tool`] for
    /// inclusion in a [`caliban_provider::CompletionRequest`].
    #[must_use]
    pub fn to_caliban_tools(&self) -> Vec<caliban_provider::Tool> {
        self.tools
            .values()
            .map(|t| caliban_provider::Tool {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.input_schema().clone(),
                cache_control: None,
            })
            .collect()
    }

    /// Variant of [`Self::to_caliban_tools`] that applies a
    /// [`crate::wire_filter::WireFilter`] so MCP tools can be
    /// elided from the wire payload unless activated (ADR-0046).
    /// Returns the filtered set plus the count of MCP tools that
    /// were dropped (used by the stream layer to splice a deferred-
    /// block paragraph into the system prompt).
    #[must_use]
    pub fn to_caliban_tools_filtered(
        &self,
        f: &crate::wire_filter::WireFilter<'_>,
    ) -> crate::wire_filter::WireFilterResult {
        let mut tools = Vec::with_capacity(self.tools.len());
        let mut dropped = 0_usize;

        for t in self.tools.values() {
            let name = t.name();
            if !crate::wire_filter::is_mcp(name) {
                tools.push(caliban_provider::Tool {
                    name: name.to_string(),
                    description: t.description().to_string(),
                    input_schema: t.input_schema().clone(),
                    cache_control: None,
                });
                continue;
            }
            if !f.lazy_mcp {
                tools.push(caliban_provider::Tool {
                    name: name.to_string(),
                    description: t.description().to_string(),
                    input_schema: t.input_schema().clone(),
                    cache_control: None,
                });
                continue;
            }
            let server_match = crate::wire_filter::mcp_server_of(name)
                .is_some_and(|s| f.eager_servers.contains(s));
            if server_match || f.active.is_active(name) {
                tools.push(caliban_provider::Tool {
                    name: name.to_string(),
                    description: t.description().to_string(),
                    input_schema: t.input_schema().clone(),
                    cache_control: None,
                });
                continue;
            }
            dropped += 1;
        }
        crate::wire_filter::WireFilterResult {
            tools,
            dropped_mcp_count: dropped,
        }
    }
}
