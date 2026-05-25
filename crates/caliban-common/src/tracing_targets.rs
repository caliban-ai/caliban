//! Canonical `tracing` target strings.
//!
//! Replaces the bare string literals like `target: "caliban::mcp"` that had
//! drifted across the workspace. The TUI's env-filter UI and the telemetry
//! pipeline rely on a stable set of target prefixes — this module is the
//! single source of truth.
//!
//! ```ignore
//! use caliban_common::tracing_targets::TARGET_MCP;
//! tracing::debug!(target: TARGET_MCP, "spawning server");
//! ```

// Layer 1: tools / agent loop / providers
/// Tool dispatch (`tools::*`) — fires per tool call.
pub const TARGET_TOOLS: &str = "caliban::tools";
/// Turn / cache / token timing.
pub const TARGET_TIMING: &str = "caliban::timing";
/// Cache-aware metadata (`cache_creation` / `cache_read` tokens).
pub const TARGET_CACHE: &str = "caliban::cache";

// Permissions / hooks
/// Permission engine + ask handler.
pub const TARGET_PERMISSIONS: &str = "caliban::permissions";
/// Custom hook execution (`hook::*`).
pub const TARGET_HOOKS: &str = "caliban::hooks";

// MCP family
/// Top-level MCP client + manager.
pub const TARGET_MCP: &str = "caliban::mcp";
/// stderr from spawned stdio servers.
pub const TARGET_MCP_STDERR: &str = "caliban::mcp::stderr";
/// OAuth flow (token store, refresh, discovery).
pub const TARGET_MCP_OAUTH: &str = "caliban::mcp::oauth";
/// Elicitation (interactive prompts triggered by the server).
pub const TARGET_MCP_ELICITATION: &str = "caliban::mcp::elicitation";

// Memory family
/// Memory top-level (loader, rules summary).
pub const TARGET_MEMORY: &str = "caliban::memory";
/// Auto-memory writer.
pub const TARGET_MEMORY_AUTO: &str = "caliban::memory::auto";
/// `/init` import flow.
pub const TARGET_MEMORY_INIT: &str = "caliban::memory::init";
/// Memory rules engine.
pub const TARGET_MEMORY_RULES: &str = "caliban::memory::rules";

// Other subsystems
/// Sessions store.
pub const TARGET_SESSIONS: &str = "caliban::sessions";
/// Settings hierarchy (layered loader + watcher).
pub const TARGET_SETTINGS: &str = "caliban::settings";
/// Skills loader.
pub const TARGET_SKILLS: &str = "caliban::skills";
/// Plugins (install, uninstall, expand).
pub const TARGET_PLUGINS: &str = "caliban::plugins";
/// Output styles loader.
pub const TARGET_OUTPUT_STYLES: &str = "caliban::output_styles";
/// Images pipeline.
pub const TARGET_IMAGES: &str = "caliban::images";
/// Model router + circuit breaker + hedge dispatch.
pub const TARGET_ROUTER: &str = "caliban::router";
/// Cost meter / telemetry.
pub const TARGET_COST: &str = "caliban::cost";
/// Telemetry init + transport.
pub const TARGET_TELEMETRY: &str = "caliban::telemetry";

// Provider-specific
/// Vertex AI provider.
pub const TARGET_PROVIDER_VERTEX: &str = "caliban::provider::vertex";
/// AWS Bedrock provider.
pub const TARGET_PROVIDER_BEDROCK: &str = "caliban::provider::bedrock";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_are_caliban_prefixed() {
        for t in [
            TARGET_TOOLS,
            TARGET_TIMING,
            TARGET_CACHE,
            TARGET_PERMISSIONS,
            TARGET_HOOKS,
            TARGET_MCP,
            TARGET_MCP_STDERR,
            TARGET_MCP_OAUTH,
            TARGET_MCP_ELICITATION,
            TARGET_MEMORY,
            TARGET_MEMORY_AUTO,
            TARGET_MEMORY_INIT,
            TARGET_MEMORY_RULES,
            TARGET_SESSIONS,
            TARGET_SETTINGS,
            TARGET_SKILLS,
            TARGET_PLUGINS,
            TARGET_OUTPUT_STYLES,
            TARGET_IMAGES,
            TARGET_ROUTER,
            TARGET_COST,
            TARGET_TELEMETRY,
            TARGET_PROVIDER_VERTEX,
            TARGET_PROVIDER_BEDROCK,
        ] {
            assert!(t.starts_with("caliban"), "{t}");
        }
    }
}
