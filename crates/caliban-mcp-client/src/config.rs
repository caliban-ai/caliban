//! `mcp.toml` config schema + discovery + merge.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::McpError;

/// One MCP server entry as written in `mcp.toml`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ServerConfig {
    /// Executable path or PATH-resolvable name.
    pub command: String,
    /// CLI arguments forwarded verbatim.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables. Values support full-value `${VAR}` expansion
    /// from the caliban process env (no inline interpolation in v1).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Working directory. Relative paths resolve against caliban's cwd. When
    /// `None`, the child inherits caliban's cwd.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// Skip this server entirely (useful for project-level disables).
    #[serde(default)]
    pub disabled: bool,
}

#[derive(Debug, Default, Deserialize)]
struct ServersFile {
    #[serde(default)]
    server: BTreeMap<String, ServerConfig>,
}

/// The merged, parsed MCP config.
#[derive(Debug, Default)]
pub struct McpConfig {
    /// Map of server name → resolved config (with `${VAR}` expanded).
    pub servers: BTreeMap<String, ServerConfig>,
}

/// Validate a server name against `^[a-z0-9_-]{1,32}$`.
#[must_use]
pub fn is_valid_server_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Resolve the standard discovery paths for `mcp.toml`. Returns
/// `(user_path, project_path)`; either may not exist.
#[must_use]
pub fn discovery_paths(workspace_root: &Path) -> (Option<PathBuf>, PathBuf) {
    let user = dirs::config_dir().map(|d| d.join("caliban").join("mcp.toml"));
    let project = workspace_root.join(".caliban").join("mcp.toml");
    (user, project)
}

/// Load and merge MCP config from the user file and the project file.
///
/// Either file may be missing — both missing is a no-op (`Ok(empty config)`).
/// Project entries replace user entries with the same name wholesale.
///
/// # Errors
/// Returns [`McpError::ConfigParse`] if a file exists but is malformed, or
/// [`McpError::InvalidServerName`] if a server key violates the naming rule.
pub fn load_config(workspace_root: &Path) -> Result<McpConfig, McpError> {
    let (user, project) = discovery_paths(workspace_root);
    let mut merged: BTreeMap<String, ServerConfig> = BTreeMap::new();
    if let Some(p) = user.as_deref() {
        merge_from(&mut merged, p)?;
    }
    merge_from(&mut merged, &project)?;
    Ok(McpConfig { servers: merged })
}

fn merge_from(into: &mut BTreeMap<String, ServerConfig>, path: &Path) -> Result<(), McpError> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(McpError::Io {
                path: path.to_path_buf(),
                source: e,
            });
        }
    };
    let parsed: ServersFile = toml::from_str(&raw).map_err(|source| McpError::ConfigParse {
        path: path.to_path_buf(),
        source,
    })?;
    for (name, cfg) in parsed.server {
        if !is_valid_server_name(&name) {
            return Err(McpError::InvalidServerName(name));
        }
        let cfg = expand_env(&name, cfg)?;
        into.insert(name, cfg);
    }
    Ok(())
}

fn expand_env(server: &str, mut cfg: ServerConfig) -> Result<ServerConfig, McpError> {
    let mut expanded: BTreeMap<String, String> = BTreeMap::new();
    for (k, v) in &cfg.env {
        let new_v = if v.starts_with("${") && v.ends_with('}') && v.len() > 3 {
            let var = &v[2..v.len() - 1];
            match std::env::var(var) {
                Ok(val) => val,
                Err(_) => {
                    return Err(McpError::MissingEnv {
                        server: server.to_string(),
                        var: var.to_string(),
                    });
                }
            }
        } else if v.contains("${") {
            // Inline interpolation is not supported in v1.
            return Err(McpError::InlineInterpolation {
                server: server.to_string(),
                key: k.clone(),
            });
        } else {
            v.clone()
        };
        expanded.insert(k.clone(), new_v);
    }
    cfg.env = expanded;
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn write(p: &Path, body: &str) {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, body).unwrap();
    }

    fn parse(body: &str) -> Result<ServersFile, toml::de::Error> {
        toml::from_str(body)
    }

    #[test]
    fn parses_minimal_server() {
        let body = "[server.s1]\ncommand = \"echo\"\n";
        let f = parse(body).unwrap();
        assert_eq!(f.server.len(), 1);
        assert_eq!(f.server["s1"].command, "echo");
        assert!(f.server["s1"].args.is_empty());
        assert!(!f.server["s1"].disabled);
    }

    #[test]
    fn parses_full_server() {
        let body = r#"
[server.linear]
command = "npx"
args = ["-y", "@linear/mcp-server"]
env = { LINEAR_API_KEY = "static-value" }
cwd = "/tmp"
disabled = false
"#;
        let f = parse(body).unwrap();
        let s = &f.server["linear"];
        assert_eq!(s.args, vec!["-y", "@linear/mcp-server"]);
        assert_eq!(s.env["LINEAR_API_KEY"], "static-value");
        assert_eq!(s.cwd.as_deref(), Some(Path::new("/tmp")));
    }

    #[test]
    fn valid_name_rule() {
        assert!(is_valid_server_name("linear"));
        assert!(is_valid_server_name("ls-9_x"));
        assert!(!is_valid_server_name(""));
        assert!(!is_valid_server_name("UPPER"));
        assert!(!is_valid_server_name("with space"));
        assert!(!is_valid_server_name(&"x".repeat(33)));
    }

    #[test]
    fn project_overrides_user_wholesale() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        let user = tmp.path().join("user/caliban/mcp.toml");
        write(
            &user,
            "[server.linear]\ncommand = \"user-cmd\"\nargs = [\"old\"]\n",
        );
        write(
            &workspace.join(".caliban/mcp.toml"),
            "[server.linear]\ncommand = \"project-cmd\"\n",
        );

        // Build a config that merges these. Since `load_config` uses `dirs::config_dir`
        // we can't easily inject the test user file via that path; exercise the merge
        // helper directly.
        let mut merged: BTreeMap<String, ServerConfig> = BTreeMap::new();
        super::merge_from(&mut merged, &user).unwrap();
        super::merge_from(&mut merged, &workspace.join(".caliban/mcp.toml")).unwrap();
        assert_eq!(merged["linear"].command, "project-cmd");
        assert!(
            merged["linear"].args.is_empty(),
            "project entry wholly replaces user entry"
        );
    }

    #[test]
    fn disabled_field_round_trip() {
        let body = "[server.s1]\ncommand = \"x\"\ndisabled = true\n";
        let f = parse(body).unwrap();
        assert!(f.server["s1"].disabled);
    }
}
