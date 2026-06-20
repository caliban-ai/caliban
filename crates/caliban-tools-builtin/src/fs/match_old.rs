//! Tiered, whitespace-tolerant matcher for `Edit` / `MultiEdit` `old_string`
//! location, with near-miss feedback on a miss.
//!
//! This module is deliberately self-contained and tool-agnostic: it operates on
//! plain `&str` and returns byte ranges + the exact replacement string to
//! splice, so both `edit.rs` and `multi_edit.rs` can share one implementation
//! (and one test suite). It is *not* wired into those tools here.
//!
//! # Tiers
//!
//! 1. **Exact** — `old` occurs verbatim as a substring of `text`. This tier
//!    always takes precedence: if any exact match exists, the fuzzy tiers are
//!    never consulted.
//! 2. **Whitespace** — line-ending normalization (CRLF/CR → LF) + per-line
//!    `trim_end()` equality, tolerating a *single uniform* leading-whitespace
//!    delta shared by every (non-blank) line of the window. When matched, `new`
//!    is reindented by that same delta before being returned.
//! 3. **Near-miss** — neither tier matched; report the closest window as a
//!    `- expected` / `+ found` diff to guide the caller.

use std::ops::Range;

/// Which tier produced a [`MatchOutcome::Located`] result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MatchTier {
    /// `old` matched verbatim as a substring.
    Exact,
    /// `old` matched after whitespace normalization / uniform reindent.
    Whitespace,
}

/// Outcome of locating `old` within `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MatchOutcome {
    /// One or more byte ranges to replace, with the (possibly reindented)
    /// replacement string to splice into each.
    Located {
        /// Byte ranges into `text` to be replaced, in ascending order.
        ranges: Vec<Range<usize>>,
        /// The exact text to splice in place of each range. For the
        /// `Exact`/trailing-whitespace cases this is verbatim `new`; for the
        /// uniform indent-shift case it is `new` reindented by the delta.
        replacement: String,
        /// Which tier matched.
        tier: MatchTier,
    },
    /// `old` matched more than once and `replace_all` was not set.
    Ambiguous {
        /// Number of matching windows.
        count: usize,
        /// 1-based `(start_line, end_line)` of each matching window.
        locations: Vec<(usize, usize)>,
    },
    /// `old` was not found by any tier. `near` carries the closest window, if
    /// the file was small enough to scan.
    NotFound {
        /// The closest near-miss window, or `None` when scanning was skipped
        /// (file too large) or `old`/`text` was empty.
        near: Option<NearMiss>,
    },
}

/// The closest non-matching window, rendered into model-facing feedback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NearMiss {
    /// 1-based line number where the closest window starts in `text`.
    start_line: usize,
    /// `old`'s lines (trailing whitespace stripped), the "expected" side.
    expected: Vec<String>,
    /// The closest window's lines (trailing whitespace stripped), "found".
    found: Vec<String>,
}

/// Maximum file size (in lines) for which the near-miss scan runs. Larger files
/// return `near: None` to bound the `O(lines × old_lines)` sweep.
const NEAR_MISS_MAX_LINES: usize = 20_000;

/// Maximum number of diff lines rendered per side in a near-miss snippet.
const NEAR_MISS_MAX_RENDER: usize = 40;

impl NearMiss {
    /// Render the near-miss as a feedback string: the closest window's starting
    /// line number followed by a per-line `- expected` / `+ found` diff.
    pub(crate) fn render(&self) -> String {
        use std::fmt::Write as _;

        let mut out = format!(
            "closest match near line {} (no exact or whitespace-tolerant match found):\n",
            self.start_line
        );
        // Render expected (`-`) then found (`+`), each capped.
        let exp_shown = self.expected.len().min(NEAR_MISS_MAX_RENDER);
        for line in &self.expected[..exp_shown] {
            out.push_str("- ");
            out.push_str(line);
            out.push('\n');
        }
        if self.expected.len() > exp_shown {
            let _ = writeln!(
                out,
                "  … ({} more expected line(s) elided)",
                self.expected.len() - exp_shown
            );
        }
        let found_shown = self.found.len().min(NEAR_MISS_MAX_RENDER);
        for line in &self.found[..found_shown] {
            out.push_str("+ ");
            out.push_str(line);
            out.push('\n');
        }
        if self.found.len() > found_shown {
            let _ = writeln!(
                out,
                "  … ({} more found line(s) elided)",
                self.found.len() - found_shown
            );
        }
        out
    }
}

