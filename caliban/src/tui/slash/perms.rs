//! Permissions commands: `/permissions`.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use super::{SlashCommand, SlashCommandMeta, SlashCommandRegistry, SlashCtx, SlashOutcome};

/// `/permissions` — open the permissions overlay.
pub(crate) struct PermissionsCommand;

#[async_trait]
impl SlashCommand for PermissionsCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/permissions",
            description: "edit permission rules; see the effective rule for a focused tool",
            args_hint: "",
            hidden: false,
            immediate: true,
        }
    }
    async fn execute(&self, _args: &str, _ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        // No dedicated overlay variant yet — surface a status message that
        // points the operator at the Settings hierarchy spec which adds
        // the proper Permissions tab.
        Ok(SlashOutcome::StatusMessage(
            "/permissions \u{2014} full overlay lands with the Settings hierarchy spec; use Shift+Tab to cycle permission modes for now".into(),
        ))
    }
}

pub(crate) fn register(registry: &mut SlashCommandRegistry) {
    registry.register(Arc::new(PermissionsCommand));
}
