#![allow(missing_docs)]

use caliban_provider::{Message, Usage};
use caliban_sessions::{PersistedSession, SessionStore};
use tempfile::TempDir;

fn fake_session() -> PersistedSession {
    let mut s = PersistedSession::new("test", "anthropic", "claude-3-5-sonnet");
    s.messages.push(Message::user_text("hi"));
    s.messages.push(Message::assistant_text("hi back"));
    s.total_usage = Usage {
        input_tokens: 5,
        output_tokens: 3,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    };
    s
}

#[test]
fn save_and_load_round_trip() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());
    let original = fake_session();
    store.save(&original).unwrap();
    let loaded = store.load("test").unwrap().unwrap();
    assert_eq!(loaded.name, original.name);
    assert_eq!(loaded.provider, original.provider);
    assert_eq!(loaded.messages.len(), 2);
    assert_eq!(loaded.total_usage.input_tokens, 5);
}

#[test]
fn load_missing_returns_none() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());
    let loaded = store.load("nonexistent").unwrap();
    assert!(loaded.is_none());
}

#[test]
fn list_returns_sorted_by_updated_at_desc() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());
    let mut a = PersistedSession::new("a", "anthropic", "m");
    a.updated_at = chrono::DateTime::from_timestamp(1000, 0).unwrap();
    let mut b = PersistedSession::new("b", "anthropic", "m");
    b.updated_at = chrono::DateTime::from_timestamp(2000, 0).unwrap();
    store.save(&a).unwrap();
    store.save(&b).unwrap();
    let list = store.list().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].name, "b"); // b is newer; comes first
    assert_eq!(list[1].name, "a");
}

#[test]
fn delete_removes_file() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());
    store.save(&fake_session()).unwrap();
    assert!(store.load("test").unwrap().is_some());
    store.delete("test").unwrap();
    assert!(store.load("test").unwrap().is_none());
}

#[test]
fn delete_missing_succeeds() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());
    store.delete("nonexistent").unwrap();
}

#[test]
fn invalid_name_rejected() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());
    assert!(store.load("../escape").is_err());
    assert!(store.load("name with spaces").is_err());
    assert!(store.load("").is_err());
}

#[test]
fn invalid_name_in_save_rejected() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());
    let mut bad = PersistedSession::new("bad name", "anthropic", "m");
    bad.messages.push(Message::user_text("x"));
    assert!(store.save(&bad).is_err());
}

#[test]
fn pretty_json_is_human_readable() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());
    store.save(&fake_session()).unwrap();
    // `save` is debounced; flush so the on-disk file exists for the
    // direct read below.
    store.flush();
    let path = store.path_for("test");
    let bytes = std::fs::read(&path).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(
        text.contains('\n'),
        "pretty-printed JSON should have newlines"
    );
    assert!(text.contains("\"name\""));
}