/// Locate `old` within `text`, returning the byte range(s) to replace and the
/// replacement to splice.
///
/// `replace_all` mirrors the `Edit`/`MultiEdit` flag: when `false`, more than
/// one match yields [`MatchOutcome::Ambiguous`]; when `true`, all matching
/// windows are returned. The Exact tier always wins over the fuzzy tiers.
pub(crate) fn locate(text: &str, old: &str, new: &str, replace_all: bool) -> MatchOutcome {
    // Degenerate input: empty `old` never matches meaningfully.
    if old.is_empty() {
        return MatchOutcome::NotFound { near: None };
    }

    // --- Tier 1: Exact (always takes precedence) ---------------------------
    let exact: Vec<Range<usize>> = text
        .match_indices(old)
        .map(|(start, m)| start..start + m.len())
        .collect();

    if !exact.is_empty() {
        if !replace_all && exact.len() > 1 {
            let locations = exact
                .iter()
                .map(|r| byte_range_to_line_span(text, r))
                .collect();
            return MatchOutcome::Ambiguous {
                count: exact.len(),
                locations,
            };
        }
        return MatchOutcome::Located {
            ranges: exact,
            replacement: new.to_string(),
            tier: MatchTier::Exact,
        };
    }

    // --- Tier 2: Whitespace-tolerant ---------------------------------------
    if let Some(outcome) = locate_whitespace(text, old, new, replace_all) {
        return outcome;
    }

    // --- Tier 3: Near-miss feedback ----------------------------------------
    MatchOutcome::NotFound {
        near: nearest_window(text, old),
    }
}

/// Per-line view of a file: each entry is `(byte_start, byte_end_exclusive,
/// content_without_newline)`. `byte_end` points just past the line content but
/// *before* the trailing `\n` (or at end-of-text for the final line).
struct LineSpan {
    start: usize,
    end: usize,
}

/// Split `text` into line spans that map back to exact byte offsets, accounting
/// for the trailing newline that `str::lines()` strips and a possibly
/// newline-less final line.
fn line_spans(text: &str) -> Vec<LineSpan> {
    let mut spans = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            // Exclude a trailing `\r` (CRLF) from the line content so byte
            // ranges align with the visible line and not the line terminator.
            let end = if i > start && bytes[i - 1] == b'\r' {
                i - 1
            } else {
                i
            };
            spans.push(LineSpan { start, end });
            start = i + 1;
        }
        i += 1;
    }
    // Final line: only push if there is trailing content, or the text ends with
    // a newline producing a final empty line? `str::lines()` does NOT yield a
    // trailing empty line after a final `\n`, so mirror that: push the tail only
    // when `start < len`.
    if start < bytes.len() {
        spans.push(LineSpan {
            start,
            end: bytes.len(),
        });
    }
    spans
}

/// Convert a byte range into a 1-based `(start_line, end_line)` span.
fn byte_range_to_line_span(text: &str, range: &Range<usize>) -> (usize, usize) {
    let start_line = text[..range.start].bytes().filter(|&b| b == b'\n').count() + 1;
    // end is exclusive; clamp to range.end.saturating_sub(1) for the last byte
    let last = range.end.saturating_sub(1).min(text.len());
    let end_line = text[..last].bytes().filter(|&b| b == b'\n').count() + 1;
    (start_line, end_line.max(start_line))
}

/// Leading-whitespace prefix of a line (spaces/tabs).
fn leading_ws(line: &str) -> &str {
    let end = line
        .find(|c: char| c != ' ' && c != '\t')
        .unwrap_or(line.len());
    &line[..end]
}

