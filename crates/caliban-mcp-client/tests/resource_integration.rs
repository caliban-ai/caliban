//! Phase C resource integration tests.
//!
//! These exercise the in-memory `McpResource` cache + the
//! `expand_template` / `ResourceMention::parse` pipeline. The full
//! `resources/list` + `resources/read` round-trip against rmcp is
//! covered by the manager integration suite once Phase C lights up the
//! TUI mention completer; here we drive the cache directly so the wire
//! protocol bits are covered by unit tests in `src/resource.rs`.

#![allow(clippy::missing_panics_doc, clippy::pedantic)]

use caliban_mcp_client::{McpResource, ResourceEntry, ResourceMention, expand_template};

/// 1. `@server:resource` autocomplete — typing `@gh:re` returns only the
///    entries beginning with `re`.
#[tokio::test]
async fn autocomplete_filters_entries_by_prefix() {
    let cache = McpResource::new();
    cache
        .set(
            "gh",
            vec![
                ResourceEntry {
                    name: "readme".to_string(),
                    uri: "github://readme".to_string(),
                    uri_template: String::new(),
                    description: None,
                    mime_type: None,
                },
                ResourceEntry {
                    name: "release-notes".to_string(),
                    uri: "github://release-notes".to_string(),
                    uri_template: String::new(),
                    description: None,
                    mime_type: None,
                },
                ResourceEntry {
                    name: "docs".to_string(),
                    uri: "github://docs".to_string(),
                    uri_template: String::new(),
                    description: None,
                    mime_type: None,
                },
            ],
        )
        .await;
    let suggestions = cache.suggest("gh", "re").await;
    assert_eq!(suggestions.len(), 2);
    assert!(suggestions.iter().all(|e| e.name.starts_with("re")));
}

/// 2. `list_changed` invalidates the cache for that server only —
///    other servers are unaffected.
#[tokio::test]
async fn list_changed_only_invalidates_that_server() {
    let cache = McpResource::new();
    cache
        .set(
            "a",
            vec![ResourceEntry {
                name: "a1".to_string(),
                uri: "u:a1".to_string(),
                uri_template: String::new(),
                description: None,
                mime_type: None,
            }],
        )
        .await;
    cache
        .set(
            "b",
            vec![ResourceEntry {
                name: "b1".to_string(),
                uri: "u:b1".to_string(),
                uri_template: String::new(),
                description: None,
                mime_type: None,
            }],
        )
        .await;
    assert!(cache.is_loaded("a").await);
    assert!(cache.is_loaded("b").await);
    cache.invalidate("a").await;
    assert!(!cache.is_loaded("a").await);
    assert!(
        cache.is_loaded("b").await,
        "server B's cache should survive"
    );
}

/// 3. Resource template expansion with positional args from a mention.
#[tokio::test]
async fn mention_drives_template_expansion() {
    let mention = ResourceMention::parse("@github:issue acme widgets 42").expect("parse");
    assert_eq!(mention.server, "github");
    assert_eq!(mention.resource, "issue");
    assert_eq!(mention.args, vec!["acme", "widgets", "42"]);
    let args_refs: Vec<&str> = mention.args.iter().map(String::as_str).collect();
    let uri =
        expand_template("github://repos/{owner}/{repo}/issues/{id}", &args_refs).expect("expand");
    assert_eq!(uri, "github://repos/acme/widgets/issues/42");
}

/// 4. Inlining resource content — once `resources/read` returns text, the
///    caller is responsible for synthesizing a content block. We assert
///    the cache exposes the `mime_type` + URI so callers can format it
///    correctly.
#[tokio::test]
async fn cache_surfaces_mime_type_for_inlining() {
    let cache = McpResource::new();
    cache
        .set(
            "s",
            vec![ResourceEntry {
                name: "doc".to_string(),
                uri: "github://doc".to_string(),
                uri_template: String::new(),
                description: Some("the docs".to_string()),
                mime_type: Some("text/markdown".to_string()),
            }],
        )
        .await;
    let entries = cache.entries("s").await;
    assert_eq!(entries[0].mime_type.as_deref(), Some("text/markdown"));
    assert_eq!(entries[0].uri, "github://doc");
}
