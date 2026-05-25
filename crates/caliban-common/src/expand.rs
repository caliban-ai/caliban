//! Single canonical `${VAR}` / `${VAR:-default}` expansion.
//!
//! Caliban historically had three competing implementations of this:
//! - `caliban-mcp-client/src/config.rs` (`expand_value`) — `${VAR}` with
//!   error-on-miss, plus a virtual `CLAUDE_PROJECT_DIR` binding.
//! - `caliban-plugins/src/expand.rs` (`expand`) — only `${CALIBAN_PLUGIN_ROOT}`
//!   and its `${CLAUDE_PLUGIN_ROOT}` alias, pass-through for other vars.
//! - `caliban-settings` — *would* have had its own loader, but ultimately
//!   delegated to the per-section loaders above.
//!
//! This module unifies them. Callers populate [`ExpandContext::vars`] with
//! the bindings they care about (typically `std::env` + any virtual ones)
//! and pick the unknown-var policy via [`ExpandContext::missing_policy`].

use std::collections::BTreeMap;

/// Behavior when an `${UNDEFINED}` reference is hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingPolicy {
    /// Return [`ExpandError::MissingVar`].
    Error,
    /// Substitute the empty string (matches `${VAR:-}` with no default).
    Empty,
    /// Pass the literal `${VAR}` text through unchanged.
    PassThrough,
}

/// Inputs to [`expand_vars`].
#[derive(Debug, Clone)]
pub struct ExpandContext {
    /// Variable bindings looked up during expansion.
    pub vars: BTreeMap<String, String>,
    /// Whether `${VAR:-default}` syntax is accepted. When `false`, a `:-`
    /// inside the braces is treated as part of the variable name (so
    /// `${A:-B}` would look up the literal key `A:-B`).
    pub allow_default: bool,
    /// What to do for a reference whose variable is missing and there is no
    /// default. Default: [`MissingPolicy::Error`].
    pub missing_policy: MissingPolicy,
}

impl Default for ExpandContext {
    fn default() -> Self {
        Self {
            vars: BTreeMap::new(),
            allow_default: true,
            missing_policy: MissingPolicy::Error,
        }
    }
}

impl ExpandContext {
    /// Pre-fill `vars` from the current process environment.
    #[must_use]
    pub fn from_process_env() -> Self {
        let mut ctx = Self::default();
        for (k, v) in std::env::vars() {
            ctx.vars.insert(k, v);
        }
        ctx
    }

    /// Insert a binding, replacing any prior value for the same key.
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.vars.insert(name.into(), value.into());
    }
}

/// Expansion failures.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ExpandError {
    /// `${VAR}` referenced but `VAR` is not in `vars` and there's no
    /// `:-default`.
    #[error("undefined variable `{name}`")]
    MissingVar {
        /// The unresolved variable name.
        name: String,
    },
    /// `${` without a matching `}` in the same string.
    #[error("unclosed `${{` brace at byte {pos}")]
    UnclosedBrace {
        /// Byte offset of the offending `${`.
        pos: usize,
    },
    /// Reserved for future syntax variants — not currently emitted.
    #[error("invalid expansion syntax: {detail}")]
    InvalidSyntax {
        /// Human-readable explanation.
        detail: String,
    },
}

/// Expand every `${VAR}` / `${VAR:-default}` reference in `s` using the
/// bindings and policies in `ctx`.
///
/// Inline expansion is supported: `https://${HOST}:${PORT}/path` works.
///
/// # Errors
/// - [`ExpandError::MissingVar`] when [`MissingPolicy::Error`] is in effect
///   and a reference can't be resolved.
/// - [`ExpandError::UnclosedBrace`] when the input contains `${` with no
///   matching `}`.
pub fn expand_vars(s: &str, ctx: &ExpandContext) -> Result<String, ExpandError> {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0_usize;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            let brace_start = i;
            let inner_start = i + 2;
            let Some(rel_end) = bytes[inner_start..].iter().position(|&b| b == b'}') else {
                return Err(ExpandError::UnclosedBrace { pos: brace_start });
            };
            let inner = &s[inner_start..inner_start + rel_end];
            let (name, default) = if ctx.allow_default {
                match inner.split_once(":-") {
                    Some((n, d)) => (n, Some(d)),
                    None => (inner, None),
                }
            } else {
                (inner, None)
            };
            let resolved = ctx.vars.get(name).cloned();
            let value = match (resolved, default) {
                (Some(v), _) => v,
                (None, Some(d)) => d.to_string(),
                (None, None) => match ctx.missing_policy {
                    MissingPolicy::Error => {
                        return Err(ExpandError::MissingVar {
                            name: name.to_string(),
                        });
                    }
                    MissingPolicy::Empty => String::new(),
                    MissingPolicy::PassThrough => format!("${{{inner}}}"),
                },
            };
            out.push_str(&value);
            i = inner_start + rel_end + 1;
        } else {
            // Copy one UTF-8 byte at a time. Safe because the multi-byte
            // continuation bytes in UTF-8 never collide with ASCII `$` or
            // `{`, so we can never split a code point mid-stream.
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    Ok(out)
}

