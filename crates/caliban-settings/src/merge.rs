//! Deep-merge two `serde_json::Value` trees with documented per-key
//! rules.
//!
//! The high-level loader walks the scope chain *from lowest-priority to
//! highest*, calling [`merge_values`] to overlay each scope's contents
//! on top of the accumulator. After the chain is consumed, the result
//! is `serde_json::from_value::<Settings>(…)`'d.
//!
//! ## Rules
//!
//! For two objects `lower` and `higher`, the merge produces a new
//! object where each key follows this matrix:
//!
//! | Key                                           | Rule                                |
//! |-----------------------------------------------|-------------------------------------|
//! | `permissions.allow|ask|deny`                  | concatenate (lower first, higher appended) |
//! | `allowed_http_hook_urls`                      | concatenate + dedupe                |
//! | `http_hook_allowed_env_vars`                  | concatenate + dedupe                |
//! | `additional_directories` / `claude_md_excludes` | concatenate + dedupe              |
//! | `mcp_servers.<name>` / `env` / `model_overrides` | deep-merge key by key             |
//! | `hooks.<Event>` (array)                       | concatenate                         |
//! | any other scalar                              | higher wins                         |
//! | nested object not listed above                | recursive deep-merge                |

use serde_json::{Map, Value};

use crate::Settings;

/// Set of permission array keys that concatenate.
const PERMISSION_LIST_KEYS: &[&str] = &["allow", "ask", "deny"];

/// Top-level array keys that concatenate + dedupe.
const DEDUPE_ARRAY_KEYS: &[&str] = &[
    "allowed_http_hook_urls",
    "http_hook_allowed_env_vars",
    "additional_directories",
    "claude_md_excludes",
];

/// Top-level object keys whose `<name>` children deep-merge per key.
const NAMED_OBJECT_KEYS: &[&str] = &["mcp_servers", "env", "model_overrides"];

/// Merge `higher` on top of `lower`, in-place.
///
/// `higher` represents the higher-priority scope; its values win for
/// scalars while arrays and objects follow the per-key rules.
pub fn merge_values(lower: &mut Value, higher: Value) {
    merge_inner(lower, higher, /* top */ true);
}

fn merge_inner(lower: &mut Value, higher: Value, top: bool) {
    match (lower, higher) {
        (Value::Object(l), Value::Object(h)) => {
            for (k, hv) in h {
                if top {
                    merge_top_key(l, k, hv);
                } else {
                    // Default behavior at lower depth: recurse if both
                    // sides are objects; otherwise higher wins.
                    if let Some(lv) = l.get_mut(&k)
                        && lv.is_object()
                        && hv.is_object()
                    {
                        merge_inner(lv, hv, false);
                        continue;
                    }
                    l.insert(k, hv);
                }
            }
        }
        (slot, Value::Null) => {
            // Explicit null at higher scope clears the value.
            *slot = Value::Null;
        }
        (slot, hv) => {
            *slot = hv;
        }
    }
}

fn merge_top_key(l: &mut Map<String, Value>, k: String, hv: Value) {
    // Permissions block: arrays concatenate, scalars override.
    if k == "permissions" {
        if !l.contains_key(&k) {
            l.insert(k, hv);
            return;
        }
        if let (Some(Value::Object(lo)), Value::Object(ho)) = (l.get_mut(&k), hv) {
            merge_permissions(lo, ho);
        }
        return;
    }
    // Dedupe-style arrays: concatenate + dedupe.
    if DEDUPE_ARRAY_KEYS.contains(&k.as_str()) {
        if let (Some(Value::Array(la)), Value::Array(ha)) = (l.get_mut(&k), &hv) {
            for item in ha {
                if !la.contains(item) {
                    la.push(item.clone());
                }
            }
            return;
        }
        l.insert(k, hv);
        return;
    }
    // Named-object containers (mcp_servers, env, model_overrides):
    // deep-merge per name.
    if NAMED_OBJECT_KEYS.contains(&k.as_str()) {
        if !l.contains_key(&k) {
            l.insert(k, hv);
            return;
        }
        if let (Some(Value::Object(lo)), Value::Object(ho)) = (l.get_mut(&k), hv) {
            for (name, hentry) in ho {
                if let Some(lentry) = lo.get_mut(&name) {
                    if lentry.is_object() && hentry.is_object() {
                        merge_inner(lentry, hentry, false);
                    } else {
                        *lentry = hentry;
                    }
                } else {
                    lo.insert(name, hentry);
                }
            }
        }
        return;
    }
    // Hooks: array values per event concatenate.
    if k == "hooks" {
        if !l.contains_key(&k) {
            l.insert(k, hv);
            return;
        }
        if let (Some(Value::Object(lo)), Value::Object(ho)) = (l.get_mut(&k), hv) {
            for (event, hv2) in ho {
                match (lo.get_mut(&event), hv2) {
                    (Some(Value::Array(la)), Value::Array(ha)) => {
                        la.extend(ha);
                    }
                    (_, hv2) => {
                        lo.insert(event, hv2);
                    }
                }
            }
        }
        return;
    }

    // Default: object → recurse; scalar/array → higher wins.
    let need_recurse = l.get(&k).is_some_and(|lv| lv.is_object() && hv.is_object());
    if need_recurse {
        if let Some(lv) = l.get_mut(&k) {
            merge_inner(lv, hv, false);
        }
    } else {
        l.insert(k, hv);
    }
}

