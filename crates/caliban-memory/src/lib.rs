//! File-backed memory tiers spliced into the caliban system prompt.
//!
//! See `docs/superpowers/specs/2026-05-23-memory-tier-1-design.md` and
//! `adrs/0018-memory-tier-model.md`; auto-memory extensions live in
//! `docs/superpowers/specs/2026-05-24-auto-memory-design.md` /
//! `adrs/0035-auto-memory.md`; ancestor walk + `@`-imports in
//! `docs/superpowers/specs/2026-05-24-claudemd-ancestry-design.md` /
//! `adrs/0036-claudemd-ancestry-and-imports.md`.

#![allow(clippy::multiple_crate_versions)]

pub mod ancestry_addendum;
pub mod auto;
pub mod config;
pub mod error;
pub mod init_import;
pub mod loader;
pub mod prefix;
pub mod project_imports;
pub mod project_walk;
pub mod rules;
pub mod sanitize;
pub mod walk;

pub use ancestry_addendum::AncestryAddendum;
pub use auto::{TopicDraft, TopicFile, TopicKind, TopicLoader, TopicSummary, strip_html_comments};
pub use config::{MemoryConfig, build_excludes};
pub use error::{MemoryError, Result};
pub use init_import::{INIT_FILENAMES, LegacyRulesFile, scan_init_files};
pub use loader::{estimate_tokens, load};
pub use prefix::{MemoryPrefix, ProjectTier, TierFile, TierFileSource, TierKind};
pub use project_imports::{
    ApprovalCallback, ApprovalMode, ImportAllowlist, ImportApproval, ImportState, MAX_IMPORT_DEPTH,
    parse_import_directive, resolve_imports,
};
pub use project_walk::{ANCESTRY_FILENAMES, WalkStop, walk_ancestors};
pub use rules::{Rule, RuleScope, RuleSet, scan_caliban_rules};
pub use sanitize::sanitize_workspace;
pub use walk::walk_up_for_file;
