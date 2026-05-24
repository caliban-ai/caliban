//! Plan-mode flag — shared between the EnterPlanMode/ExitPlanMode tools and
//! the agent's dispatcher.
//!
//! See `docs/superpowers/specs/2026-05-23-plan-mode-design.md`.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Shared, atomically-read plan-mode flag. Cheap to clone (it's just an `Arc`).
/// `Relaxed` ordering is fine: this flag is set on one task and observed by
/// the dispatcher in the same async runtime; we don't need stronger memory
/// guarantees.
pub type SharedPlanMode = Arc<AtomicBool>;

/// Construct a new flag initialized to `false`.
#[must_use]
pub fn new_shared_plan_mode() -> SharedPlanMode {
    Arc::new(AtomicBool::new(false))
}

/// Built-in tools that are allowed to run while plan mode is active.
///
/// `EnterPlanMode` is idempotent; `ExitPlanMode` is the escape hatch.
/// Read-only built-ins (`Read`, `Grep`, `Glob`, `WebFetch`) and the `Skill`
/// tool are pure injections of context with no side effects.
pub const PLAN_MODE_ALLOWLIST: &[&str] = &[
    "Read",
    "Grep",
    "Glob",
    "WebFetch",
    "Skill",
    "EnterPlanMode",
    "ExitPlanMode",
];

/// Returns `true` when `name` is in the plan-mode allowlist.
#[must_use]
pub fn is_allowed_in_plan_mode(name: &str) -> bool {
    PLAN_MODE_ALLOWLIST.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn defaults_to_false() {
        let f = new_shared_plan_mode();
        assert!(!f.load(Ordering::Relaxed));
    }

    #[test]
    fn store_visible_across_clones() {
        let a = new_shared_plan_mode();
        let b = Arc::clone(&a);
        b.store(true, Ordering::Relaxed);
        assert!(a.load(Ordering::Relaxed));
    }

    #[test]
    fn allowlist_includes_expected_tools() {
        assert!(is_allowed_in_plan_mode("Read"));
        assert!(is_allowed_in_plan_mode("Grep"));
        assert!(is_allowed_in_plan_mode("Glob"));
        assert!(is_allowed_in_plan_mode("WebFetch"));
        assert!(is_allowed_in_plan_mode("Skill"));
        assert!(is_allowed_in_plan_mode("EnterPlanMode"));
        assert!(is_allowed_in_plan_mode("ExitPlanMode"));
    }

    #[test]
    fn allowlist_excludes_mutating_tools() {
        assert!(!is_allowed_in_plan_mode("Bash"));
        assert!(!is_allowed_in_plan_mode("Write"));
        assert!(!is_allowed_in_plan_mode("Edit"));
        assert!(!is_allowed_in_plan_mode("TodoWrite"));
    }
}
