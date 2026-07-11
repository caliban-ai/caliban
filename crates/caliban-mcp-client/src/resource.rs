//! `McpResource` — server-advertised data references surfaced via
//! `@<server>:<resource>` in user messages.
//!
//! Resources are not eagerly fetched at startup. The first time the user
//! types `@<server>:` in the TUI, caliban calls `resources/list` for that
//! server and caches the result; the cache is invalidated by
//! `resources/list_changed` notifications. On submit, the resource URI is
//! resolved (templates are expanded positionally from arguments typed
//! after the resource name) and `resources/read` is called; the returned
//! content is inlined into the user message as a content block.
//!
//! See `docs/superpowers/specs/2026-05-24-mcp-v2-design.md` (Resources
//! section) and ADR 0023.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::error::McpError;

/// Resolved view of one resource advertised by a server. Includes both
/// concrete resources (`uri`) and templates (`uri_template`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceEntry {
    /// Identifier used in `@server:name` references.
    pub name: String,
    /// Concrete `uri` when this is not a template; otherwise empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub uri: String,
    /// `uri_template` like `github://repos/{owner}/{repo}/issues/{id}` when
    /// this is a template; otherwise empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub uri_template: String,
    /// Optional description for autocomplete UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional MIME type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

impl ResourceEntry {
    /// `true` if this entry has a `uri_template` and therefore requires
    /// positional arguments to resolve.
    #[must_use]
    pub fn is_template(&self) -> bool {
        !self.uri_template.is_empty()
    }

    /// Extract the placeholder names (`owner`, `repo`, …) from the
    /// `uri_template`. Returns an empty vec for non-templates.
    #[must_use]
    pub fn template_params(&self) -> Vec<String> {
        if !self.is_template() {
            return Vec::new();
        }
        extract_template_params(&self.uri_template)
    }
}

fn extract_template_params(tmpl: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = tmpl.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            let start = i + 1;
            if let Some(off) = bytes[start..].iter().position(|&b| b == b'}') {
                let name = &tmpl[start..start + off];
                if !name.is_empty() {
                    out.push(name.to_string());
                }
                i = start + off + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Expand a `uri_template` by replacing `{name}` placeholders positionally
/// from `args`. The number of args must equal the number of placeholders;
/// extra args or missing args surface as a `ResourceTemplateArity` error.
///
/// Repeated placeholders (`{x}/{x}`) reuse the same positional arg.
///
/// # Errors
/// [`McpError::ResourceTemplateArity`] when `args.len()` doesn't match
/// the unique placeholder count.
pub fn expand_template(tmpl: &str, args: &[&str]) -> Result<String, McpError> {
    let params = extract_template_params(tmpl);
    // Distinct placeholders, preserving first-occurrence order.
    let mut unique: Vec<&str> = Vec::new();
    for p in &params {
        let s = p.as_str();
        if !unique.contains(&s) {
            unique.push(s);
        }
    }
    if unique.len() != args.len() {
        return Err(McpError::ResourceTemplateArity {
            template: tmpl.to_string(),
            expected: unique.len(),
            actual: args.len(),
        });
    }
    let mut map: BTreeMap<&str, &str> = BTreeMap::new();
    for (name, val) in unique.iter().zip(args.iter()) {
        map.insert(*name, *val);
    }
    let mut out = String::with_capacity(tmpl.len());
    let bytes = tmpl.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            let start = i + 1;
            if let Some(off) = bytes[start..].iter().position(|&b| b == b'}') {
                let name = &tmpl[start..start + off];
                if let Some(val) = map.get(name) {
                    out.push_str(val);
                    i = start + off + 1;
                    continue;
                }
            }
        }
        // Copy the char at byte offset `i` intact. `i` is always a char
        // boundary (placeholder jumps land right after an ASCII `}`, and the
        // non-placeholder branch advances by full char widths), so this never
        // splits a UTF-8 scalar. The old `bytes[i] as char` reinterpreted
        // continuation bytes as U+0080–U+00FF, corrupting any non-ASCII
        // template into mojibake (#432).
        // `i` is always on a char boundary here, so `chars().next()` yields the
        // char; the `else` is unreachable but avoids a panic path.
        let Some(ch) = tmpl[i..].chars().next() else {
            break;
        };
        out.push(ch);
        i += ch.len_utf8();
    }
    Ok(out)
}

