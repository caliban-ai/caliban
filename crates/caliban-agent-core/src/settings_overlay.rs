//! Overlay `caliban-settings` values onto a fresh [`AgentConfig`].
//!
//! These projections used to live on `caliban_settings::Settings` as
//! `apply_*` methods, which forced `caliban-settings` to depend **upward** on
//! `caliban-agent-core` to name [`AgentConfig`]. Epic #539 / ADR 0061 inverts
//! that: the projections live here (the crate that owns [`AgentConfig`]) and
//! read `&Settings` downward, so `caliban-settings` no longer depends on this
//! crate.

use caliban_settings::Settings;

use crate::AgentConfig;

/// Apply context-window management knobs onto a fresh [`AgentConfig`]. Only
/// fields explicitly set in `settings.json` override the defaults; everything
/// else is left at the upstream default (see [`AgentConfig::default`]).
pub fn apply_context_management(settings: &Settings, cfg: &mut AgentConfig) {
    if let Some(v) = settings.auto_compact_threshold {
        cfg.auto_compact_threshold = Some(v);
    }
    if let Some(v) = settings.micro_compact_enabled {
        cfg.micro_compact_enabled = v;
    }
    if let Some(v) = settings.tool_result_cap_chars {
        cfg.tool_result_cap_chars = v;
    }
    if let Some(v) = settings.min_cache_block_tokens {
        cfg.min_cache_block_tokens = v;
    }
}

/// Apply stream-watchdog knobs onto a fresh [`AgentConfig`]. Only fields
/// explicitly set in settings override the defaults. See #263 / #254.
pub fn apply_stream_watchdog(settings: &Settings, cfg: &mut AgentConfig) {
    if let Some(v) = settings.stream_idle_timeout_ms {
        cfg.stream_idle_timeout_ms = v;
    }
    if let Some(v) = settings.stream_prefill_timeout_ms {
        cfg.stream_prefill_timeout_ms = v;
    }
}

