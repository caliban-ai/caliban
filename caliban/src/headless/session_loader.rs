//! Helpers for `--continue` / `--resume <id>` headless flags.
//!
//! These wrap [`SessionStore`] with the small bits of logic needed to pick
//! the most-recently-modified session (`--continue`) or look up one by name
//! (`--resume`). They are intentionally pure (no I/O outside `SessionStore`)
//! so unit tests can run against a `TempDir`.

use caliban_sessions::{PersistedSession, SessionStore};

use crate::headless::HeadlessError;

/// Resolve which session a headless run should hydrate from.
///
/// - `continue_latest = true` → most-recently-updated session in the store.
/// - `resume = Some(id)` → load the named session; error if missing.
/// - Neither set → `Ok(None)` (ephemeral run).
///
/// # Errors
/// - I/O / parse error from [`SessionStore`].
/// - `Resume("foo")` named a session that does not exist.
pub(crate) fn resolve_session(
    store: &SessionStore,
    continue_latest: bool,
    resume: Option<&str>,
) -> Result<Option<PersistedSession>, HeadlessError> {
    if let Some(id) = resume {
        return match store
            .load(id)
            .map_err(|e| HeadlessError::SessionLoad(e.to_string()))?
        {
            Some(s) => Ok(Some(s)),
            None => Err(HeadlessError::ResumeNotFound(id.to_string())),
        };
    }
    if continue_latest {
        let list = store
            .list()
            .map_err(|e| HeadlessError::SessionLoad(e.to_string()))?;
        if let Some(meta) = list.into_iter().next() {
            return match store
                .load(&meta.name)
                .map_err(|e| HeadlessError::SessionLoad(e.to_string()))?
            {
                Some(s) => Ok(Some(s)),
                None => Ok(None),
            };
        }
        return Err(HeadlessError::NoSessionsToContinue);
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use caliban_provider::Message;
    use caliban_sessions::PersistedSession;
    use tempfile::TempDir;

    fn fresh_store() -> (TempDir, SessionStore) {
        let dir = TempDir::new().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        (dir, store)
    }

    #[test]
    fn resolve_returns_none_when_neither_flag_set() {
        let (_dir, store) = fresh_store();
        let res = resolve_session(&store, false, None).unwrap();
        assert!(res.is_none());
    }

    #[test]
    fn resume_named_loads_session() {
        let (_dir, store) = fresh_store();
        let mut s = PersistedSession::new("alpha", "mock", "m");
        s.messages.push(Message::user_text("hello"));
        store.save(&s).unwrap();

        let res = resolve_session(&store, false, Some("alpha")).unwrap();
        assert!(res.is_some());
        assert_eq!(res.unwrap().name, "alpha");
    }

    #[test]
    fn resume_missing_returns_resume_not_found() {
        let (_dir, store) = fresh_store();
        let err = resolve_session(&store, false, Some("ghost")).unwrap_err();
        assert!(matches!(err, HeadlessError::ResumeNotFound(_)));
    }

    #[test]
    fn continue_with_no_sessions_errors() {
        let (_dir, store) = fresh_store();
        let err = resolve_session(&store, true, None).unwrap_err();
        assert!(matches!(err, HeadlessError::NoSessionsToContinue));
    }

    #[test]
    fn continue_picks_most_recent_session() {
        let (_dir, store) = fresh_store();
        let mut older = PersistedSession::new("older", "mock", "m");
        older.messages.push(Message::user_text("first"));
        store.save(&older).unwrap();
        // Small gap so updated_at differs.
        std::thread::sleep(std::time::Duration::from_millis(5));
        let mut newer = PersistedSession::new("newer", "mock", "m");
        newer.messages.push(Message::user_text("second"));
        store.save(&newer).unwrap();

        let res = resolve_session(&store, true, None).unwrap().unwrap();
        assert_eq!(res.name, "newer");
    }

    #[test]
    fn resume_takes_precedence_over_continue() {
        let (_dir, store) = fresh_store();
        let mut older = PersistedSession::new("older", "mock", "m");
        older.messages.push(Message::user_text("o"));
        store.save(&older).unwrap();
        let mut newer = PersistedSession::new("newer", "mock", "m");
        newer.messages.push(Message::user_text("n"));
        store.save(&newer).unwrap();

        // continue=true but explicit resume("older") wins.
        let res = resolve_session(&store, true, Some("older"))
            .unwrap()
            .unwrap();
        assert_eq!(res.name, "older");
    }
}