/// Per-server resource cache. Populated lazily by `ensure_loaded`; the
/// `list_changed` notification invalidates it.
#[derive(Debug, Default, Clone)]
pub struct McpResource {
    inner: Arc<RwLock<ServerResourceCache>>,
}

#[derive(Debug, Default)]
struct ServerResourceCache {
    /// `server -> Vec<ResourceEntry>` cache; `None` means "not loaded yet".
    by_server: BTreeMap<String, Option<Vec<ResourceEntry>>>,
}

impl McpResource {
    /// Fresh empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a server in the cache map without pre-loading. Subsequent
    /// queries return an empty suggestion list until `set` or
    /// `ensure_loaded` populates it.
    pub async fn register_server(&self, server: &str) {
        let mut inner = self.inner.write().await;
        inner.by_server.entry(server.to_string()).or_insert(None);
    }

    /// Replace one server's entries (called after `resources/list`).
    pub async fn set(&self, server: &str, entries: Vec<ResourceEntry>) {
        let mut inner = self.inner.write().await;
        inner.by_server.insert(server.to_string(), Some(entries));
    }

    /// Invalidate one server's cache (e.g. on `resources/list_changed`).
    pub async fn invalidate(&self, server: &str) {
        let mut inner = self.inner.write().await;
        if let Some(slot) = inner.by_server.get_mut(server) {
            *slot = None;
        } else {
            inner.by_server.insert(server.to_string(), None);
        }
    }

    /// `true` if the server's entries are already populated.
    pub async fn is_loaded(&self, server: &str) -> bool {
        let inner = self.inner.read().await;
        matches!(inner.by_server.get(server), Some(Some(_)))
    }

    /// Snapshot of the current entries for a server. Returns an empty
    /// vec if the server isn't tracked or hasn't been loaded yet.
    pub async fn entries(&self, server: &str) -> Vec<ResourceEntry> {
        let inner = self.inner.read().await;
        inner
            .by_server
            .get(server)
            .and_then(|opt| opt.as_ref())
            .cloned()
            .unwrap_or_default()
    }

    /// Servers currently tracked, in key order.
    pub async fn servers(&self) -> Vec<String> {
        let inner = self.inner.read().await;
        inner.by_server.keys().cloned().collect()
    }

    /// Autocomplete suggestions for a typed `@<server>:<prefix>` token.
    /// Returns matching `ResourceEntry`s in the cache's natural order,
    /// filtering by `name.starts_with(prefix)`.
    pub async fn suggest(&self, server: &str, prefix: &str) -> Vec<ResourceEntry> {
        let entries = self.entries(server).await;
        if prefix.is_empty() {
            return entries;
        }
        entries
            .into_iter()
            .filter(|e| e.name.starts_with(prefix))
            .collect()
    }
}

/// Parsed `@<server>:<resource> [arg1 arg2 …]` mention. Used by the TUI
/// completer + by the submit pipeline to resolve to a `resources/read`
/// call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceMention {
    /// Server prefix.
    pub server: String,
    /// Resource name (post-colon).
    pub resource: String,
    /// Positional args typed after the resource name (whitespace-split).
    pub args: Vec<String>,
}

