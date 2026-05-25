//! Filesystem watcher around the loaded scope paths.
//!
//! Wraps `notify::RecommendedWatcher`. The caller subscribes to the
//! [`SettingsWatcher::events`] receiver and re-runs `load_settings` on
//! each notification; the watcher does not own the merged `Settings`
//! itself (that lives in [`crate::SettingsHandle`]).
//!
//! Events are coalesced naturally by the `tokio::sync::mpsc::Receiver`
//! end of the channel — the caller is responsible for picking the
//! latest event before reloading.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc;

const DEBOUNCE: Duration = Duration::from_millis(250);

/// One filesystem event the watcher emitted.
#[derive(Debug, Clone)]
pub struct WatcherEvent {
    /// Path that triggered the event.
    pub path: PathBuf,
    /// `notify`'s event kind.
    pub kind: EventKind,
}

/// Holds the watcher handle (drop = unsubscribe) and exposes a tokio
/// mpsc receiver of events.
pub struct SettingsWatcher {
    _watcher: notify::RecommendedWatcher,
    rx: mpsc::UnboundedReceiver<WatcherEvent>,
}

impl SettingsWatcher {
    /// Spawn a watcher over the given paths plus their parent
    /// directories (so file-create events fire too).
    ///
    /// # Errors
    /// Forwards `notify` setup errors.
    pub fn watch(paths: &[PathBuf]) -> notify::Result<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        // Track the last-emitted instant per path for in-watcher
        // debouncing (otherwise notify sometimes fires twice for one
        // logical write).
        let mut last: std::collections::HashMap<PathBuf, Instant> =
            std::collections::HashMap::new();
        let tx_clone = tx.clone();
        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if let Ok(event) = res {
                    for p in &event.paths {
                        let now = Instant::now();
                        let send = last
                            .get(p)
                            .is_none_or(|prev| now.duration_since(*prev) > DEBOUNCE);
                        if send {
                            last.insert(p.clone(), now);
                            let _ = tx_clone.send(WatcherEvent {
                                path: p.clone(),
                                kind: event.kind,
                            });
                        }
                    }
                }
            })?;
        // Watch each path's *parent* (so create events fire) plus the
        // file itself (so we catch atomic renames where the parent dir
        // doesn't see the write).
        for p in paths {
            if let Some(parent) = p.parent() {
                let _ = watcher.watch(parent, RecursiveMode::NonRecursive);
            }
            if p.exists() {
                let _ = watcher.watch(p, RecursiveMode::NonRecursive);
            }
        }
        let _ = tx; // explicit clone-and-drop pattern
        Ok(Self {
            _watcher: watcher,
            rx,
        })
    }

    /// Receive the next event. Returns `None` when the watcher was
    /// dropped.
    pub async fn next(&mut self) -> Option<WatcherEvent> {
        self.rx.recv().await
    }

    /// Try to receive without awaiting. Returns `Err` if no event is
    /// ready (or the watcher is closed).
    pub fn try_next(&mut self) -> Result<WatcherEvent, mpsc::error::TryRecvError> {
        self.rx.try_recv()
    }
}

impl std::fmt::Debug for SettingsWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsWatcher").finish_non_exhaustive()
    }
}

/// Build the list of paths the watcher should track from a slice of
/// `ScopeSource` entries (output of the loader).
#[must_use]
pub fn watch_paths_from_sources(sources: &[crate::ScopeSource]) -> Vec<PathBuf> {
    sources.iter().filter_map(|s| s.path.clone()).collect()
}

/// Walk the parent dir of `p` looking for either `settings.json` or
/// `settings.toml`. Helper used by the watcher to distinguish "scope
/// file" notifications from unrelated parent-dir noise.
#[must_use]
pub fn is_settings_path(p: &Path) -> bool {
    p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
        matches!(
            n,
            "settings.json"
                | "settings.toml"
                | "settings.local.json"
                | "settings.local.toml"
                | "managed-settings.json"
                | "managed-settings.toml"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_settings_path_recognizes_canonical_filenames() {
        assert!(is_settings_path(Path::new(
            "/tmp/ws/.caliban/settings.json"
        )));
        assert!(is_settings_path(Path::new(
            "/tmp/ws/.caliban/settings.local.toml"
        )));
        assert!(!is_settings_path(Path::new("/tmp/ws/.caliban/other.json")));
    }

    #[tokio::test]
    async fn watcher_fires_on_file_create() {
        let tmp = tempfile::TempDir::new().unwrap();
        let target = tmp.path().join("settings.json");
        let mut watcher = SettingsWatcher::watch(std::slice::from_ref(&target)).unwrap();
        // Spawn the write in a separate task so the watcher has time
        // to register.
        let path = target.clone();
        tokio::spawn(async move {
            // Give notify a moment to set up.
            tokio::time::sleep(Duration::from_millis(100)).await;
            std::fs::write(&path, r#"{"model": "new"}"#).unwrap();
        });
        let event = tokio::time::timeout(Duration::from_secs(5), watcher.next())
            .await
            .expect("watcher event timeout")
            .expect("watcher closed");
        assert!(event.path.ends_with("settings.json"));
    }
}