/// Normalize line endings to `\n` only.
fn normalize_eol(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// Tier 2: whitespace-tolerant location. Returns `None` to fall through to the
/// near-miss tier (no whitespace match, or a non-uniform indent delta).
fn locate_whitespace(text: &str, old: &str, new: &str, replace_all: bool) -> Option<MatchOutcome> {
    let norm_old = normalize_eol(old);

    // `old` line views (trailing ws stripped). Use the normalized text for line
    // splitting so windows align, but map ranges back to the *original* text.
    let old_lines: Vec<&str> = norm_old.lines().collect();
    if old_lines.is_empty() {
        return None;
    }

    // Line spans over the ORIGINAL text give us exact byte ranges. We compare
    // using the original line content with EOL stripped + trim_end.
    let spans = line_spans(text);
    let win = old_lines.len();
    if spans.len() < win {
        return None;
    }

    let strip = |s: &str| -> String { normalize_eol(s).trim_end().to_string() };
    let old_trimmed: Vec<String> = old_lines.iter().map(|l| strip(l)).collect();

    let mut matches: Vec<(Range<usize>, String)> = Vec::new();

    'windows: for w in 0..=spans.len() - win {
        // The uniform indent delta for this window, determined from the first
        // non-blank line pair and required to hold for every other one.
        let mut delta: Option<IndentDelta> = None;
        for (offset, old_t) in old_trimmed.iter().enumerate() {
            let span = &spans[w + offset];
            let file_line = norm_text_line(text, span);
            let file_t = file_line.trim_end();

            if old_t.is_empty() && file_t.is_empty() {
                // Both blank — neutral for delta determination.
                continue;
            }

            let file_lead = leading_ws(file_t);
            let old_lead = leading_ws(old_t);
            let file_body = &file_t[file_lead.len()..];
            let old_body = &old_t[old_lead.len()..];

            if file_body != old_body {
                continue 'windows; // content differs — not this window
            }

            // Derive the signed prefix delta that maps `old_lead` → `file_lead`.
            let Some(this) = IndentDelta::between(old_lead, file_lead) else {
                continue 'windows; // leads not prefix-compatible → not uniform
            };
            match &delta {
                None => delta = Some(this),
                Some(d) if *d == this => {}
                Some(_) => continue 'windows, // non-uniform → fall through
            }
        }

        // Window matched. Compute the byte range over the ORIGINAL text.
        let range_start = spans[w].start;
        let range_end = spans[w + win - 1].end;
        let range = range_start..range_end;

        // Reindent `new` by the delta (no-op for an empty/all-blank delta).
        let replacement = match &delta {
            None => new.to_string(), // all-blank window: verbatim
            Some(d) => d.apply(new),
        };

        matches.push((range, replacement));
    }

    if matches.is_empty() {
        return None;
    }

    if !replace_all && matches.len() > 1 {
        let locations = matches
            .iter()
            .map(|(r, _)| byte_range_to_line_span(text, r))
            .collect();
        return Some(MatchOutcome::Ambiguous {
            count: matches.len(),
            locations,
        });
    }

    // `replacement` applies to every returned range. In the `replace_all`
    // multi-window case each window's delta is recomputed independently; if they
    // diverge we keep the first (the uniform-delta case is overwhelmingly the
    // norm, and trailing-ws/exact-shape windows reindent to the same string).
    let replacement = matches[0].1.clone();
    let ranges = matches.into_iter().map(|(r, _)| r).collect();

    Some(MatchOutcome::Located {
        ranges,
        replacement,
        tier: MatchTier::Whitespace,
    })
}

/// Extract a line's content (EOL-normalized) for a span over the original text.
fn norm_text_line(text: &str, span: &LineSpan) -> String {
    normalize_eol(&text[span.start..span.end])
}

/// A uniform leading-whitespace shift mapping `old` lines to file lines.
///
/// Derived from the matched window: every non-blank `old` line's leading
/// whitespace, transformed by this delta, equals the corresponding file line's
/// leading whitespace. [`apply`](IndentDelta::apply) replays the same shift onto
/// `new` so the replacement lands at the file's actual indentation.
#[derive(Debug, Clone, PartialEq, Eq)]
enum IndentDelta {
    /// File lines are `prefix` MORE indented than `old`: prepend `prefix`.
    Add(String),
    /// File lines are `prefix` LESS indented than `old`: strip leading `prefix`.
    Strip(String),
}

impl IndentDelta {
    /// Compute the delta mapping `old_lead` → `file_lead`, or `None` if neither
    /// is a prefix of the other (incompatible whitespace, e.g. tabs vs spaces).
    fn between(old_lead: &str, file_lead: &str) -> Option<Self> {
        if let Some(extra) = file_lead.strip_prefix(old_lead) {
            Some(IndentDelta::Add(extra.to_string()))
        } else {
            old_lead
                .strip_prefix(file_lead)
                .map(|extra| IndentDelta::Strip(extra.to_string()))
        }
    }