impl ResourceMention {
    /// Parse a single mention token like `@github:issue 1234`. The leading
    /// `@` is optional. Returns `None` if no `:` was found after the
    /// server prefix.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let s = raw.strip_prefix('@').unwrap_or(raw);
        let (server, rest) = s.split_once(':')?;
        if server.is_empty() {
            return None;
        }
        let mut parts = rest.split_whitespace();
        let resource = parts.next()?.to_string();
        let args: Vec<String> = parts.map(str::to_string).collect();
        Some(Self {
            server: server.to_string(),
            resource,
            args,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_params_basic() {
        let p = extract_template_params("github://repos/{owner}/{repo}/issues/{id}");
        assert_eq!(p, vec!["owner", "repo", "id"]);
    }

    #[test]
    fn expand_template_happy() {
        let s = expand_template(
            "github://repos/{owner}/{repo}/issues/{id}",
            &["acme", "widgets", "42"],
        )
        .expect("expand");
        assert_eq!(s, "github://repos/acme/widgets/issues/42");
    }

    #[test]
    fn expand_template_wrong_arity_errors() {
        let err = expand_template("github://{a}/{b}", &["only-one"]).unwrap_err();
        assert!(
            matches!(err, McpError::ResourceTemplateArity { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn expand_template_repeated_placeholder_reuses_arg() {
        let s = expand_template("/{x}/{x}", &["v"]).expect("expand");
        assert_eq!(s, "/v/v");
    }

    #[test]
    fn expand_template_preserves_non_ascii() {
        // #432: non-ASCII literals in the template must survive intact, not be
        // corrupted into mojibake by a byte-wise `as char` copy.
        let s = expand_template("proj/café/{id}/naïve", &["42"]).expect("expand");
        assert_eq!(s, "proj/café/42/naïve");
    }

    #[tokio::test]
    async fn cache_set_and_entries() {
        let cache = McpResource::new();
        cache.register_server("github").await;
        assert!(!cache.is_loaded("github").await);
        cache
            .set(
                "github",
                vec![ResourceEntry {
                    name: "readme".to_string(),
                    uri: "github://readme".to_string(),
                    uri_template: String::new(),
                    description: None,
                    mime_type: None,
                }],
            )
            .await;
        assert!(cache.is_loaded("github").await);
        let entries = cache.entries("github").await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "readme");
    }

    #[tokio::test]
    async fn invalidate_drops_entries() {
        let cache = McpResource::new();
        cache
            .set(
                "s",
                vec![ResourceEntry {
                    name: "x".to_string(),
                    uri: "uri:x".to_string(),
                    uri_template: String::new(),
                    description: None,
                    mime_type: None,
                }],
            )
            .await;
        assert!(cache.is_loaded("s").await);
        cache.invalidate("s").await;
        assert!(!cache.is_loaded("s").await);
    }

    #[tokio::test]
    async fn suggest_filters_by_prefix() {
        let cache = McpResource::new();
        cache
            .set(
                "s",
                vec![
                    ResourceEntry {
                        name: "issue-12".to_string(),
                        uri: "u:1".to_string(),
                        uri_template: String::new(),
                        description: None,
                        mime_type: None,
                    },
                    ResourceEntry {
                        name: "issue-13".to_string(),
                        uri: "u:2".to_string(),
                        uri_template: String::new(),
                        description: None,
                        mime_type: None,
                    },
                    ResourceEntry {
                        name: "doc-x".to_string(),
                        uri: "u:3".to_string(),
                        uri_template: String::new(),
                        description: None,
                        mime_type: None,
                    },
                ],
            )
            .await;
        let suggestions = cache.suggest("s", "issue").await;
        assert_eq!(suggestions.len(), 2);
        assert!(suggestions.iter().all(|e| e.name.starts_with("issue")));
    }

    #[test]
    fn parse_mention_simple() {
        let m = ResourceMention::parse("@github:readme").expect("parse");
        assert_eq!(m.server, "github");
        assert_eq!(m.resource, "readme");
        assert!(m.args.is_empty());
    }

    #[test]
    fn parse_mention_with_args() {
        let m = ResourceMention::parse("@github:issue 1234 v2").expect("parse");
        assert_eq!(m.server, "github");
        assert_eq!(m.resource, "issue");
        assert_eq!(m.args, vec!["1234", "v2"]);
    }

    #[test]
    fn parse_mention_no_at_prefix() {
        let m = ResourceMention::parse("github:doc").expect("parse");
        assert_eq!(m.server, "github");
        assert_eq!(m.resource, "doc");
    }

    #[test]
    fn parse_mention_requires_colon() {
        assert!(ResourceMention::parse("@github").is_none());
        assert!(ResourceMention::parse("@:nope").is_none());
    }

    #[test]
    fn template_params_via_entry() {
        let entry = ResourceEntry {
            name: "issue".to_string(),
            uri: String::new(),
            uri_template: "github://repos/{owner}/issues/{id}".to_string(),
            description: None,
            mime_type: None,
        };
        assert!(entry.is_template());
        assert_eq!(entry.template_params(), vec!["owner", "id"]);
    }
}
