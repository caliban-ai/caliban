//! Minimal `--json-schema` support.
//!
//! Native structured-output (Anthropic / `OpenAI` `json_schema` mode) lands
//! with the model router (ADR 0032). Until then we do a best-effort
//! local validation: parse the final assistant text as JSON, then check
//! that any `required` top-level fields exist and (where the schema gives
//! us a `type`) the field types match.
//!
//! This is intentionally *not* a full JSON Schema validator. It covers the
//! ~80% case of "schema asks for `{ ok: bool, message: string }`" without
//! pulling in `jsonschema` as a dep.

use std::path::Path;

use serde_json::Value;

use crate::headless::HeadlessError;

/// A loaded JSON schema (we just keep the raw `Value` and walk it).
#[derive(Debug, Clone)]
pub(crate) struct JsonSchema {
    /// The raw JSON Schema document.
    pub(crate) raw: Value,
}

impl JsonSchema {
    /// Parse a schema from either an inline JSON string or a file path.
    ///
    /// # Errors
    /// - [`HeadlessError::SchemaParse`] on JSON parse failure.
    /// - [`HeadlessError::Io`] on file read failure.
    pub(crate) fn from_cli_arg(arg: &str) -> Result<Self, HeadlessError> {
        // Heuristic: if it starts with '{' or '[' treat as inline JSON.
        let trimmed = arg.trim_start();
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            return Self::parse_str(trimmed);
        }
        let path = Path::new(arg);
        let bytes =
            std::fs::read(path).map_err(|e| HeadlessError::Io(format!("schema file: {e}")))?;
        let s = String::from_utf8(bytes)
            .map_err(|e| HeadlessError::Io(format!("schema file utf-8: {e}")))?;
        Self::parse_str(&s)
    }

    /// Parse a schema from a raw JSON string.
    ///
    /// # Errors
    /// [`HeadlessError::SchemaParse`] on parse failure.
    pub(crate) fn parse_str(s: &str) -> Result<Self, HeadlessError> {
        let raw: Value =
            serde_json::from_str(s).map_err(|e| HeadlessError::SchemaParse(e.to_string()))?;
        Ok(Self { raw })
    }

    /// Validate `candidate` against the schema (best-effort).
    ///
    /// Returns `Ok(())` on a valid match. The error string is a human-
    /// readable explanation suitable for stuffing into the `result` frame.
    ///
    /// # Errors
    /// Returns `Err(<reason>)` when validation fails.
    pub(crate) fn validate(&self, candidate: &Value) -> Result<(), String> {
        // Top-level type check.
        if let Some(schema_type) = self.raw.get("type").and_then(Value::as_str)
            && !type_matches(schema_type, candidate)
        {
            return Err(format!(
                "expected top-level type `{schema_type}`, got `{}`",
                json_type_name(candidate),
            ));
        }

        // Required fields (top-level only).
        let required = self
            .raw
            .get("required")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let Some(obj) = candidate.as_object() else {
            if !required.is_empty() {
                return Err("schema requires fields but candidate is not an object".into());
            }
            return Ok(());
        };
        for name in &required {
            if let Some(n) = name.as_str()
                && !obj.contains_key(n)
            {
                return Err(format!("missing required field `{n}`"));
            }
        }

        // Per-field type check (from properties.<name>.type).
        if let Some(props) = self.raw.get("properties").and_then(Value::as_object) {
            for (k, prop) in props {
                let Some(v) = obj.get(k) else { continue };
                if let Some(t) = prop.get("type").and_then(Value::as_str)
                    && !type_matches(t, v)
                {
                    return Err(format!(
                        "field `{k}` expected type `{t}`, got `{}`",
                        json_type_name(v),
                    ));
                }
            }
        }

        Ok(())
    }
}

fn type_matches(schema_type: &str, v: &Value) -> bool {
    match schema_type {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "number" => v.is_number(),
        "integer" => v.is_i64() || v.is_u64(),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        _ => true, // Unknown type → don't reject.
    }
}

fn json_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Try to extract a candidate JSON object from an assistant's final text
/// reply. Strategy: scan for the first balanced `{...}` block and parse it.
/// Returns `None` if no balanced object is found.
#[must_use]
pub(crate) fn extract_json_object(text: &str) -> Option<Value> {
    // Fast path: the whole reply is JSON.
    if let Ok(v) = serde_json::from_str::<Value>(text.trim()) {
        return Some(v);
    }
    // Slow path: scan for `{`, track brace depth (string-aware).
    let bytes = text.as_bytes();
    let mut start = None;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    for (i, &b) in bytes.iter().enumerate() {
        if in_string {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0
                    && let Some(s) = start
                    && let Ok(v) = serde_json::from_str::<Value>(&text[s..=i])
                {
                    return Some(v);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_inline_object_schema() {
        let s = JsonSchema::from_cli_arg(r#"{"type":"object","required":["ok"]}"#).unwrap();
        assert_eq!(s.raw["type"], "object");
    }

    #[test]
    fn validate_required_field_present() {
        let s = JsonSchema::parse_str(r#"{"type":"object","required":["ok"]}"#).unwrap();
        s.validate(&serde_json::json!({"ok": true})).unwrap();
    }

    #[test]
    fn validate_required_field_missing_fails() {
        let s = JsonSchema::parse_str(r#"{"type":"object","required":["ok"]}"#).unwrap();
        let err = s.validate(&serde_json::json!({"other": 1})).unwrap_err();
        assert!(err.contains("missing required field"));
    }

    #[test]
    fn validate_top_level_type_mismatch() {
        let s = JsonSchema::parse_str(r#"{"type":"object"}"#).unwrap();
        let err = s.validate(&serde_json::json!("plain string")).unwrap_err();
        assert!(err.contains("expected top-level type"));
    }

    #[test]
    fn validate_field_type_mismatch() {
        let s =
            JsonSchema::parse_str(r#"{"type":"object","properties":{"ok":{"type":"boolean"}}}"#)
                .unwrap();
        let err = s.validate(&serde_json::json!({"ok": "yes"})).unwrap_err();
        assert!(err.contains("field `ok` expected type `boolean`"));
    }

    #[test]
    fn validate_field_type_correct() {
        let s = JsonSchema::parse_str(
            r#"{"type":"object","properties":{"ok":{"type":"boolean"},"n":{"type":"integer"}}}"#,
        )
        .unwrap();
        s.validate(&serde_json::json!({"ok": true, "n": 3}))
            .unwrap();
    }

    #[test]
    fn extract_json_object_from_plain_json() {
        let v = extract_json_object(r#"{"a":1}"#).unwrap();
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn extract_json_object_finds_embedded() {
        let v = extract_json_object("prefix text {\"a\":1,\"b\":\"x\"} suffix").unwrap();
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"], "x");
    }

    #[test]
    fn extract_json_object_returns_none_when_absent() {
        assert!(extract_json_object("no json here").is_none());
    }
}
