//! Slash command registry — extensible `SlashCommand` trait, dispatch, and
//! typeahead suggester (ADR 0040).
//!
//! Each slash command is its own `impl SlashCommand` under
//! `caliban/src/tui/slash/<group>.rs`. The registry holds them by name in
//! a `HashMap<&'static str, Arc<dyn SlashCommand>>` and exposes
//! `register`, `suggest`, `dispatch`. The TUI's input bar consults the
//! suggester for typeahead; the dispatcher routes execution.
//!
//! See `docs/superpowers/specs/2026-05-24-slash-command-coverage-design.md`.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

pub(crate) mod basic;
pub(crate) mod config;
pub(crate) mod cost;
pub(crate) mod dx;
pub(crate) mod existing;
pub(crate) mod export;
pub(crate) mod model;
pub(crate) mod observe;
pub(crate) mod perms;
pub(crate) mod session;
pub(crate) mod think;

/// Static metadata that the registry exposes for typeahead, `/help`, and
/// suggester ranking. Held as `&'static` strings so the registry can
/// borrow the meta cheaply across many lookups.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SlashCommandMeta {
    /// The leading-slash command name (e.g. `"/clear"`).
    pub(crate) name: &'static str,
    /// Human-readable description shown in `/help` and typeahead.
    pub(crate) description: &'static str,
    /// Args hint shown next to the description in `/help`. Empty for no-arg
    /// commands.
    pub(crate) args_hint: &'static str,
    /// When `true`, the command is hidden from the typeahead suggester and
    /// from `/help`. Still dispatchable by name.
    pub(crate) hidden: bool,
    /// When `true`, the command can fire while a turn is in flight
    /// (IE1: it doesn't need the model). The submit handler intercepts
    /// immediate commands before the running-turn bail.
    /// Default `false`. See caliban-ai/caliban#13 (immediate slash commands).
    pub(crate) immediate: bool,
}

/// Outcome returned from `SlashCommand::execute`. The caller (`Tui`) acts
/// on this — it never reaches into the command for follow-up state.
#[derive(Debug)]
pub(crate) enum SlashOutcome {
    /// No-op: return control to the input bar.
    Continue,
    /// Exit caliban cleanly.
    Quit,
    /// Pre-fill the next prompt with this text.
    #[allow(dead_code)] // wired in the typeahead refactor (ADR 0040 follow-up).
    InsertText(String),
    /// Open the named overlay.
    Overlay(crate::tui::Overlay),
    /// Reload settings / skills / hooks / mcp from disk.
    #[allow(dead_code)] // wired alongside the Settings hierarchy spec.
    Reload,
    /// Show an ephemeral one-line status message in the transcript.
    StatusMessage(String),
}

/// Pluggable slash command. All impls must be `Send + Sync` so the
/// registry can hand them out across tasks.
#[async_trait]
pub(crate) trait SlashCommand: Send + Sync {
    /// Return the command's static metadata.
    fn meta(&self) -> &SlashCommandMeta;

    /// Execute the command against the running TUI session.
    async fn execute(&self, args: &str, ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome>;
}

/// Operator portal handed to every slash command at dispatch time.
///
/// Commands need mutable access to the running [`App`] (session history,
/// transcript, view state, todos, plan mode). Long-lived registries
/// (provider, model router, MCP, skills, hooks, sub-agent fleet,
/// settings) currently live behind `App` accessors or are loaded from
/// the workspace path; commands reach them through the contained
/// `&mut App`.
///
/// The spec describes `SlashCtx` as a fat struct with twelve fields.
/// We keep that *intent* — a single borrowing portal — but anchor it on
/// `&mut App` so we don't fabricate types that don't yet exist in
/// caliban (e.g. `SkillsRegistry`, `SubagentFleet`). When those types
/// land in their respective specs, they'll be added as additional
/// fields on `SlashCtx` alongside `app`.
pub(crate) struct SlashCtx<'a> {
    /// The running TUI app. Commands reach into this for messages,
    /// transcript, view state, todos, plan mode, agent, hooks, etc.
    pub(crate) app: &'a mut crate::tui::App,
}

/// Central registry of all slash commands. Built once in `App::new`,
/// queried by the typeahead suggester and the dispatcher.
pub(crate) struct SlashCommandRegistry {
    by_name: HashMap<&'static str, Arc<dyn SlashCommand>>,
}

