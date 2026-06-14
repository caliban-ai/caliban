//! Skill loader + `SkillTool` for the caliban agent harness.
//!
//! See `docs/superpowers/specs/2026-05-23-skills-design.md` and
//! `adrs/0019-skills-loading.md`.

#![allow(clippy::multiple_crate_versions)]

pub mod builtins;
pub mod loader;
pub mod skill;
pub mod tool;

pub use builtins::{builtin_skills, register as register_builtins};
pub use loader::{
    SkillLoadReport, SkillSkip, default_roots, load_one, load_skills, load_skills_report,
};
pub use skill::Skill;
pub use tool::SkillTool;
