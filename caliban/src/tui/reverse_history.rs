//! Reverse-history search (`Ctrl+R`) with scope cycling (`Ctrl+S`).
//!
//! Three scopes (cycled by `Ctrl+S`):
//!
//! 1. **Session** — entries from `Input::history` only.
//! 2. **Project** — entries from
//!    `~/.caliban/projects/<sanitized-cwd>/input-history.txt`.
//! 3. **`AllProjects`** — entries from every project-history file under
//!    `~/.caliban/projects/`.
//!
//! Substring match (case-insensitive) on the query against each history entry.
//! Empty query lists all entries newest-first. `Enter` accepts the highlighted
//! match into the input buffer; `Esc` reverts.

use std::path::{Path, PathBuf};

/// Search scope; cycled by `Ctrl+S` from `Session` → `Project` → `AllProjects`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryScope {
    /// Current TUI session only.
    Session,
    /// Persisted history for the current project (cwd-derived path).
    Project,
    /// All persisted histories under `~/.caliban/projects/`.
    AllProjects,
}

impl HistoryScope {
    /// Cycle to the next scope in `Session → Project → AllProjects → Session`.
    pub(crate) fn cycle(self) -> Self {
        match self {
            Self::Session => Self::Project,
            Self::Project => Self::AllProjects,
            Self::AllProjects => Self::Session,
        }
    }

    /// Human-readable label for the status footer.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Project => "project",
            Self::AllProjects => "all-projects",
        }
    }
}

/// State held by the input while the reverse-history overlay is open.
#[derive(Debug)]
pub(crate) struct ReverseHistoryState {
    /// Current substring filter typed by the user.
    pub(crate) query: String,
    /// Active scope (session by default, cycled by `Ctrl+S`).
    pub(crate) scope: HistoryScope,
    /// Index into `matches()` of the currently highlighted entry.
    pub(crate) cursor: usize,
    /// Cached session history at construction time.
    session_history: Vec<String>,
    /// Optional path to the project history file.
    project_path: Option<PathBuf>,
    /// Optional root for the all-projects scan.
    all_root: Option<PathBuf>,
    /// Lazily loaded project history (computed once per state instance).
    project_cache: Option<Vec<String>>,
    /// Lazily loaded all-projects history.
    all_cache: Option<Vec<String>>,
}

impl ReverseHistoryState {
    /// Construct a fresh reverse-history search at session scope.
    pub(crate) fn new(
        session_history: Vec<String>,
        project_path: Option<PathBuf>,
        all_root: Option<PathBuf>,
    ) -> Self {
        Self {
            query: String::new(),
            scope: HistoryScope::Session,
            cursor: 0,
            session_history,
            project_path,
            all_root,
            project_cache: None,
            all_cache: None,
        }
    }

    /// Append a character to the filter and reset the cursor.
    pub(crate) fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.cursor = 0;
    }

    /// Remove the trailing character from the filter; reset cursor.
    pub(crate) fn pop_char(&mut self) {
        self.query.pop();
        self.cursor = 0;
    }

    /// Cycle to the next scope. Lazy-loads the wider scope's history.
    pub(crate) fn cycle_scope(&mut self) {
        self.scope = self.scope.cycle();
        self.cursor = 0;
        // Lazily memoize the wider scopes when first visited.
        if self.scope == HistoryScope::Project && self.project_cache.is_none() {
            self.project_cache = Some(
                self.project_path
                    .as_deref()
                    .map(load_history_file)
                    .unwrap_or_default(),
            );
        }
        if self.scope == HistoryScope::AllProjects && self.all_cache.is_none() {
            self.all_cache = Some(
                self.all_root
                    .as_deref()
                    .map(load_all_projects)
                    .unwrap_or_default(),
            );
        }
    }

    /// Move highlight up (toward newer matches).
    pub(crate) fn cursor_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    /// Move highlight down (toward older matches).
    pub(crate) fn cursor_down(&mut self) {
        let total = self.matches().len();
        if total > 0 && self.cursor + 1 < total {
            self.cursor += 1;
        }
    }

    /// Borrow the active history list for the current scope. Returns an owned
    /// `Vec<&str>` so callers don't need to know which underlying buffer
    /// supplied the entries.
    fn active_history(&self) -> Vec<&str> {
        match self.scope {
            HistoryScope::Session => self.session_history.iter().map(String::as_str).collect(),
            HistoryScope::Project => self
                .project_cache
                .as_ref()
                .map(|v| v.iter().map(String::as_str).collect())
                .unwrap_or_default(),
            HistoryScope::AllProjects => self
                .all_cache
                .as_ref()
                .map(|v| v.iter().map(String::as_str).collect())
                .unwrap_or_default(),
        }
    }

    /// Compute the (newest-first) list of entries matching `query`. Empty
    /// query yields every entry in the active scope.
    pub(crate) fn matches(&self) -> Vec<String> {
        let active = self.active_history();
        let q = self.query.to_lowercase();
        let mut out: Vec<String> = active
            .iter()
            .rev() // newest first
            .filter(|line| q.is_empty() || line.to_lowercase().contains(&q))
            .map(|s| (*s).to_string())
            .collect();
        // Drop adjacent duplicates so the user doesn't see the same prompt
        // multiple times in a row.
        out.dedup();
        out
    }

    /// Return the currently selected entry, if any.
    pub(crate) fn selected(&self) -> Option<String> {
        self.matches().into_iter().nth(self.cursor)
    }
}