impl std::fmt::Debug for SlashCommandRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlashCommandRegistry")
            .field("count", &self.by_name.len())
            .finish()
    }
}

impl Default for SlashCommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SlashCommandRegistry {
    /// Empty registry. Call [`register_builtin`] to populate.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            by_name: HashMap::new(),
        }
    }

    /// Register a command. If a command with the same name is already
    /// registered, the new one wins and a warning is logged — built-ins
    /// register first, then plugin-supplied commands; the override means
    /// "plugin shadows built-in" is an explicit operator decision.
    pub(crate) fn register(&mut self, cmd: Arc<dyn SlashCommand>) {
        let name = cmd.meta().name;
        if self.by_name.insert(name, cmd).is_some() {
            tracing::warn!(
                command = name,
                "slash command re-registered (overrides built-in)"
            );
        }
    }

    /// Fuzzy-filter to commands whose name (sans the leading `/`) contains
    /// `prefix` as a case-insensitive *subsequence* — so `cfg` matches
    /// `/config` — excluding hidden commands. Results are ranked best-match
    /// first (contiguous, start/word-boundary matches outscore scattered
    /// ones) with alphabetical name as the tiebreak, which keeps plain
    /// prefix matches ordered ahead of looser ones. An empty `prefix`
    /// returns all visible commands alphabetically. (#15)
    pub(crate) fn suggest(&self, prefix: &str) -> Vec<&SlashCommandMeta> {
        let mut scored: Vec<(i32, &SlashCommandMeta)> = self
            .by_name
            .values()
            .map(|c| c.meta())
            .filter(|m| !m.hidden)
            .filter_map(|m| {
                let body = m.name.strip_prefix('/').unwrap_or(m.name);
                fuzzy_score(body, prefix).map(|score| (score, m))
            })
            .collect();
        // Higher score first; alphabetical by name within equal scores.
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(b.1.name)));
        scored.into_iter().map(|(_, m)| m).collect()
    }

    /// Return every visible command's meta, sorted alphabetically. Used
    /// by `/help` to list the live set.
    pub(crate) fn visible_metas(&self) -> Vec<&SlashCommandMeta> {
        let mut out: Vec<&SlashCommandMeta> = self
            .by_name
            .values()
            .map(|c| c.meta())
            .filter(|m| !m.hidden)
            .collect();
        out.sort_by_key(|m| m.name);
        out
    }

    /// Total number of registered commands (visible + hidden). Used by
    /// tests + diagnostics.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.by_name.len()
    }

    /// `true` iff a command with this exact name is registered. Used by
    /// tests and by `/help` to render the canonical command line.
    #[cfg(test)]
    pub(crate) fn contains(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    /// Look up a command's static [`SlashCommandMeta`] by exact name
    /// (e.g. `"/context"`). Returns `None` if no command is registered
    /// under that name. Used by the [`is_immediate_slash`] classifier
    /// so the submit handler can read the `immediate` flag without
    /// going through the full async dispatch path. See
    /// caliban-ai/caliban#13 (immediate slash commands).
    pub(crate) fn lookup_meta(&self, name: &str) -> Option<&SlashCommandMeta> {
        self.by_name.get(name).map(|c| c.meta())
    }

    /// Dispatch the command. Returns `StatusMessage` for an unknown name
    /// so the caller can surface it via the transcript without
    /// special-casing.
    pub(crate) async fn dispatch(
        &self,
        name: &str,
        args: &str,
        ctx: &mut SlashCtx<'_>,
    ) -> Result<SlashOutcome> {
        let Some(cmd) = self.by_name.get(name) else {
            return Ok(SlashOutcome::StatusMessage(format!(
                "unknown command: {name} \u{2014} type /help"
            )));
        };
        cmd.execute(args, ctx).await
    }
}