/// Apply the `[agent_loop]` spiral-containment guards onto a fresh
/// [`AgentConfig`] (ADR 0058, B1 · #661). Only fields explicitly set in
/// settings override the defaults; everything else is left at the upstream
/// default (see [`AgentConfig::default`]), so an absent group is
/// behavior-preserving.
///
/// **`max_turns` is intentionally not applied here.** It carries a CLI flag
/// (`--max-turns`) that must win over settings, so the caller
/// (`startup::compose`) resolves it with the CLI > settings > default idiom via
/// `Settings::agent_loop_max_turns`. Folding it into this overlay would let a
/// settings value silently override an explicit CLI flag.
pub fn apply_agent_loop(settings: &Settings, cfg: &mut AgentConfig) {
    let Some(al) = settings.agent_loop.as_ref() else {
        return;
    };
    if let Some(v) = al.no_edit_nudge_threshold {
        cfg.no_edit_nudge_threshold = v;
    }
    if let Some(v) = al.empty_turn_nudge_max {
        cfg.empty_turn_nudge_max = v;
    }
    if let Some(v) = al.max_turn_thinking_chars {
        cfg.max_turn_thinking_chars = v;
    }
    // B2 (#662): a positive value sets the wall-clock deadline; `0` means
    // "no deadline" (disabled) so a config value cannot accidentally set a
    // zero-second budget that terminates before the first turn.
    if let Some(secs) = al.time_budget_secs {
        cfg.time_budget = (secs > 0).then(|| std::time::Duration::from_secs(secs));
    }
    // B3 (#663): a positive value sets the cost cap; `<= 0` disables it so a
    // config value cannot impose a zero-dollar budget that stops every run.
    // Enforcement additionally requires an injected cost model (the binary
    // supplies one); the cap is inert without pricing.
    if let Some(usd) = al.cost_budget_usd {
        cfg.cost_budget_usd = (usd > 0.0).then_some(usd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_context_management_overrides_each_field() {
        // Round-trip a settings.toml fragment that sets all four Plan B
        // context-management knobs to non-default values, then assert
        // apply_context_management copies each onto a fresh AgentConfig.
        // Guards the historical wiring gap (PR #60 added the Settings
        // fields + the helper but never wired the call from build_agent).
        let raw = r#"{
            "auto_compact_threshold": 0.42,
            "micro_compact_enabled": false,
            "tool_result_cap_chars": 12345,
            "min_cache_block_tokens": 789
        }"#;
        let s: caliban_settings::Settings = serde_json::from_str(raw).unwrap();
        let mut cfg = AgentConfig::default();
        apply_context_management(&s, &mut cfg);
        assert!((cfg.auto_compact_threshold.unwrap() - 0.42_f32).abs() < 1e-6);
        assert!(!cfg.micro_compact_enabled);
        assert_eq!(cfg.tool_result_cap_chars, 12_345);
        assert_eq!(cfg.min_cache_block_tokens, 789);
    }
    #[test]
    fn apply_context_management_leaves_defaults_when_unset() {
        // No knobs set → AgentConfig::default() values survive untouched.
        let s: caliban_settings::Settings = serde_json::from_str(r"{}").unwrap();
        let mut cfg = AgentConfig::default();
        let snap_threshold = cfg.auto_compact_threshold;
        let snap_micro = cfg.micro_compact_enabled;
        let snap_cap = cfg.tool_result_cap_chars;
        let snap_min = cfg.min_cache_block_tokens;
        apply_context_management(&s, &mut cfg);
        assert_eq!(cfg.auto_compact_threshold, snap_threshold);
        assert_eq!(cfg.micro_compact_enabled, snap_micro);
        assert_eq!(cfg.tool_result_cap_chars, snap_cap);
        assert_eq!(cfg.min_cache_block_tokens, snap_min);
    }
    #[test]
    fn apply_agent_loop_cost_budget_positive_sets_and_nonpositive_disables() {
        let s: caliban_settings::Settings =
            serde_json::from_str(r#"{"agent_loop": {"cost_budget_usd": 1.25}}"#).unwrap();
        let mut cfg = AgentConfig::default();
        assert_eq!(cfg.cost_budget_usd, None, "default is no cost cap");
        apply_agent_loop(&s, &mut cfg);
        assert_eq!(cfg.cost_budget_usd, Some(1.25));

        // `0` (or negative) disables — never a zero-dollar budget.
        let s: caliban_settings::Settings =
            serde_json::from_str(r#"{"agent_loop": {"cost_budget_usd": 0}}"#).unwrap();
        let mut cfg = AgentConfig::default();
        apply_agent_loop(&s, &mut cfg);
        assert_eq!(cfg.cost_budget_usd, None);
    }
    #[test]
    fn apply_agent_loop_time_budget_positive_sets_and_zero_disables() {
        // A positive value becomes a Duration deadline.
        let s: caliban_settings::Settings =
            serde_json::from_str(r#"{"agent_loop": {"time_budget_secs": 300}}"#).unwrap();
        let mut cfg = AgentConfig::default();
        assert_eq!(cfg.time_budget, None, "default is no deadline");
        apply_agent_loop(&s, &mut cfg);
        assert_eq!(cfg.time_budget, Some(std::time::Duration::from_mins(5)));

        // `0` explicitly disables (maps to None), never a zero-second budget.
        let s: caliban_settings::Settings =
            serde_json::from_str(r#"{"agent_loop": {"time_budget_secs": 0}}"#).unwrap();
        let mut cfg = AgentConfig::default();
        apply_agent_loop(&s, &mut cfg);
        assert_eq!(cfg.time_budget, None);
    }
    #[test]
    fn apply_agent_loop_overrides_each_guard() {
        // Every guard set to a non-default value is copied onto a fresh
        // AgentConfig. max_turns is deliberately NOT applied by this overlay
        // (CLI precedence — see apply_agent_loop docs), so it stays default.
        let raw = r#"{
            "agent_loop": {
                "max_turns": 7,
                "no_edit_nudge_threshold": 4,
                "empty_turn_nudge_max": 1,
                "max_turn_thinking_chars": 9999
            }
        }"#;
        let s: caliban_settings::Settings = serde_json::from_str(raw).unwrap();
        let mut cfg = AgentConfig::default();
        let default_max_turns = cfg.max_turns;
        apply_agent_loop(&s, &mut cfg);
        assert_eq!(cfg.no_edit_nudge_threshold, 4);
        assert_eq!(cfg.empty_turn_nudge_max, 1);
        assert_eq!(cfg.max_turn_thinking_chars, 9999);
        // max_turns is resolved by the caller, not this overlay.
        assert_eq!(cfg.max_turns, default_max_turns);
    }
    #[test]
    fn apply_agent_loop_leaves_defaults_when_unset() {
        // No agent_loop group → AgentConfig::default() guards survive untouched.
        let s: caliban_settings::Settings = serde_json::from_str(r"{}").unwrap();
        let mut cfg = AgentConfig::default();
        let snap_no_edit = cfg.no_edit_nudge_threshold;
        let snap_empty = cfg.empty_turn_nudge_max;
        let snap_thinking = cfg.max_turn_thinking_chars;
        apply_agent_loop(&s, &mut cfg);
        assert_eq!(cfg.no_edit_nudge_threshold, snap_no_edit);
        assert_eq!(cfg.empty_turn_nudge_max, snap_empty);
        assert_eq!(cfg.max_turn_thinking_chars, snap_thinking);
    }
    #[test]
    fn apply_agent_loop_partial_leaves_unset_guards_at_default() {
        // Only one guard set; the others keep their AgentConfig defaults.
        let raw = r#"{"agent_loop": {"no_edit_nudge_threshold": 0}}"#;
        let s: caliban_settings::Settings = serde_json::from_str(raw).unwrap();
        let mut cfg = AgentConfig::default();
        let snap_empty = cfg.empty_turn_nudge_max;
        let snap_thinking = cfg.max_turn_thinking_chars;
        apply_agent_loop(&s, &mut cfg);
        assert_eq!(cfg.no_edit_nudge_threshold, 0);
        assert_eq!(cfg.empty_turn_nudge_max, snap_empty);
        assert_eq!(cfg.max_turn_thinking_chars, snap_thinking);
    }
    #[test]
    fn apply_stream_watchdog_overrides_each_field() {
        let raw = r#"{
            "stream_idle_timeout_ms": 45000,
            "stream_prefill_timeout_ms": 600000
        }"#;
        let s: caliban_settings::Settings = serde_json::from_str(raw).unwrap();
        let mut cfg = AgentConfig::default();
        apply_stream_watchdog(&s, &mut cfg);
        assert_eq!(cfg.stream_idle_timeout_ms, 45_000);
        assert_eq!(cfg.stream_prefill_timeout_ms, 600_000);
    }
    #[test]
    fn apply_stream_watchdog_leaves_defaults_when_unset() {
        let s: caliban_settings::Settings = serde_json::from_str(r"{}").unwrap();
        let mut cfg = AgentConfig::default();
        let snap_idle = cfg.stream_idle_timeout_ms;
        let snap_prefill = cfg.stream_prefill_timeout_ms;
        apply_stream_watchdog(&s, &mut cfg);
        assert_eq!(cfg.stream_idle_timeout_ms, snap_idle);
        assert_eq!(cfg.stream_prefill_timeout_ms, snap_prefill);
    }
}
