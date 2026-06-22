//! Cross-plugin aggregation: component discovery roots, merged hooks configs,
//! merged MCP server configs (incl. `${CALIBAN_PLUGIN_ROOT}` expansion and
//! namespacing), and the semver-padding helper they share.

use std::path::{Path, PathBuf};

use crate::expand;
use crate::loaded::LoadedPlugin;

/// Union of skill discovery roots across `plugins`. When a manifest's
/// `components.skills` is set, the explicit subdirectories are returned;
/// otherwise it falls back to `<plugin>/skills/`.
#[must_use]
pub fn skill_roots(plugins: &[LoadedPlugin]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in plugins {
        if p.components.skills.is_empty() {
            out.push(p.root_dir.join("skills"));
        } else {
            out.extend(p.components.skills.iter().cloned());
        }
    }
    out
}

/// Same as [`skill_roots`] for output styles.
#[must_use]
pub fn output_style_roots(plugins: &[LoadedPlugin]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in plugins {
        if p.components.output_styles.is_empty() {
            out.push(p.root_dir.join("output-styles"));
        } else {
            out.extend(p.components.output_styles.iter().cloned());
        }
    }
    out
}

/// Same as [`skill_roots`] for agents.
#[must_use]
pub fn agent_roots(plugins: &[LoadedPlugin]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in plugins {
        if p.components.agents.is_empty() {
            out.push(p.root_dir.join("agents"));
        } else {
            out.extend(p.components.agents.iter().cloned());
        }
    }
    out
}

/// Merged hooks config across all loaded `plugins`. Each plugin's hooks file is
/// read, `${CALIBAN_PLUGIN_ROOT}` expanded, and the resulting
/// `serde_json::Value` returned in load order.
#[must_use]
pub fn hooks_configs(plugins: &[LoadedPlugin]) -> Vec<(String, serde_json::Value)> {
    let mut out = Vec::new();
    for p in plugins {
        let candidates: Vec<PathBuf> = if p.components.hooks.is_empty() {
            vec![p.root_dir.join("hooks").join("hooks.json")]
        } else {
            p.components.hooks.clone()
        };
        for path in candidates {
            if !path.exists() {
                continue;
            }
            match std::fs::read_to_string(&path) {
                Ok(raw) => match serde_json::from_str::<serde_json::Value>(&raw) {
                    Ok(mut v) => {
                        expand::expand_json_in_place(&mut v, &p.root_dir);
                        out.push((p.namespace.clone(), v));
                    }
                    Err(e) => {
                        tracing::warn!(
                            target: caliban_common::tracing_targets::TARGET_PLUGINS,
                            path = %path.display(),
                            error = %e,
                            "skipping malformed plugin hooks.json",
                        );
                    }
                },
                Err(e) => {
                    tracing::warn!(
                        target: caliban_common::tracing_targets::TARGET_PLUGINS,
                        path = %path.display(),
                        error = %e,
                        "could not read plugin hooks.json",
                    );
                }
            }
        }
    }
    out
}

/// Merged MCP server configs across `plugins`. Inline `mcpServers` block wins
/// over `components.mcp_servers` when both are present (with a warning). Each
/// server name is namespaced `<plugin>:<server>`.
#[must_use]
pub fn mcp_servers(plugins: &[LoadedPlugin]) -> Vec<(String, serde_json::Value)> {
    let mut out = Vec::new();
    for p in plugins {
        let has_inline = !p.manifest.mcp_servers_inline.is_empty();
        let has_external = !p.components.mcp_servers.is_empty()
            || p.root_dir.join("mcp").join(".mcp.json").exists();
        if has_inline && has_external {
            tracing::warn!(
                target: caliban_common::tracing_targets::TARGET_PLUGINS,
                plugin = %p.namespace,
                "both inline mcpServers and components.mcp_servers set; inline wins",
            );
        }
        if has_inline {
            for (srv_name, srv) in &p.manifest.mcp_servers_inline {
                let key = format!("{}:{srv_name}", p.namespace);
                let mut v = serde_json::to_value(srv).unwrap_or(serde_json::Value::Null);
                expand::expand_json_in_place(&mut v, &p.root_dir);
                out.push((key, v));
            }
        } else {
            let candidates: Vec<PathBuf> = if p.components.mcp_servers.is_empty() {
                let candidate = p.root_dir.join("mcp").join(".mcp.json");
                if candidate.exists() {
                    vec![candidate]
                } else {
                    Vec::new()
                }
            } else {
                p.components.mcp_servers.clone()
            };
            for path in candidates {
                if !path.exists() {
                    continue;
                }
                match std::fs::read_to_string(&path) {
                    Ok(raw) => match serde_json::from_str::<serde_json::Value>(&raw) {
                        Ok(v) => {
                            flatten_mcp_json(&mut out, &p.namespace, &v, &p.root_dir);
                        }
                        Err(e) => tracing::warn!(
                            target: caliban_common::tracing_targets::TARGET_PLUGINS,
                            path = %path.display(),
                            error = %e,
                            "skipping malformed plugin .mcp.json",
                        ),
                    },
                    Err(e) => tracing::warn!(
                        target: caliban_common::tracing_targets::TARGET_PLUGINS,
                        path = %path.display(),
                        error = %e,
                        "could not read plugin .mcp.json",
                    ),
                }
            }
        }
    }
    out
}

/// Flatten `{"mcpServers": {"a": {...}, "b": {...}}}` (Claude Code shape) or
/// `{"a": {...}}` (bare) into namespaced entries.
fn flatten_mcp_json(
    out: &mut Vec<(String, serde_json::Value)>,
    namespace: &str,
    v: &serde_json::Value,
    root: &Path,
) {
    // Accept either `{"mcpServers": {...}}` or a bare object of servers.
    let map = if let Some(inner) = v.get("mcpServers").and_then(|x| x.as_object()) {
        inner.clone()
    } else if let Some(obj) = v.as_object() {
        obj.clone()
    } else {
        return;
    };
    for (srv_name, mut srv) in map {
        expand::expand_json_in_place(&mut srv, root);
        out.push((format!("{namespace}:{srv_name}"), srv));
    }
}

/// Pad a "0.5" → "0.5.0" so semver parses it.
#[must_use]
pub fn pad_version(v: &str) -> String {
    let parts: Vec<&str> = v.split('.').collect();
    match parts.len() {
        1 => format!("{}.0.0", parts[0]),
        2 => format!("{}.{}.0", parts[0], parts[1]),
        _ => v.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pad_version_widens_partial_versions() {
        assert_eq!(pad_version("1"), "1.0.0");
        assert_eq!(pad_version("1.2"), "1.2.0");
        assert_eq!(pad_version("1.2.3"), "1.2.3");
        assert_eq!(pad_version("1.2.3.4"), "1.2.3.4");
    }
}
