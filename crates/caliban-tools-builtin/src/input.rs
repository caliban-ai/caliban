//! Shared tool-input deserialization.
//!
//! Every built-in tool deserializes its JSON `input` into a typed struct and
//! maps a `serde` failure onto [`ToolError::InvalidInput`]. That one line had
//! been copy-pasted ~18 times and had drifted: most tools wrapped the message
//! as `format!("invalid input: {e}")` — which, because `ToolError`'s `Display`
//! already prefixes `"invalid input: "`, surfaced a doubled
//! `"invalid input: invalid input: …"` — while `bash`/`tool_search` used the
//! bare `e.to_string()`. This helper unifies them on the correct single-prefix
//! form.

use caliban_agent_core::ToolError;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Deserialize a tool's JSON `input` into `T`, mapping a `serde` failure onto
/// [`ToolError::InvalidInput`].
///
/// The error message is the bare `serde` message; `ToolError`'s `Display`
/// supplies the `"invalid input: "` prefix, so callers must not add their own.
///
/// # Errors
///
/// Returns [`ToolError::InvalidInput`] when `input` does not match `T`.
pub fn parse_input<T: DeserializeOwned>(input: Value) -> Result<T, ToolError> {
    serde_json::from_value(input).map_err(|e| ToolError::invalid_input(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::json;

    #[derive(Debug, Deserialize)]
    struct Demo {
        name: String,
    }

    #[test]
    fn parses_valid_input() {
        let v: Demo = parse_input(json!({ "name": "x" })).unwrap();
        assert_eq!(v.name, "x");
    }

    #[test]
    fn maps_failure_to_invalid_input_without_doubled_prefix() {
        let err = parse_input::<Demo>(json!({ "name": 5 })).unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
        // Display prefixes once; the payload must not repeat it.
        let shown = err.to_string();
        assert!(shown.starts_with("invalid input: "), "got {shown:?}");
        assert!(
            !shown.contains("invalid input: invalid input:"),
            "doubled prefix: {shown:?}",
        );
    }
}