/// Fuzzy subsequence score of `needle` against `haystack`, matched
/// case-insensitively. Returns `None` when `needle` is not a subsequence of
/// `haystack`; a higher score is a better match. An empty needle scores a
/// neutral `0` (matches everything, leaving alphabetical tiebreak to decide
/// order). Matches at the very start (`+15`) or just after a non-alphanumeric
/// word boundary (`+10`) and contiguous runs (`+8` per adjacent char) score
/// higher; gaps between matched chars are penalized. (#15)
fn fuzzy_score(haystack: &str, needle: &str) -> Option<i32> {
    if needle.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = haystack.chars().flat_map(char::to_lowercase).collect();
    let mut score = 0i32;
    let mut hi = 0usize;
    let mut prev: Option<usize> = None;
    for nc in needle.chars().flat_map(char::to_lowercase) {
        let idx = loop {
            if hi >= hay.len() {
                return None; // needle char not found → not a subsequence
            }
            if hay[hi] == nc {
                break hi;
            }
            hi += 1;
        };
        if idx == 0 {
            score += 15;
        } else if !hay[idx - 1].is_alphanumeric() {
            score += 10;
        }
        match prev {
            Some(p) if idx == p + 1 => score += 8,
            Some(p) => score -= i32::try_from(idx - p - 1).unwrap_or(i32::MAX),
            None => {}
        }
        prev = Some(idx);
        hi = idx + 1;
    }
    Some(score)
}

/// IE1 classifier: returns `true` iff `prompt` is a slash invocation
/// whose command is registered with `immediate: true`. Pure function
/// so the event-handler intercept stays unit-testable. Splits on
/// whitespace to extract the leading slash token (so `/context` and
/// `/context --foo` both classify the same way). Returns `false` for
/// empty prompts, non-slash prompts, unknown commands, or commands
/// whose `immediate` flag is `false`. See
/// caliban-ai/caliban#13 (immediate slash commands).
#[must_use]
pub(crate) fn is_immediate_slash(prompt: &str, registry: &SlashCommandRegistry) -> bool {
    let name = prompt.split_whitespace().next().unwrap_or("");
    if !name.starts_with('/') {
        return false;
    }
    registry.lookup_meta(name).is_some_and(|m| m.immediate)
}

/// Construct the built-in registry — every command shipped with the
/// caliban binary. Called once from `App::new`.
#[must_use]
pub(crate) fn register_builtin() -> SlashCommandRegistry {
    let mut registry = SlashCommandRegistry::new();
    basic::register(&mut registry);
    session::register(&mut registry);
    observe::register(&mut registry);
    config::register(&mut registry);
    cost::register(&mut registry);
    export::register(&mut registry);
    model::register(&mut registry);
    think::register(&mut registry);
    perms::register(&mut registry);
    dx::register(&mut registry);
    existing::register(&mut registry);
    registry
}

