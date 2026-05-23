//! Fuzzy match candidates for slash and @-path menus.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32String};

/// One menu candidate.
#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    /// What the user sees in the menu (e.g. `"/help"` or `"main.rs"`).
    pub(crate) display: String,
    /// What replaces the trigger token when the user picks this candidate.
    /// Must include any leading trigger character (`/` or `@`).
    pub(crate) insert: String,
    /// Higher = better match. Zero for empty-query (un-ranked) listings.
    pub(crate) score: u32,
}

/// Rank `items` (display, insert) pairs against `query` using nucleo.
/// Empty query yields all `items` in original order with score 0.
pub(crate) fn rank(items: &[(&str, &str)], query: &str, limit: usize) -> Vec<Candidate> {
    if items.is_empty() {
        return Vec::new();
    }
    if query.is_empty() {
        return items
            .iter()
            .take(limit)
            .map(|(d, i)| Candidate {
                display: (*d).to_string(),
                insert: (*i).to_string(),
                score: 0,
            })
            .collect();
    }
    let mut matcher = Matcher::new(Config::DEFAULT);
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut scored: Vec<Candidate> = items
        .iter()
        .filter_map(|(d, i)| {
            let haystack = Utf32String::from(*d);
            pattern
                .score(haystack.slice(..), &mut matcher)
                .map(|s| Candidate {
                    display: (*d).to_string(),
                    insert: (*i).to_string(),
                    score: s,
                })
        })
        .collect();
    scored.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.display.cmp(&b.display)));
    scored.truncate(limit);
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_returns_all_in_order() {
        let items = &[("/help", "/help"), ("/quit", "/quit")];
        let out = rank(items, "", 10);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].display, "/help");
    }

    #[test]
    fn ranks_prefix_matches_highest() {
        let items = &[
            ("/help", "/help"),
            ("/clear", "/clear"),
            ("/config", "/config"),
        ];
        let out = rank(items, "he", 10);
        assert_eq!(out[0].display, "/help");
    }

    #[test]
    fn nonmatch_excluded() {
        let items = &[("/help", "/help")];
        let out = rank(items, "zzz", 10);
        assert!(out.is_empty());
    }

    #[test]
    fn limit_respected() {
        let items = &[("a", "a"), ("ab", "ab"), ("abc", "abc")];
        let out = rank(items, "a", 2);
        assert_eq!(out.len(), 2);
    }
}