    /// Apply this indent shift to every non-blank line of `new`.
    fn apply(&self, new: &str) -> String {
        match self {
            IndentDelta::Add(p) if p.is_empty() => new.to_string(),
            IndentDelta::Strip(p) if p.is_empty() => new.to_string(),
            _ => {
                let mut out = String::with_capacity(new.len());
                let mut lines = new.split('\n').peekable();
                while let Some(line) = lines.next() {
                    if line.trim().is_empty() {
                        out.push_str(line);
                    } else {
                        match self {
                            IndentDelta::Add(p) => {
                                out.push_str(p);
                                out.push_str(line);
                            }
                            IndentDelta::Strip(p) => {
                                out.push_str(line.strip_prefix(p.as_str()).unwrap_or(line));
                            }
                        }
                    }
                    if lines.peek().is_some() {
                        out.push('\n');
                    }
                }
                out
            }
        }
    }
}

/// Find the file window (of `old`'s line count) most similar to `old` by
/// Levenshtein distance over the trimmed, line-joined text. Bounded for large
/// files.
fn nearest_window(text: &str, old: &str) -> Option<NearMiss> {
    let norm_old = normalize_eol(old);
    let old_lines: Vec<String> = norm_old.lines().map(|l| l.trim_end().to_string()).collect();
    if old_lines.is_empty() {
        return None;
    }

    let spans = line_spans(text);
    if spans.is_empty() || spans.len() > NEAR_MISS_MAX_LINES {
        return None;
    }

    let win = old_lines.len();
    let file_lines: Vec<String> = spans
        .iter()
        .map(|s| norm_text_line(text, s).trim_end().to_string())
        .collect();

    // Guard: if `old` has more lines than the file, no window can fit.
    // Without this, `saturating_sub` clamps `upper` to 0 but the loop body
    // still executes once (for w=0) and slices file_lines[0..win] out-of-bounds.
    if win > file_lines.len() {
        return None;
    }

    let old_joined = old_lines.join("\n");
    let old_key = old_joined.trim().to_string();

    let upper = file_lines.len().saturating_sub(win);
    let mut best: Option<(usize, usize)> = None; // (distance, window_start)
    for w in 0..=upper {
        let window = &file_lines[w..w + win];
        let joined = window.join("\n");
        let dist = levenshtein(joined.trim(), &old_key);
        match best {
            Some((bd, _)) if dist >= bd => {}
            _ => best = Some((dist, w)),
        }
    }

    let (_, w) = best?;
    let found = file_lines[w..w + win].to_vec();
    Some(NearMiss {
        start_line: w + 1,
        expected: old_lines,
        found,
    })
}

