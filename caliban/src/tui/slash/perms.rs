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
            description: "view current mode + runtime rules; Tab cycles mode, d deletes rule",
            args_hint: "",
            hidden: false,
            immediate: true,
        }
    }
    async fn execute(&self, _args: &str, _ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        // Opens the Permissions overlay — shows current mode +
        // bypass-latch + runtime rules added via the Ask modal's
        // "Always allow/reject" branches. Tab cycles mode; `d`
        // removes the selected rule. See
        // `caliban/src/tui/overlay.rs::permissions_lines` and
        // `caliban/src/tui/events.rs::handle_permissions_overlay_key`.
        Ok(SlashOutcome::Overlay(crate::tui::Overlay::Permissions))
    }
}

pub(crate) fn register(registry: &mut SlashCommandRegistry) {
    registry.register(Arc::new(PermissionsCommand));
}
