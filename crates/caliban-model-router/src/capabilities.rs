//! Derive capability needs from a `CompletionRequest`.

use caliban_provider::{Capabilities, CompletionRequest, ContentBlock, ToolUseCapability};

use crate::config::{CapabilityRequirements, RouteEntry};

/// Capability needs derived from the request itself.
///
/// A route must satisfy these regardless of whether the operator declared
/// `requires.X = true` — a vision-only request can't be answered by a
/// non-vision model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DerivedNeeds {
    /// Request contains at least one `ImageBlock`.
    pub vision: bool,
    /// Request has a non-empty `tools` list.
    pub tool_use: bool,
    /// Request has a `thinking` config attached.
    pub thinking: bool,
}

impl DerivedNeeds {
    /// Compute the needs from a request.
    #[must_use]
    pub fn from_request(req: &CompletionRequest) -> Self {
        let vision = req.messages.iter().any(|m| {
            m.content
                .iter()
                .any(|b| matches!(b, ContentBlock::Image(_)))
        });
        let tool_use = !req.tools.is_empty();
        let thinking = req.thinking.is_some();
        Self {
            vision,
            tool_use,
            thinking,
        }
    }

    /// Render as a compact debug string.
    #[must_use]
    pub fn render(&self) -> String {
        format!(
            "vision={} tool_use={} thinking={}",
            self.vision, self.tool_use, self.thinking
        )
    }
}

/// `true` if `caps` is sufficient to handle a request with `needs`.
#[must_use]
pub fn caps_satisfy_needs(caps: Capabilities, needs: DerivedNeeds) -> bool {
    if needs.vision && !caps.vision {
        return false;
    }
    if needs.thinking && !caps.thinking {
        return false;
    }
    if needs.tool_use && matches!(caps.tool_use, ToolUseCapability::None) {
        return false;
    }
    true
}

/// `true` if the route's declared `requires` block is satisfied by `caps`.
#[must_use]
pub fn caps_satisfy_route_requires(caps: Capabilities, requires: CapabilityRequirements) -> bool {
    if requires.vision && !caps.vision {
        return false;
    }
    if requires.thinking && !caps.thinking {
        return false;
    }
    if requires.tool_use && matches!(caps.tool_use, ToolUseCapability::None) {
        return false;
    }
    true
}

/// `true` if `route.requires` is consistent with `needs` — i.e. the route
/// doesn't *forbid* a needed capability. (A route that doesn't declare
/// vision but is otherwise capable can still answer a vision request — that
/// depends on `caps`, not on `requires`.)
#[must_use]
pub fn route_requires_consistent_with_needs(
    requires: CapabilityRequirements,
    needs: DerivedNeeds,
) -> bool {
    // `requires.X = true` means "this route is for X requests"; if `needs.X`
    // is false but the route requires it, the route is asking for something
    // the request can't supply — skip it.
    if requires.vision && !needs.vision {
        return false;
    }
    if requires.thinking && !needs.thinking {
        return false;
    }
    if requires.tool_use && !needs.tool_use {
        return false;
    }
    true
}

/// Returned by the resolver alongside each route to tell the dispatcher
/// why the candidate was kept (purpose match vs. capability auto-route vs.
/// fallback chain). Used by `caliban router debug`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateOrigin {
    /// Primary route for the request's purpose.
    Primary,
    /// Reached via the primary's `fallback = [...]` list.
    FallbackId,
    /// Reached via implicit fallback (declaration order over same purpose).
    ImplicitFallback,
}

impl CandidateOrigin {
    /// Short label for diagnostic output.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            CandidateOrigin::Primary => "primary",
            CandidateOrigin::FallbackId => "fallback",
            CandidateOrigin::ImplicitFallback => "implicit-fallback",
        }
    }
}

/// Per-candidate annotation surfaced via the diagnostic subcommand.
#[derive(Debug, Clone)]
pub struct CandidateAnnotation {
    /// The route entry id.
    pub route_id: String,
    /// Why the candidate was kept.
    pub origin: CandidateOrigin,
}

impl CandidateAnnotation {
    /// Construct an annotation for a route.
    #[must_use]
    pub fn for_route(route: &RouteEntry, origin: CandidateOrigin) -> Self {
        Self {
            route_id: route.id.clone(),
            origin,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caliban_provider::{
        CompletionRequest, ImageBlock, ImageSource, Message, Role, ThinkingConfig, Tool,
    };
    use serde_json::json;

    fn req_text() -> CompletionRequest {
        CompletionRequest {
            model: "x".into(),
            messages: vec![Message::user_text("hi")],
            tools: vec![],
            tool_choice: caliban_provider::ToolChoice::default(),
            max_tokens: 64,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: vec![],
            thinking: None,
            metadata: Default::default(),
        }
    }

    #[test]
    fn derived_needs_marks_vision_required_when_image_present() {
        let mut req = req_text();
        req.messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Image(ImageBlock {
                source: ImageSource::Url {
                    url: "https://example.com/x.png".into(),
                },
                cache_control: None,
            })],
        }];
        let n = DerivedNeeds::from_request(&req);
        assert!(n.vision);
    }

    #[test]
    fn derived_needs_marks_tool_use_when_tools_set() {
        let mut req = req_text();
        req.tools = vec![Tool {
            name: "T".into(),
            description: "d".into(),
            input_schema: json!({"type":"object"}),
            cache_control: None,
        }];
        let n = DerivedNeeds::from_request(&req);
        assert!(n.tool_use);
        assert!(!n.vision);
    }

    #[test]
    fn derived_needs_marks_thinking_when_config_set() {
        let mut req = req_text();
        req.thinking = Some(ThinkingConfig {
            budget_tokens: 1024,
        });
        let n = DerivedNeeds::from_request(&req);
        assert!(n.thinking);
    }
}
