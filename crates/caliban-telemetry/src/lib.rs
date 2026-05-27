//! Caliban telemetry: OTLP export + cost accounting + context-window tracker.
//!
//! Implements ADR 0033. See `docs/superpowers/specs/2026-05-24-otel-and-cost-design.md`.
//!
//! The crate is split into:
//!
//! - [`init::Telemetry`] — the entry point read by the binary at startup.
//! - [`cost::CostAccumulator`] — session-scoped USD ledger using `rust_decimal`.
//! - [`cost::RateCard`] — parsed YAML rate card (vendored at crate root).
//! - [`context::ContextWindow`] — context-window utilization tracker; powers
//!   the status-bar percent indicator and `/context` overlay.
//! - [`metrics::MetricEmitter`] — typed wrappers around the standard metric
//!   set (`caliban.session.count`, `caliban.cost.usage`, etc.).
//! - [`attrs::StandardAttrs`] — standard attribute set + cardinality knobs.
//! - [`headers`] — `otel_headers_helper` integration.
//!
//! ## Env-var contract
//!
//! Master switch: `CALIBAN_ENABLE_TELEMETRY=1`. Privacy opt-outs
//! `DISABLE_TELEMETRY=1` / `DO_NOT_TRACK=1` force-disable even when the
//! master switch is on. The full `OTEL_*` contract is adopted verbatim from
//! Claude Code. See `TelemetryConfig::from_env` for the parsed knobs.
//!
//! ## Tests
//!
//! `cargo test -p caliban-telemetry` runs ~30 unit tests covering rate-card
//! parsing, cost math, context-window arithmetic, metric emission with the
//! in-memory recorder, headers-helper integration, and the env-var contract.
//! The optional `otlp` feature pulls in the real `opentelemetry-otlp`
//! pipeline.

#![cfg_attr(not(test), deny(clippy::print_stdout, clippy::print_stderr))]
// Domain acronyms (OTel, OTLP, UUIDv4, UUIDv7, mTLS, OAuth, FinOps) appear
// frequently in spec-derived prose. The workspace's pedantic `doc_markdown`
// lint flags them; we allow at crate level rather than backtick-spam every
// doc string.
#![allow(clippy::doc_markdown)]
// `TelemetryConfig` has several independent bool fields (one per env-var
// content-control + cardinality knob). The pedantic `struct_excessive_bools`
// lint fires but those bools are intentional, not a refactoring opportunity.
#![allow(clippy::struct_excessive_bools)]
// Every accessor that touches an `Arc<Mutex<…>>` could in principle panic on
// a poisoned mutex, but in practice the only writers are inside this crate
// and we always `expect("…poisoned")` with a clear message. Annotating each
// reader with `# Panics` clutters the docs without adding information.
#![allow(clippy::missing_panics_doc)]

pub mod attrs;
pub mod compaction;
pub mod context;
pub mod cost;
pub mod error;
pub mod headers;
pub mod init;
pub mod metrics;

pub use attrs::{StandardAttrs, anonymous_user_id, env_truthy_default, privacy_opt_out};
pub use context::{
    ContextBin, ContextBreakdown, ContextWindow, MessageKind, format_capacity_short,
    format_status_segment,
};
pub use cost::{
    CostAccumulator, CostBreakdown, ModelCost, QuerySource, RateCard, RateCardFile, RateRule,
};
pub use error::TelemetryError;
pub use headers::{
    HeadersHelperConfig, merge_headers, parse_helper_output, parse_otlp_headers_env,
};
pub use init::{Telemetry, TelemetryConfig};
pub use metrics::{InMemoryRecorder, MetricEmitter, RecordedMetric};