/// Sanitize a path for use as a directory name under `~/.caliban/projects/`.
/// Replaces path separators and other non-portable characters with `-`.
#[must_use]
pub(crate) fn sanitize_cwd(p: &Path) -> String {
    let s = p.display().to_string();
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '.' || c == '_' {
            out.push(c);
        } else {
            out.push('-');
        }
    }
    // Trim leading/trailing dashes for tidiness.
    out.trim_matches('-').to_string()
}

/// Resolve the per-project history file path under `~/.caliban/projects/`.
/// Returns `None` if `dirs::home_dir()` fails.
#[must_use]
pub(crate) fn project_history_path(cwd: &Path) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    Some(
        home.join(".caliban")
            .join("projects")
            .join(sanitize_cwd(cwd))
            .join("input-history.txt"),
    )
}

/// Resolve the root of the all-projects directory.
#[must_use]
pub(crate) fn projects_root() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    Some(home.join(".caliban").join("projects"))
}

/// Read history file as newline-separated entries. Missing file → empty vec.
pub(crate) fn load_history_file(path: &Path) -> Vec<String> {
    let Ok(body) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    body.lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Append a single entry (one line) to the project history file. Creates
/// parent directories if missing. Silent on IO error — history is best
/// effort.
pub(crate) fn append_history(path: &Path, entry: &str) {
    use std::io::Write;
    if entry.is_empty() {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Open for append so concurrent sessions don't clobber each other.
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    let _ = writeln!(f, "{entry}");
}

/// Scan every `input-history.txt` under `root` and concatenate the entries
/// (oldest project first; within a project, file order). Caps at 5000
/// entries total to keep the search responsive.
pub(crate) fn load_all_projects(root: &Path) -> Vec<String> {
    const CAP: usize = 5000;
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.is_dir() { Some(p) } else { None }
        })
        .collect();
    dirs.sort();
    for d in dirs {
        let f = d.join("input-history.txt");
        if out.len() >= CAP {
            break;
        }
        out.extend(load_history_file(&f));
    }
    out.truncate(CAP);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn cycle_visits_three_scopes() {
        let s = HistoryScope::Session;
        let s = s.cycle();
        assert_eq!(s, HistoryScope::Project);
        let s = s.cycle();
        assert_eq!(s, HistoryScope::AllProjects);
        let s = s.cycle();
        assert_eq!(s, HistoryScope::Session);
    }

    #[test]
    fn sanitize_replaces_separators() {
        let p = Path::new("/Users/jf/dev/personal/caliban");
        let s = sanitize_cwd(p);
        // No slashes; only alphanumerics + `.` + `_` + `-`.
        for c in s.chars() {
            assert!(c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-');
        }
        assert!(!s.starts_with('-'));
        assert!(!s.ends_with('-'));
    }

    #[test]
    fn session_scope_filters_by_substring() {
        let h = vec!["git status".into(), "git push".into(), "echo hello".into()];
        let mut s = ReverseHistoryState::new(h, None, None);
        s.push_char('g');
        s.push_char('i');
        s.push_char('t');
        let m = s.matches();
        assert_eq!(m.len(), 2);
        // Newest first.
        assert_eq!(m[0], "git push");
    }

    #[test]
    fn cycle_scope_advances() {
        let mut s = ReverseHistoryState::new(vec![], None, None);
        assert_eq!(s.scope, HistoryScope::Session);
        s.cycle_scope();
        assert_eq!(s.scope, HistoryScope::Project);
        s.cycle_scope();
        assert_eq!(s.scope, HistoryScope::AllProjects);
    }

    #[test]
    fn append_then_load_roundtrip() {
        let td = TempDir::new().unwrap();
        let p = td.path().join("input-history.txt");
        append_history(&p, "first");
        append_history(&p, "second");
        let loaded = load_history_file(&p);
        assert_eq!(loaded, vec!["first".to_string(), "second".to_string()]);
    }

    #[test]
    fn project_history_path_includes_sanitized_cwd() {
        let cwd = Path::new("/tmp/some project");
        if let Some(path) = project_history_path(cwd) {
            assert!(path.display().to_string().contains(".caliban/projects/"));
            assert!(
                path.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n == "input-history.txt")
            );
        }
    }

    #[test]
    fn all_projects_walks_every_dir() {
        let td = TempDir::new().unwrap();
        let a = td.path().join("proj-a");
        let b = td.path().join("proj-b");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        append_history(&a.join("input-history.txt"), "cmd-a");
        append_history(&b.join("input-history.txt"), "cmd-b");
        let all = load_all_projects(td.path());
        assert!(all.contains(&"cmd-a".to_string()));
        assert!(all.contains(&"cmd-b".to_string()));
    }

    #[test]
    fn cursor_moves_within_match_bounds() {
        let h = vec!["a".into(), "b".into(), "c".into()];
        let mut s = ReverseHistoryState::new(h, None, None);
        s.cursor_down();
        s.cursor_down();
        s.cursor_down(); // should clamp at last
        assert_eq!(s.cursor, 2);
        s.cursor_up();
        assert_eq!(s.cursor, 1);
    }

    #[test]
    fn selected_returns_matching_entry() {
        let h = vec!["alpha".into(), "beta".into(), "gamma".into()];
        let mut s = ReverseHistoryState::new(h, None, None);
        // Newest first → cursor 0 selects "gamma".
        assert_eq!(s.selected().as_deref(), Some("gamma"));
        s.cursor_down();
        assert_eq!(s.selected().as_deref(), Some("beta"));
    }

    #[test]
    fn case_insensitive_substring_match() {
        let h = vec!["GIT push".into(), "echo".into()];
        let mut s = ReverseHistoryState::new(h, None, None);
        s.push_char('g');
        s.push_char('i');
        s.push_char('t');
        let m = s.matches();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0], "GIT push");
    }
}