/// Classic two-row dynamic-programming Levenshtein edit distance over chars.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur: Vec<usize> = vec![0; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    // Contract 1: Exact unique → Located/Exact, verbatim replacement.
    #[test]
    fn exact_unique() {
        let text = "hello foo world";
        let out = locate(text, "foo", "bar", false);
        match out {
            MatchOutcome::Located {
                ranges,
                replacement,
                tier,
            } => {
                assert_eq!(tier, MatchTier::Exact);
                assert_eq!(ranges, vec![6..9]);
                assert_eq!(replacement, "bar");
                assert_eq!(&text[ranges[0].clone()], "foo");
            }
            other => panic!("expected Located, got {other:?}"),
        }
    }

    // Contract 2: Exact multiple + replace_all=false → Ambiguous.
    #[test]
    fn exact_multiple_no_replace_all_ambiguous() {
        let out = locate("foo and foo", "foo", "bar", false);
        match out {
            MatchOutcome::Ambiguous { count, locations } => {
                assert_eq!(count, 2);
                assert_eq!(locations.len(), 2);
                assert_eq!(locations[0].0, 1); // both on line 1
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    // Contract 3: Exact multiple + replace_all=true → all ranges.
    #[test]
    fn exact_multiple_replace_all() {
        let out = locate("foo and foo", "foo", "bar", true);
        match out {
            MatchOutcome::Located { ranges, tier, .. } => {
                assert_eq!(tier, MatchTier::Exact);
                assert_eq!(ranges, vec![0..3, 8..11]);
            }
            other => panic!("expected Located, got {other:?}"),
        }
    }

    // Contract 4: old has trailing whitespace the file lacks → Tier-2 unique.
    #[test]
    fn trailing_ws_tolerated() {
        let text = "let x = 1;\nlet y = 2;\n";
        let old = "let x = 1;   \nlet y = 2;"; // trailing ws on line 1
        let out = locate(text, old, "let x = 9;\nlet y = 8;", false);
        match out {
            MatchOutcome::Located {
                ranges,
                replacement,
                tier,
            } => {
                assert_eq!(tier, MatchTier::Whitespace);
                assert_eq!(ranges.len(), 1);
                // Range should cover both lines (without the final newline).
                assert_eq!(&text[ranges[0].clone()], "let x = 1;\nlet y = 2;");
                assert_eq!(replacement, "let x = 9;\nlet y = 8;");
            }
            other => panic!("expected Located/Whitespace, got {other:?}"),
        }
    }

    // Contract 5: CRLF file vs LF old → matches.
    #[test]
    fn crlf_file_lf_old() {
        let text = "alpha\r\nbeta\r\ngamma\r\n";
        let old = "alpha\nbeta";
        let out = locate(text, old, "ALPHA\nBETA", false);
        match out {
            MatchOutcome::Located { ranges, tier, .. } => {
                assert_eq!(tier, MatchTier::Whitespace);
                assert_eq!(ranges.len(), 1);
                // The original-text range maps to "alpha\r\nbeta".
                assert_eq!(&text[ranges[0].clone()], "alpha\r\nbeta");
            }
            other => panic!("expected Located/Whitespace, got {other:?}"),
        }
    }

    // Contract 6: old uniformly under-indented by 4 → matches; new reindented +4.
    #[test]
    fn uniform_indent_shift_reindents_new() {
        // File has 4-space indent; old is written with NO indent.
        let text = "    if x {\n        y();\n    }\n";
        let old = "if x {\n    y();\n}";
        let new = "if x {\n    z();\n}";
        let out = locate(text, old, new, false);
        match out {
            MatchOutcome::Located {
                ranges,
                replacement,
                tier,
            } => {
                assert_eq!(tier, MatchTier::Whitespace);
                assert_eq!(ranges.len(), 1);
                assert_eq!(&text[ranges[0].clone()], "    if x {\n        y();\n    }");
                // Reindent assertion: every non-blank line of `new` gains +4.
                assert_eq!(replacement, "    if x {\n        z();\n    }");
                // Splice and check resulting indentation.
                let mut spliced = text.to_string();
                spliced.replace_range(ranges[0].clone(), &replacement);
                assert_eq!(spliced, "    if x {\n        z();\n    }\n");
                for line in replacement.lines().filter(|l| !l.trim().is_empty()) {
                    assert!(line.starts_with("    "), "line not reindented: {line:?}");
                }
            }
            other => panic!("expected Located/Whitespace, got {other:?}"),
        }
    }

    // Contract 7: No match → NotFound with rendered near-miss diff.
    #[test]
    fn not_found_renders_near_miss() {
        let text = "fn alpha() {\n    do_thing();\n}\n";
        let old = "fn alpha() {\n    do_OTHER();\n}";
        let out = locate(text, old, "x", false);
        match out {
            MatchOutcome::NotFound { near: Some(nm) } => {
                let rendered = nm.render();
                assert!(
                    !rendered.contains("not found in file"),
                    "should not be bare not-found: {rendered}"
                );
                assert!(rendered.contains("line 1"), "rendered: {rendered}");
                assert!(rendered.contains("- "), "expected `-` diff: {rendered}");
                assert!(rendered.contains("+ "), "expected `+` diff: {rendered}");
                assert!(
                    rendered.contains("do_OTHER();"),
                    "expected side missing: {rendered}"
                );
                assert!(
                    rendered.contains("do_thing();"),
                    "found side missing: {rendered}"
                );
            }
            other => panic!("expected NotFound/Some(near), got {other:?}"),
        }
    }

    // Contract 8: Tier-2 ambiguity (two normalized windows, no replace_all).
    #[test]
    fn tier2_ambiguity() {
        // Two indented copies of the same block; old is unindented.
        let text = "  a()\n  b()\nMID\n  a()\n  b()\n";
        let old = "a()\nb()";
        let out = locate(text, old, "x()\ny()", false);
        match out {
            MatchOutcome::Ambiguous { count, locations } => {
                assert_eq!(count, 2);
                assert_eq!(locations.len(), 2);
                assert_eq!(locations[0], (1, 2));
                assert_eq!(locations[1], (4, 5));
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    // Contract 8b: Tier-2 ambiguity resolved by replace_all.
    #[test]
    fn tier2_ambiguity_replace_all() {
        let text = "  a()\n  b()\nMID\n  a()\n  b()\n";
        let old = "a()\nb()";
        let out = locate(text, old, "x()\ny()", true);
        match out {
            MatchOutcome::Located { ranges, tier, .. } => {
                assert_eq!(tier, MatchTier::Whitespace);
                assert_eq!(ranges.len(), 2);
            }
            other => panic!("expected Located, got {other:?}"),
        }
    }

    // Contract 9: Exact match exists AND a fuzzy window also exists → exact wins.
    #[test]
    fn exact_wins_unique() {
        // "UNIQ()" appears verbatim once (line 1) and the file also contains an
        // indented near-variant the whitespace tier *could* match if consulted.
        let text = "UNIQ()\nMID\n    UNIQX()\n";
        let old = "UNIQ()";
        let out = locate(text, old, "DONE()", false);
        match out {
            MatchOutcome::Located {
                tier, replacement, ..
            } => {
                assert_eq!(tier, MatchTier::Exact);
                assert_eq!(replacement, "DONE()");
            }
            other => panic!("expected Located/Exact, got {other:?}"),
        }
    }

    // Non-uniform indent delta → NOT matched in tier 2; falls to near-miss.
    #[test]
    fn non_uniform_indent_falls_through() {
        // line 1 shifted +2, line 2 shifted +4 → non-uniform → no tier-2 match.
        let text = "  a()\n    b()\n";
        let old = "a()\nb()";
        let out = locate(text, old, "x", false);
        assert!(
            matches!(out, MatchOutcome::NotFound { .. }),
            "non-uniform indent must not match tier 2: {out:?}"
        );
    }

    #[test]
    fn levenshtein_basics() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("abc", "abc"), 0);
        assert_eq!(levenshtein("abc", ""), 3);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("flaw", "lawn"), 2);
    }

    #[test]
    fn empty_old_is_not_found() {
        assert!(matches!(
            locate("anything", "", "x", false),
            MatchOutcome::NotFound { near: None }
        ));
    }

    #[test]
    fn final_line_without_newline_maps_correctly() {
        let text = "line1\nline2"; // no trailing newline
        let out = locate(text, "line2", "LINE2", false);
        match out {
            MatchOutcome::Located { ranges, tier, .. } => {
                assert_eq!(tier, MatchTier::Exact);
                assert_eq!(&text[ranges[0].clone()], "line2");
            }
            other => panic!("got {other:?}"),
        }
    }

    // Regression: nearest_window must not panic when old has more lines than the
    // file. Before the guard, `saturating_sub` set upper=0 but the loop still
    // ran once and sliced file_lines[0..win] out-of-bounds.
    //
    // The bug report example locate("x", "aaaa\nbbbb\ncccc", "y", false) has
    // the arguments in wrong order for our signature (text, old, new). The actual
    // panic is triggered when old has MORE lines than the file:
    // old = "aaaa\nbbbb\ncccc" (3 lines), file = "x" (1 line).
    #[test]
    fn not_found_old_longer_than_file_no_panic() {
        // old has 3 lines; file has 1 line — window cannot fit, must not panic.
        let out = locate("x", "aaaa\nbbbb\ncccc", "y", false);
        assert!(
            matches!(out, MatchOutcome::NotFound { near: None }),
            "expected NotFound{{near: None}}, got {out:?}"
        );
    }

    // Regression: single-line old longer than a 1-line file must not panic.
    #[test]
    fn not_found_old_two_lines_single_line_file_no_panic() {
        // old has 2 lines; file has only 1 — window cannot fit.
        let out = locate(
            "only_one_line",
            "missing_a\nmissing_b",
            "replacement",
            false,
        );
        assert!(
            matches!(out, MatchOutcome::NotFound { near: None }),
            "expected NotFound{{near: None}}, got {out:?}"
        );
    }

    #[test]
    fn whitespace_tier_final_line_no_newline() {
        // Whitespace match where the matched window ends at the last
        // (newline-less) line — ensure the byte range stops at end-of-text.
        // `old` is unindented so the Exact tier cannot match the indented file.
        let text = "alpha\n    beta\n    gamma"; // no trailing \n, +4 indent
        let old = "beta\ngamma";
        let out = locate(text, old, "BETA\nGAMMA", false);
        match out {
            MatchOutcome::Located {
                ranges,
                tier,
                replacement,
            } => {
                assert_eq!(tier, MatchTier::Whitespace);
                assert_eq!(&text[ranges[0].clone()], "    beta\n    gamma");
                // Reindented +4 to match the file.
                assert_eq!(replacement, "    BETA\n    GAMMA");
            }
            other => panic!("got {other:?}"),
        }
    }
}