/// Tiny `--key=value` arg parser for the few commands that need it.
/// Bare flags (`--force`) map to `"true"`. Quoted values are unquoted.
/// Anything that doesn't start with `--` is silently dropped.
#[must_use]
pub(crate) fn parse_kv_args(s: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for tok in s.split_whitespace() {
        let Some(rest) = tok.strip_prefix("--") else {
            continue;
        };
        if let Some((k, v)) = rest.split_once('=') {
            let v = v.trim_matches(|c: char| c == '"' || c == '\'');
            out.insert(k.to_string(), v.to_string());
        } else {
            out.insert(rest.to_string(), "true".to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal command for registry tests.
    struct Echo {
        meta: SlashCommandMeta,
    }

    #[async_trait]
    impl SlashCommand for Echo {
        fn meta(&self) -> &SlashCommandMeta {
            &self.meta
        }
        async fn execute(&self, args: &str, _ctx: &mut SlashCtx<'_>) -> Result<SlashOutcome> {
            Ok(SlashOutcome::StatusMessage(format!("echo: {args}")))
        }
    }

    fn echo(name: &'static str, hidden: bool) -> Arc<dyn SlashCommand> {
        Arc::new(Echo {
            meta: SlashCommandMeta {
                name,
                description: "echo for tests",
                args_hint: "",
                hidden,
                immediate: false,
            },
        })
    }

    fn echo_immediate(name: &'static str, immediate: bool) -> Arc<dyn SlashCommand> {
        Arc::new(Echo {
            meta: SlashCommandMeta {
                name,
                description: "echo for tests",
                args_hint: "",
                hidden: false,
                immediate,
            },
        })
    }

    #[test]
    fn suggester_returns_alpha_order_when_prefix_empty() {
        let mut r = SlashCommandRegistry::new();
        r.register(echo("/zebra", false));
        r.register(echo("/apple", false));
        r.register(echo("/mango", false));
        let names: Vec<&str> = r.suggest("").iter().map(|m| m.name).collect();
        assert_eq!(names, vec!["/apple", "/mango", "/zebra"]);
    }

    #[test]
    fn suggester_filters_by_substring() {
        let mut r = SlashCommandRegistry::new();
        r.register(echo("/compact", false));
        r.register(echo("/config", false));
        r.register(echo("/context", false));
        r.register(echo("/quit", false));
        let names: Vec<&str> = r.suggest("co").iter().map(|m| m.name).collect();
        assert_eq!(names, vec!["/compact", "/config", "/context"]);
    }

    #[test]
    fn suggester_prefix_matches_sort_before_substring() {
        let mut r = SlashCommandRegistry::new();
        // "/config" starts with "/co"; "/recap" doesn't but contains "ca".
        r.register(echo("/config", false));
        r.register(echo("/recap", false));
        let names: Vec<&str> = r.suggest("ca").iter().map(|m| m.name).collect();
        // Both contain "ca"; only "/recap" does *not* start with "ca".
        // Same group: alphabetical.
        // "/config" contains "ca"? no — drop it.
        assert_eq!(names, vec!["/recap"]);
    }

    #[test]
    fn suggester_matches_non_contiguous_subsequence() {
        let mut r = SlashCommandRegistry::new();
        r.register(echo("/compact", false));
        r.register(echo("/config", false));
        r.register(echo("/context", false));
        // "cfg" is a subsequence of "config" (c-o-n-f-i-g) only — substring
        // matching would have dropped it entirely.
        let names: Vec<&str> = r.suggest("cfg").iter().map(|m| m.name).collect();
        assert_eq!(names, vec!["/config"]);
    }

    #[test]
    fn suggester_is_case_insensitive() {
        let mut r = SlashCommandRegistry::new();
        r.register(echo("/compact", false));
        let names: Vec<&str> = r.suggest("CMP").iter().map(|m| m.name).collect();
        assert_eq!(names, vec!["/compact"]);
    }

    #[test]
    fn suggester_ranks_contiguous_prefix_above_scattered() {
        let mut r = SlashCommandRegistry::new();
        r.register(echo("/cost", false)); // "co" contiguous at start
        r.register(echo("/doctor", false)); // "co" scattered (d-o-...-c→o)
        let names: Vec<&str> = r.suggest("co").iter().map(|m| m.name).collect();
        assert_eq!(names, vec!["/cost", "/doctor"]);
    }

    #[test]
    fn suggester_drops_non_subsequence() {
        let mut r = SlashCommandRegistry::new();
        r.register(echo("/compact", false));
        assert!(r.suggest("xyz").is_empty());
    }

    #[test]
    fn fuzzy_score_rejects_non_subsequence() {
        assert!(fuzzy_score("config", "cfgx").is_none());
    }

    #[test]
    fn fuzzy_score_prefers_contiguous_over_gapped() {
        let contiguous = fuzzy_score("config", "co").expect("subsequence");
        let gapped = fuzzy_score("doctor", "co").expect("subsequence");
        assert!(contiguous > gapped, "{contiguous} !> {gapped}");
    }

    #[test]
    fn fuzzy_score_empty_needle_is_neutral() {
        assert_eq!(fuzzy_score("anything", ""), Some(0));
    }

    #[test]
    fn suggester_hides_hidden_commands() {
        let mut r = SlashCommandRegistry::new();
        r.register(echo("/visible", false));
        r.register(echo("/voice", true));
        let names: Vec<&str> = r.suggest("").iter().map(|m| m.name).collect();
        assert_eq!(names, vec!["/visible"]);
    }

    #[test]
    fn parse_kv_args_handles_flags_and_pairs() {
        let kv = parse_kv_args("--force --target=path --name=\"hello\"");
        assert_eq!(kv.get("force"), Some(&"true".to_string()));
        assert_eq!(kv.get("target"), Some(&"path".to_string()));
        // Surrounding quotes are stripped; whitespace-quoted values are
        // not supported (we tokenize on whitespace first).
        assert_eq!(kv.get("name"), Some(&"hello".to_string()));
    }

    #[test]
    fn registry_contains_and_len_track_registrations() {
        let mut r = SlashCommandRegistry::new();
        assert_eq!(r.len(), 0);
        assert!(!r.contains("/echo"));
        r.register(echo("/echo", false));
        assert_eq!(r.len(), 1);
        assert!(r.contains("/echo"));
    }

    #[test]
    fn visible_metas_excludes_hidden() {
        let mut r = SlashCommandRegistry::new();
        r.register(echo("/aaa", false));
        r.register(echo("/bbb", true));
        let names: Vec<&str> = r.visible_metas().iter().map(|m| m.name).collect();
        assert_eq!(names, vec!["/aaa"]);
    }

    /// IE1 Task 3 (RED): builtin registry tags non-model-touching
    /// commands as `immediate: true` so the submit handler dispatches
    /// them during inference; agent-loop-touching commands stay
    /// `immediate: false`. See caliban-ai/caliban#13 (immediate slash commands).
    #[test]
    fn known_immediate_commands_are_tagged_in_builtin_registry() {
        let r = register_builtin();
        let immediate = [
            // Original 13 (read-only diagnostics, overlays, runtime config).
            "/usage",
            "/context",
            "/cost",
            "/help",
            "/permissions",
            "/config",
            "/model",
            "/effort",
            "/think",
            "/export",
            "/doctor",
            "/quit",
            "/exit",
            "/system",
            // Flipped in caliban-ai/caliban#13: each `execute` returns only
            // Continue/Overlay/StatusMessage and never touches the model or
            // the in-flight conversation, so it is safe to fire mid-turn.
            "/heapdump",
            "/feedback",
            "/statusline",
            "/tui",
            "/voice",
            "/hooks",
            "/mcp",
            "/plugins",
            "/plugin",
            "/agents",
            "/skills",
            "/memory",
            "/output-style",
            "/status",
            "/login",
            "/logout",
            "/setup-token",
            "/init",
            "/resume",
        ];
        for cmd in &immediate {
            let m = r
                .lookup_meta(cmd)
                .unwrap_or_else(|| panic!("missing {cmd} from registry"));
            assert!(m.immediate, "expected {cmd} to be immediate");
        }
        // Sanity: ones that touch the model or the in-flight conversation
        // stay non-immediate.
        // - /clear, /compact, /rewind: mutate conversation history.
        // - /plan: toggles mid-turn mutating-tool gating.
        // - /recap, /btw: call the model/provider.
        // - /loop: drives the agent loop (re-runs assistant turns).
        let not_immediate = [
            "/clear", "/compact", "/rewind", "/plan", "/recap", "/btw", "/loop",
        ];
        for cmd in &not_immediate {
            let m = r
                .lookup_meta(cmd)
                .unwrap_or_else(|| panic!("missing {cmd} from registry"));
            assert!(!m.immediate, "{cmd} should NOT be immediate");
        }
    }

    /// IE1 Task 2 (RED): registry exposes `lookup_meta(name)` so the
    /// classifier can read the `immediate` flag without going through
    /// the full dispatch path.
    #[test]
    fn lookup_meta_returns_some_for_known_command() {
        let mut r = SlashCommandRegistry::new();
        r.register(echo("/foo", false));
        assert!(r.lookup_meta("/foo").is_some());
        assert!(r.lookup_meta("/bar").is_none());
    }

    /// IE1 Task 2 (RED): `is_immediate_slash` classifier — pure helper
    /// the submit handler uses to decide whether to bypass the
    /// running-turn bail.
    #[test]
    fn is_immediate_slash_recognizes_tagged_command() {
        let mut r = SlashCommandRegistry::new();
        r.register(echo_immediate("/inst", true));
        r.register(echo_immediate("/slow", false));
        assert!(is_immediate_slash("/inst", &r));
        assert!(is_immediate_slash("/inst with args", &r));
        assert!(!is_immediate_slash("/slow", &r));
        assert!(!is_immediate_slash("/unknown", &r));
        assert!(!is_immediate_slash("hello world", &r));
        assert!(!is_immediate_slash("", &r));
    }

    /// IE1 Task 1 (RED): `SlashCommandMeta` carries an `immediate` flag so
    /// the submit handler can distinguish commands that need the model
    /// from those that don't, and fire the latter during inference.
    #[test]
    fn meta_carries_immediate_flag() {
        let m = SlashCommandMeta {
            name: "/x",
            description: "",
            args_hint: "",
            hidden: false,
            immediate: true,
        };
        assert!(m.immediate);
        let m2 = SlashCommandMeta {
            name: "/y",
            description: "",
            args_hint: "",
            hidden: false,
            immediate: false,
        };
        assert!(!m2.immediate);
    }
}
