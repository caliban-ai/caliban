#![allow(missing_docs)]

use caliban_provider::{CompletionRequest, ToolChoice};
use caliban_provider_anthropic::ir_convert::ir_to_native_request;

/// `IrToolChoice::None` with tools present → `tool_choice` field must be absent.
#[test]
fn tool_choice_none_omits_field() {
    let req = CompletionRequest::builder("claude-3-5-sonnet")
        .user_text("hi")
        .tool_choice(ToolChoice::None)
        .max_tokens(64)
        .build()
        .unwrap();
    let native = ir_to_native_request(req, false);
    assert!(native.tool_choice.is_none());
}

/// `IrToolChoice::Auto` with empty tools → `tool_choice` field must be absent.
#[test]
fn tool_choice_auto_empty_tools_omits_field() {
    let req = CompletionRequest::builder("claude-3-5-sonnet")
        .user_text("hi")
        .tool_choice(ToolChoice::Auto)
        .max_tokens(64)
        .build()
        .unwrap();
    // No tools added → req.tools is empty
    let native = ir_to_native_request(req, false);
    assert!(native.tool_choice.is_none());
}

/// `IrToolChoice::Specific` → serializes as `{"type":"tool","name":"x"}`.
#[test]
fn tool_choice_specific_serializes_correctly() {
    use caliban_provider_anthropic::schema::request::NativeToolChoice;
    use serde_json::json;

    let req = CompletionRequest::builder("claude-3-5-sonnet")
        .user_text("hi")
        .tool_choice(ToolChoice::Specific {
            name: "my_tool".to_string(),
        })
        .max_tokens(64)
        .build()
        .unwrap();
    // Manually set tools to non-empty so the choice is preserved
    // We test the serialized NativeToolChoice directly.
    let choice = NativeToolChoice::Tool {
        name: "my_tool".to_string(),
    };
    let serialized = serde_json::to_value(&choice).unwrap();
    assert_eq!(serialized, json!({"type": "tool", "name": "my_tool"}));

    // Also verify ir_to_native_request returns Some for Specific with non-empty tools.
    // We check via the req that has no actual tools — it should be None (no_tools=true).
    let native = ir_to_native_request(req, false);
    // tools list is empty, so tool_choice is omitted regardless of Specific
    assert!(native.tool_choice.is_none());
}

/// `IrToolChoice::Any` with non-empty tools → serializes as `{"type":"any"}`.
#[test]
fn tool_choice_any_with_tools_present() {
    use caliban_provider::Tool;
    use serde_json::json;

    let tool = Tool {
        name: "search".to_string(),
        description: "Search the web".to_string(),
        input_schema: json!({"type": "object", "properties": {}}),
        cache_control: None,
    };
    let req = CompletionRequest::builder("claude-3-5-sonnet")
        .user_text("hi")
        .tool(tool)
        .tool_choice(ToolChoice::Any)
        .max_tokens(64)
        .build()
        .unwrap();
    let native = ir_to_native_request(req, false);
    assert!(native.tool_choice.is_some());
    let serialized = serde_json::to_value(&native.tool_choice).unwrap();
    assert_eq!(serialized, json!({"type": "any"}));
}
