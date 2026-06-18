//! Effort-level helpers. The mapping from `EffortLevel` → provider-specific
//! knob is recorded on each route's `effort_map`; this module exposes the
//! lookup so adapters (and the `caliban router debug --effort-table`
//! subcommand) can render the resolved values.

use crate::config::{EffortLevel, RouteEntry};

/// Resolve an effort level for the request based on (a) any per-request
/// override (derived from `CompletionRequest.effort` in the dispatch path, or
/// passed in from the CLI/headless layer for `router debug`), or (b) the
/// route's pinned default, or (c) `EffortLevel::Medium` as a final fallback.
#[must_use]
pub fn effective_effort_for(route: &RouteEntry, per_request: Option<EffortLevel>) -> EffortLevel {
    per_request.or(route.effort).unwrap_or(EffortLevel::Medium)
}

/// Map a router [`EffortLevel`] to the provider-shared `Effort` enum that
/// rides on `CompletionRequest.effort` and is read by each adapter (#173).
#[must_use]
pub fn effort_level_to_provider(level: EffortLevel) -> caliban_provider::Effort {
    match level {
        EffortLevel::Low => caliban_provider::Effort::Low,
        EffortLevel::Medium => caliban_provider::Effort::Medium,
        EffortLevel::High => caliban_provider::Effort::High,
    }
}

/// The provider-specific knob string the route maps the effort level to.
/// Defaults to the level slug (`"low"`/`"medium"`/`"high"`) when the route
/// has no explicit map.
#[must_use]
pub fn effort_knob_for(route: &RouteEntry, level: EffortLevel) -> &str {
    route.effort_knob_for(level)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        BreakerPolicy, CapabilityRequirements, EffortMap, HedgePolicy, RouteEntry,
    };
    use caliban_provider::RequestPurpose;

    fn route_with_effort(effort: Option<EffortLevel>, map: EffortMap) -> RouteEntry {
        RouteEntry {
            id: "r".into(),
            purpose: RequestPurpose::MainLoop,
            provider: "anthropic".into(),
            model: "m".into(),
            requires: CapabilityRequirements::default(),
            fallback: None,
            hedge: HedgePolicy::Disabled,
            breaker: BreakerPolicy::disabled(),
            effort,
            effort_map: map,
        }
    }

    #[test]
    fn per_route_effort_map_is_honored() {
        let r = route_with_effort(
            Some(EffortLevel::High),
            EffortMap {
                low: Some("budget=0".into()),
                medium: Some("budget=4096".into()),
                high: Some("budget=16384".into()),
            },
        );
        let level = effective_effort_for(&r, None);
        assert_eq!(level, EffortLevel::High);
        assert_eq!(effort_knob_for(&r, level), "budget=16384");
    }

    #[test]
    fn per_request_overrides_route_default() {
        let r = route_with_effort(Some(EffortLevel::High), EffortMap::default());
        let level = effective_effort_for(&r, Some(EffortLevel::Low));
        assert_eq!(level, EffortLevel::Low);
        // No map => slug fallback.
        assert_eq!(effort_knob_for(&r, level), "low");
    }

    #[test]
    fn falls_back_to_medium_when_unspecified() {
        let r = route_with_effort(None, EffortMap::default());
        assert_eq!(effective_effort_for(&r, None), EffortLevel::Medium);
    }
}
