//! Tests for `McpActivationSet` (ADR-0046).

use caliban_agent_core::mcp_activation::McpActivationSet;

#[test]
fn activate_idempotent_bumps_lru() {
    let mut s = McpActivationSet::new(8);
    assert!(s.activate("mcp__a__one").is_none());
    assert!(s.activate("mcp__a__two").is_none());
    let evicted = s.activate("mcp__a__one");
    assert!(evicted.is_none(), "no eviction at len < cap");
    let order: Vec<&str> = s.iter_active().collect();
    assert_eq!(
        order,
        vec!["mcp__a__one", "mcp__a__two"],
        "re-activate moves the existing entry to MRU"
    );
}

#[test]
fn evicts_oldest_at_cap() {
    let mut s = McpActivationSet::new(2);
    assert!(s.activate("a").is_none());
    assert!(s.activate("b").is_none());
    let evicted = s.activate("c");
    assert_eq!(evicted, Some("a".to_string()));
    assert!(!s.is_active("a"));
    assert!(s.is_active("b"));
    assert!(s.is_active("c"));
}

#[test]
fn snapshot_independent_after_mutate() {
    let mut s = McpActivationSet::new(4);
    s.activate("a");
    let snap = s.snapshot();
    s.activate("b");
    assert!(s.is_active("b"));
    assert!(!snap.is_active("b"), "snapshot is decoupled");
    assert!(snap.is_active("a"));
}

#[test]
fn iter_active_returns_mru_first() {
    let mut s = McpActivationSet::new(4);
    s.activate("a");
    s.activate("b");
    s.activate("c");
    let order: Vec<&str> = s.iter_active().collect();
    assert_eq!(order, vec!["c", "b", "a"], "front of LRU is MRU");
}

#[test]
fn cap_zero_disables_activation() {
    let mut s = McpActivationSet::new(0);
    let evicted = s.activate("a");
    assert_eq!(evicted, None, "cap=0 returns no eviction");
    assert!(!s.is_active("a"), "cap=0 stores nothing");
    assert_eq!(s.len(), 0);
}

#[test]
fn len_and_is_empty_track_state() {
    let mut s = McpActivationSet::new(4);
    assert_eq!(s.len(), 0);
    assert!(s.is_empty());
    s.activate("a");
    assert_eq!(s.len(), 1);
    assert!(!s.is_empty());
}