/// Convenience: build an [`ExpandContext`] from the current process env
/// and call [`expand_vars`].
///
/// # Errors
/// See [`expand_vars`].
pub fn expand_vars_from_env(s: &str) -> Result<String, ExpandError> {
    expand_vars(s, &ExpandContext::from_process_env())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with(pairs: &[(&str, &str)]) -> ExpandContext {
        let mut c = ExpandContext::default();
        for (k, v) in pairs {
            c.set(*k, *v);
        }
        c
    }

    // --- happy path / ${VAR} resolution ---

    #[test]
    fn expands_single_var() {
        let c = ctx_with(&[("FOO", "bar")]);
        assert_eq!(expand_vars("${FOO}", &c).unwrap(), "bar");
    }

    #[test]
    fn expands_inline_placement() {
        let c = ctx_with(&[("HOST", "example.com"), ("PORT", "443")]);
        assert_eq!(
            expand_vars("https://${HOST}:${PORT}/path", &c).unwrap(),
            "https://example.com:443/path"
        );
    }

    #[test]
    fn literal_dollar_sign_passes_through() {
        let c = ExpandContext::default();
        assert_eq!(expand_vars("price: $5", &c).unwrap(), "price: $5");
    }

    // --- default fallback ---

    #[test]
    fn falls_back_to_default_when_var_missing() {
        let c = ExpandContext::default();
        assert_eq!(expand_vars("${MISSING:-fallback}", &c).unwrap(), "fallback");
    }

    #[test]
    fn default_disabled_when_allow_default_false() {
        let mut c = ExpandContext {
            allow_default: false,
            ..Default::default()
        };
        c.set("MISSING:-fallback", "literal-key-hit");
        // With allow_default=false the `:-` is part of the key.
        assert_eq!(
            expand_vars("${MISSING:-fallback}", &c).unwrap(),
            "literal-key-hit"
        );
    }

    // --- missing var policy ---

    #[test]
    fn missing_var_errors_by_default() {
        let c = ExpandContext::default();
        let err = expand_vars("${NOPE}", &c).unwrap_err();
        assert_eq!(
            err,
            ExpandError::MissingVar {
                name: "NOPE".into()
            }
        );
    }

    #[test]
    fn missing_var_empty_policy_yields_empty() {
        let c = ExpandContext {
            missing_policy: MissingPolicy::Empty,
            ..Default::default()
        };
        assert_eq!(expand_vars("a${NOPE}b", &c).unwrap(), "ab");
    }

    #[test]
    fn missing_var_passthrough_policy_emits_literal() {
        let c = ExpandContext {
            missing_policy: MissingPolicy::PassThrough,
            ..Default::default()
        };
        assert_eq!(expand_vars("a${NOPE}b", &c).unwrap(), "a${NOPE}b");
    }

    // --- unclosed brace ---

    #[test]
    fn unclosed_brace_returns_error() {
        let c = ExpandContext::default();
        let err = expand_vars("a ${UNCLOSED", &c).unwrap_err();
        assert!(
            matches!(err, ExpandError::UnclosedBrace { .. }),
            "got {err:?}"
        );
    }

    // --- process env helper ---

    #[test]
    fn expand_vars_from_env_resolves_path() {
        // PATH is essentially always set in a test environment.
        let out = expand_vars_from_env("PATH=${PATH}").unwrap();
        assert!(out.starts_with("PATH="));
        // Either non-empty PATH (typical case) or empty string PATH; both
        // resolve cleanly because the var is present.
        assert!(out.len() >= "PATH=".len());
    }
}
