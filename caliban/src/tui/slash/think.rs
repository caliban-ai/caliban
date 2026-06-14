//! `/think` — runtime extended-thinking control, decoupled from `/effort`
//! (ticket #100). Mirrors [`super::model::EffortCommand`]: the parsed
//! [`caliban_provider::ThinkingSetting`] is stored into the Agent's swappable
//! `AgentConfig.thinking` and takes effect on the next assistant turn.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use caliban_provider::ThinkingSetting;

use super::{SlashCommand, SlashCommandMeta, SlashCommandRegistry, SlashCtx, SlashOutcome};

/// Parse a `/think` argument into a [`ThinkingSetting`].
///
/// Grammar: `on` | `off` | `auto` | `on <budget>` | `<budget>`. The caller
/// handles the empty-argument case (report current state) before calling this.
///
/// # Errors
///
/// Returns a user-facing message string when the argument is not recognized.
fn parse_thinking_arg(arg: &str) -> std::result::Result<ThinkingSetting, String> {
    let lowered = arg.trim().to_ascii_lowercase();
    match lowered.as_str() {
        "auto" => Ok(ThinkingSetting::Auto),
        "off" => Ok(ThinkingSetting::Off),
        "on" => Ok(ThinkingSetting::On(None)),
        other => {
            // Accept `on <budget>` and the bare `<budget>` shorthand.
            let budget = other.strip_prefix("on").unwrap_or(other).trim();
            match budget.parse::<u32>() {
                Ok(0) | Err(_) => Err(format!(
                    "/think: unknown arg `{}` (expected on|off|auto|<budget>)",
                    arg.trim()
                )),
                Ok(n) => Ok(ThinkingSetting::On(Some(n))),
            }
        }
    }
}

/// Human-readable rendering of the current setting for status messages.
fn describe(setting: ThinkingSetting) -> String {
    match setting {
        ThinkingSetting::Auto => "auto (derived from /effort)".into(),
        ThinkingSetting::Off => "off".into(),
        ThinkingSetting::On(None) => "on".into(),
        ThinkingSetting::On(Some(budget)) => format!("on (budget {budget})"),
    }
}

/// `/think <on|off|auto|budget>` — toggle extended thinking independently of
/// reasoning effort. With no argument, reports the current setting.
pub(crate) struct ThinkCommand;

#[async_trait]
impl SlashCommand for ThinkCommand {
    fn meta(&self) -> &SlashCommandMeta {
        &SlashCommandMeta {
            name: "/think",
            description: "toggle extended thinking (on|off|auto|<budget>), independent of /effort",
            args_hint: "<on|off|auto|budget>",
            hidden: false,
            immediate: true,
        }
    }

    async fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
        let trimmed = args.trim();
        if trimmed.is_empty() {
            let current = *ctx.app.agent.config().thinking.load_full();
            return Ok(SlashOutcome::StatusMessage(format!(
                "thinking: {}",
                describe(current)
            )));
        }
        match parse_thinking_arg(trimmed) {
            Ok(setting) => {
                ctx.app.agent.config().thinking.store(Arc::new(setting));
                Ok(SlashOutcome::StatusMessage(format!(
                    "thinking \u{2192} {}",
                    describe(setting)
                )))
            }
            Err(msg) => Ok(SlashOutcome::StatusMessage(msg)),
        }
    }
}

pub(crate) fn register(registry: &mut SlashCommandRegistry) {
    registry.register(Arc::new(ThinkCommand));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_auto_off_on() {
        assert_eq!(parse_thinking_arg("auto"), Ok(ThinkingSetting::Auto));
        assert_eq!(parse_thinking_arg("off"), Ok(ThinkingSetting::Off));
        assert_eq!(parse_thinking_arg("on"), Ok(ThinkingSetting::On(None)));
    }

    #[test]
    fn parses_on_with_budget() {
        assert_eq!(
            parse_thinking_arg("on 12000"),
            Ok(ThinkingSetting::On(Some(12_000)))
        );
    }

    #[test]
    fn parses_bare_budget_shorthand() {
        assert_eq!(
            parse_thinking_arg("8192"),
            Ok(ThinkingSetting::On(Some(8_192)))
        );
    }

    #[test]
    fn is_case_insensitive_and_trims() {
        assert_eq!(parse_thinking_arg("  OFF "), Ok(ThinkingSetting::Off));
        assert_eq!(
            parse_thinking_arg("On 4096"),
            Ok(ThinkingSetting::On(Some(4_096)))
        );
    }

    #[test]
    fn rejects_zero_budget_and_garbage() {
        assert!(parse_thinking_arg("0").is_err());
        assert!(parse_thinking_arg("on 0").is_err());
        assert!(parse_thinking_arg("sometimes").is_err());
    }
}