fn merge_permissions(lo: &mut Map<String, Value>, ho: Map<String, Value>) {
    for (k, hv) in ho {
        if PERMISSION_LIST_KEYS.contains(&k.as_str())
            && let (Some(Value::Array(la)), Value::Array(ha)) = (lo.get_mut(&k), &hv)
        {
            // Concatenate: lower entries first, then higher. Dedupe is
            // intentionally skipped (the design wants explicit first-
            // match-wins ordering at evaluation time).
            for item in ha {
                la.push(item.clone());
            }
            continue;
        }
        // Scalars + the future-compat `extra` map → higher wins.
        lo.insert(k, hv);
    }
}

// ---------------------------------------------------------------------------
// Diff (for live-reload)
// ---------------------------------------------------------------------------

/// Restart impact for a changed key — fed into the `ConfigChange` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartImpact {
    /// Live-reloadable; the new value is in effect immediately.
    Hot,
    /// Restart required; the new value is logged and held until next
    /// launch.
    Restart,
}

/// One delta in the diff between two `Settings` snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedKey {
    /// Dotted top-level key (e.g. `"model"`, `"permissions.allow"`).
    pub key: String,
    /// Whether the change is live-reloadable.
    pub impact: RestartImpact,
}

const RESTART_REQUIRED: &[&str] = &[
    "model",
    "fallback_model",
    "agent",
    "router",
    "mcp_servers",
    "memory",
    "output_style",
];

/// Compute the diff between two settings snapshots.
///
/// The result is a list of top-level keys whose serde-JSON
/// representation changed. The `permissions` block decomposes to
/// `permissions.allow|ask|deny`. Each entry carries its `RestartImpact`.
#[must_use]
pub fn diff_settings(old: &Settings, new: &Settings) -> Vec<ChangedKey> {
    let old_v = serde_json::to_value(old).unwrap_or(Value::Null);
    let new_v = serde_json::to_value(new).unwrap_or(Value::Null);
    let mut out = Vec::new();
    if let (Value::Object(o), Value::Object(n)) = (&old_v, &new_v) {
        let mut keys: std::collections::BTreeSet<&String> = o.keys().collect();
        keys.extend(n.keys());
        for key in keys {
            if key == "permissions" {
                // Decompose into sub-keys.
                let empty = Value::Object(Map::new());
                let op = o.get(key).unwrap_or(&empty);
                let np = n.get(key).unwrap_or(&empty);
                if let (Value::Object(om), Value::Object(nm)) = (op, np) {
                    for sub in ["allow", "ask", "deny"] {
                        if om.get(sub) != nm.get(sub) {
                            out.push(ChangedKey {
                                key: format!("permissions.{sub}"),
                                impact: RestartImpact::Hot,
                            });
                        }
                    }
                }
                continue;
            }
            if o.get(key) != n.get(key) {
                let impact = if RESTART_REQUIRED.contains(&key.as_str()) {
                    RestartImpact::Restart
                } else {
                    RestartImpact::Hot
                };
                out.push(ChangedKey {
                    key: key.clone(),
                    impact,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn scalar_higher_wins() {
        let mut lower = json!({"max_tokens": 1024});
        merge_values(&mut lower, json!({"max_tokens": 2048}));
        assert_eq!(lower["max_tokens"], json!(2048));
    }

    #[test]
    fn permission_arrays_concatenate_in_priority_order() {
        // lower (user) = ["Read"]; higher (project) = ["Bash"].
        // After merge: lower-first, higher appended → ["Read", "Bash"].
        let mut lower = json!({"permissions": {"allow": ["Read"]}});
        merge_values(&mut lower, json!({"permissions": {"allow": ["Bash"]}}));
        assert_eq!(lower["permissions"]["allow"], json!(["Read", "Bash"]));
    }

    #[test]
    fn dedupe_arrays_concatenate_and_dedup() {
        let mut lower = json!({"allowed_http_hook_urls": ["https://a"]});
        merge_values(
            &mut lower,
            json!({"allowed_http_hook_urls": ["https://a", "https://b"]}),
        );
        assert_eq!(
            lower["allowed_http_hook_urls"],
            json!(["https://a", "https://b"])
        );
    }

    #[test]
    fn deep_merge_mcp_servers() {
        let mut lower = json!({
            "mcp_servers": { "linear": { "command": "npx", "args": ["-y"] } }
        });
        let higher = json!({
            "mcp_servers": { "linear": { "env": { "TOKEN": "xyz" } } }
        });
        merge_values(&mut lower, higher);
        assert_eq!(lower["mcp_servers"]["linear"]["command"], json!("npx"));
        assert_eq!(lower["mcp_servers"]["linear"]["args"], json!(["-y"]));
        assert_eq!(lower["mcp_servers"]["linear"]["env"]["TOKEN"], json!("xyz"));
    }

    #[test]
    fn nested_object_recurses() {
        let mut lower = json!({"router": {"breaker": {"timeout_ms": 1000}}});
        merge_values(
            &mut lower,
            json!({"router": {"breaker": {"max_failures": 5}}}),
        );
        assert_eq!(lower["router"]["breaker"]["timeout_ms"], json!(1000));
        assert_eq!(lower["router"]["breaker"]["max_failures"], json!(5));
    }

    #[test]
    fn diff_flags_model_as_restart_required() {
        let old = Settings::default();
        let new = Settings {
            model: Some(crate::ModelSelector::Name("claude-sonnet-4-7".into())),
            ..Default::default()
        };
        let d = diff_settings(&old, &new);
        let m = d.iter().find(|c| c.key == "model").expect("model in diff");
        assert_eq!(m.impact, RestartImpact::Restart);
    }

    #[test]
    fn diff_decomposes_permissions() {
        let old = Settings::default();
        let new = Settings {
            permissions: crate::Permissions {
                allow: vec!["Read".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let d = diff_settings(&old, &new);
        assert!(d.iter().any(|c| c.key == "permissions.allow"));
        assert!(d.iter().all(|c| c.key != "permissions"));
    }
}
