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
/// The plan-control tools that must always run while plan mode is active,
/// regardless of side effects: `EnterPlanMode` is idempotent and `ExitPlanMode`
/// is the escape hatch out of plan mode. These are intrinsic framework tools
/// (not user-extensible), so they stay an explicit pair.
///
/// Every *other* tool's plan-mode eligibility is decided by whether it is
/// side-effect-free ([`crate::Tool::is_read_only`]) — read-only built-ins
/// (`Read`, `Grep`, `Glob`, `WebFetch`) and the `Skill` tool override that to
/// `true`. This replaces the previous hardcoded name allowlist so a new
/// read-only built-in or MCP tool becomes plan-safe without a central edit.
const PLAN_CONTROL_TOOLS: &[&str] = &["EnterPlanMode", "ExitPlanMode"];

/// Returns `true` when `name` is a plan-control tool (always allowed in plan
/// mode). Read-only tools are allowed separately by the permission layer via
/// [`crate::Tool::is_read_only`].
#[must_use]
pub fn is_plan_control_tool(name: &str) -> bool {
    PLAN_CONTROL_TOOLS.contains(&name)
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
    fn plan_control_tools_recognized() {
        assert!(is_plan_control_tool("EnterPlanMode"));
        assert!(is_plan_control_tool("ExitPlanMode"));
    }

    #[test]
    fn non_control_tools_are_not_plan_control() {
        // Read-only tools (Read/Grep/Glob/WebFetch/Skill) are allowed in plan
        // mode via Tool::is_read_only, checked by the permission layer — not
        // here. Mutating tools are neither read-only nor plan-control.
        for name in [
            "Read", "Grep", "Glob", "WebFetch", "Skill", "Bash", "Write", "Edit",
        ] {
            assert!(
                !is_plan_control_tool(name),
                "{name} should not be plan-control"
            );
        }
    }
}
