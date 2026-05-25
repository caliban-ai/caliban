//! JSON-Schema validation against the embedded settings schema.
//!
//! The schema doc lives at `crates/caliban-settings/src/schema.json` and
//! is embedded via [`include_str!`]. Public path is forward-looking
//! (`https://caliban.dev/schemas/settings/v1.json`).

use serde_json::Value;

/// Raw embedded schema JSON.
pub const SCHEMA_JSON: &str = include_str!("schema.json");

/// Validate `value` against the embedded schema.
///
/// Returns the list of human-readable validation messages (empty when
/// the value is valid). Caller decides whether to error or just warn —
/// per ADR 0026, the loader warns and continues.
#[must_use]
pub fn validate_value(value: &Value) -> Vec<String> {
    let schema = match serde_json::from_str::<Value>(SCHEMA_JSON) {
        Ok(s) => s,
        Err(e) => return vec![format!("settings schema is malformed: {e}")],
    };
    // `jsonschema` 0.17's `JSONSchema::compile` is the stable surface.
    let compiled = match jsonschema::JSONSchema::options()
        .with_draft(jsonschema::Draft::Draft7)
        .compile(&schema)
    {
        Ok(s) => s,
        Err(e) => return vec![format!("settings schema failed to compile: {e}")],
    };
    let result = compiled.validate(value);
    match result {
        Ok(()) => Vec::new(),
        Err(errors) => errors
            .map(|e| {
                let path = e.instance_path.to_string();
                if path.is_empty() {
                    e.to_string()
                } else {
                    format!("{path}: {e}")
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_passes() {
        let v: Value = serde_json::from_str(
            r#"{"model": "claude-sonnet-4-7", "permissions": {"allow": ["Read"]}}"#,
        )
        .unwrap();
        let errs = validate_value(&v);
        assert!(errs.is_empty(), "expected no errors, got {errs:?}");
    }

    #[test]
    fn unknown_top_level_keys_are_tolerated() {
        let v: Value = serde_json::from_str(r#"{"future_key": 7}"#).unwrap();
        assert!(validate_value(&v).is_empty());
    }

    #[test]
    fn invalid_type_reported() {
        // permissions.allow must be array, not string.
        let v: Value = serde_json::from_str(r#"{"permissions": {"allow": "Read"}}"#).unwrap();
        let errs = validate_value(&v);
        assert!(!errs.is_empty(), "expected error for wrong type");
    }
}
