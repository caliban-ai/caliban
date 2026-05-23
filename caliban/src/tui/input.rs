//! Input state machine for the TUI prompt area.

use crate::tui::completer::{Candidate, rank};

/// Active mode of the input — drives both render and key dispatch.
#[derive(Debug, Default)]
pub(crate) enum InputMode {
    #[default]
    Idle,
    SlashMenu(MenuState),
    AtMenu(MenuState),
}

/// State carried by an open slash or @-path menu.
#[derive(Debug)]
pub(crate) struct MenuState {
    pub(crate) candidates: Vec<Candidate>,
    pub(crate) selected: usize,
    /// Byte offset of the trigger character (`/` or `@`) in `Input::buffer`.
    pub(crate) trigger_start: usize,
}

impl MenuState {
    pub(crate) fn new(trigger_start: usize, candidates: Vec<Candidate>) -> Self {
        Self {
            candidates,
            selected: 0,
            trigger_start,
        }
    }

    pub(crate) fn cycle_next(&mut self) {
        if self.candidates.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.candidates.len();
    }

    pub(crate) fn cycle_prev(&mut self) {
        if self.candidates.is_empty() {
            return;
        }
        self.selected = if self.selected == 0 {
            self.candidates.len() - 1
        } else {
            self.selected - 1
        };
    }
}

/// Multi-line input buffer with cursor, history navigation, and an
/// optional active menu (slash or @-path completion).
#[derive(Debug, Default)]
pub(crate) struct Input {
    pub(crate) buffer: String,
    pub(crate) cursor: usize,
    pub(crate) history: Vec<String>,
    pub(crate) history_cursor: Option<usize>,
    pub(crate) mode: InputMode,
}

impl Input {
    #[cfg(test)]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn from_history(history: Vec<String>) -> Self {
        Self {
            history,
            ..Self::default()
        }
    }

    pub(crate) fn insert_char(&mut self, c: char) {
        self.buffer.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub(crate) fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = self.buffer[..self.cursor]
            .chars()
            .next_back()
            .map_or(0, char::len_utf8);
        self.cursor -= prev;
        self.buffer.drain(self.cursor..self.cursor + prev);
    }

    pub(crate) fn delete_forward(&mut self) {
        if self.cursor >= self.buffer.len() {
            return;
        }
        let next = self.buffer[self.cursor..]
            .chars()
            .next()
            .map_or(0, char::len_utf8);
        self.buffer.drain(self.cursor..self.cursor + next);
    }

    pub(crate) fn cursor_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = self.buffer[..self.cursor]
            .chars()
            .next_back()
            .map_or(0, char::len_utf8);
        self.cursor -= prev;
    }

    pub(crate) fn cursor_right(&mut self) {
        if self.cursor >= self.buffer.len() {
            return;
        }
        let next = self.buffer[self.cursor..]
            .chars()
            .next()
            .map_or(0, char::len_utf8);
        self.cursor += next;
    }

    pub(crate) fn cursor_home(&mut self) {
        self.cursor = 0;
    }

    pub(crate) fn cursor_end(&mut self) {
        self.cursor = self.buffer.len();
    }

    pub(crate) fn insert_newline(&mut self) {
        self.buffer.insert(self.cursor, '\n');
        self.cursor += 1;
    }

    pub(crate) fn clear(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
        self.history_cursor = None;
    }

    /// Take the buffer, push it onto history, and reset to an empty prompt.
    pub(crate) fn submit(&mut self) -> String {
        let line = std::mem::take(&mut self.buffer);
        self.cursor = 0;
        self.history.push(line.clone());
        self.history_cursor = None;
        line
    }

    pub(crate) fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let new_idx = match self.history_cursor {
            None => self.history.len() - 1,
            Some(0) => 0,
            Some(i) => i - 1,
        };
        self.history_cursor = Some(new_idx);
        self.buffer = self.history[new_idx].clone();
        self.cursor = self.buffer.len();
    }

    pub(crate) fn history_down(&mut self) {
        let Some(idx) = self.history_cursor else {
            return;
        };
        if idx + 1 >= self.history.len() {
            self.history_cursor = None;
            self.buffer.clear();
            self.cursor = 0;
        } else {
            self.history_cursor = Some(idx + 1);
            self.buffer = self.history[idx + 1].clone();
            self.cursor = self.buffer.len();
        }
    }

    pub(crate) fn close_menu(&mut self) {
        self.mode = InputMode::Idle;
    }

    /// Open the slash menu if the buffer is exactly `/` at the very start
    /// (i.e. the user just typed `/` to begin a slash command).
    pub(crate) fn maybe_open_slash_menu(&mut self, all_commands: &[(&str, &str)]) {
        if matches!(self.mode, InputMode::Idle) && self.buffer == "/" && self.cursor == 1 {
            let cands = rank(all_commands, "", 32);
            self.mode = InputMode::SlashMenu(MenuState::new(0, cands));
        }
    }

    /// Refilter the slash menu against the prefix after the leading `/`.
    /// Closes the menu if the buffer no longer starts with `/`.
    pub(crate) fn refilter_slash_menu(&mut self, all_commands: &[(&str, &str)]) {
        if let InputMode::SlashMenu(ref mut menu) = self.mode {
            if self.buffer.starts_with('/') {
                // Slash command name runs from byte 1 to either the cursor
                // or the next whitespace, whichever comes first.
                let end = self
                    .buffer
                    .find(char::is_whitespace)
                    .unwrap_or(self.buffer.len());
                let prefix = &self.buffer[1..end];
                let cands = rank(all_commands, prefix, 32);
                menu.candidates = cands;
                menu.selected = 0;
                return;
            }
            self.mode = InputMode::Idle;
        }
    }

    pub(crate) fn open_at_menu(&mut self, trigger_start: usize, candidates: Vec<Candidate>) {
        self.mode = InputMode::AtMenu(MenuState::new(trigger_start, candidates));
    }

    /// Replace the active token (from `trigger_start` to the next whitespace
    /// or end-of-buffer) with the selected candidate's `insert` text.
    /// Returns `true` if the accepted candidate was a directory entry
    /// (display ends with `/`), so the caller can keep the menu open and
    /// refresh for the new directory.
    pub(crate) fn accept_menu_selection(&mut self) -> bool {
        let (start, end, insert, was_dir) = match &self.mode {
            InputMode::SlashMenu(m) | InputMode::AtMenu(m) => {
                let Some(cand) = m.candidates.get(m.selected) else {
                    self.mode = InputMode::Idle;
                    return false;
                };
                let start = m.trigger_start;
                let after_trigger = &self.buffer[start..];
                let end_offset = after_trigger
                    .find(char::is_whitespace)
                    .unwrap_or(after_trigger.len());
                (
                    start,
                    start + end_offset,
                    cand.insert.clone(),
                    cand.display.ends_with('/'),
                )
            }
            InputMode::Idle => return false,
        };
        self.buffer.replace_range(start..end, &insert);
        self.cursor = start + insert.len();
        self.mode = InputMode::Idle;
        was_dir
    }

    /// Find the active @-token surrounding the cursor, if any. The `@` must
    /// be at the start of the buffer or preceded by whitespace.
    pub(crate) fn active_at_token(&self) -> Option<(usize, String)> {
        let before = &self.buffer[..self.cursor];
        let at_pos = before.rfind('@')?;
        if at_pos > 0 {
            let prev = before[..at_pos].chars().next_back().unwrap_or(' ');
            if !prev.is_whitespace() {
                return None;
            }
        }
        let after_at = &self.buffer[at_pos + 1..];
        let end_in_after = after_at.find(char::is_whitespace).unwrap_or(after_at.len());
        let token = &self.buffer[at_pos + 1..at_pos + 1 + end_in_after];
        Some((at_pos, token.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_cursor_advance() {
        let mut i = Input::new();
        i.insert_char('a');
        i.insert_char('b');
        assert_eq!(i.buffer, "ab");
        assert_eq!(i.cursor, 2);
    }

    #[test]
    fn backspace_at_start_is_noop() {
        let mut i = Input::new();
        i.backspace();
        assert_eq!(i.buffer, "");
        assert_eq!(i.cursor, 0);
    }

    #[test]
    fn backspace_handles_multibyte() {
        let mut i = Input::new();
        i.insert_char('é');
        i.backspace();
        assert_eq!(i.buffer, "");
        assert_eq!(i.cursor, 0);
    }

    #[test]
    fn submit_pushes_history_and_clears() {
        let mut i = Input::new();
        i.insert_char('x');
        let out = i.submit();
        assert_eq!(out, "x");
        assert_eq!(i.buffer, "");
        assert_eq!(i.history, vec!["x".to_string()]);
    }

    #[test]
    fn insert_newline_inserts_lf_at_cursor() {
        let mut i = Input::new();
        i.insert_char('a');
        i.insert_char('b');
        i.cursor_left();
        i.insert_newline();
        assert_eq!(i.buffer, "a\nb");
        assert_eq!(i.cursor, 2);
    }

    #[test]
    fn slash_opens_menu_at_col_zero() {
        let mut i = Input::new();
        i.insert_char('/');
        i.maybe_open_slash_menu(&[("/help", "/help"), ("/quit", "/quit")]);
        assert!(matches!(i.mode, InputMode::SlashMenu(_)));
    }

    #[test]
    fn typing_refilters_slash_menu() {
        let cmds = &[("/help", "/help"), ("/quit", "/quit")];
        let mut i = Input::new();
        i.insert_char('/');
        i.maybe_open_slash_menu(cmds);
        i.insert_char('h');
        i.refilter_slash_menu(cmds);
        match &i.mode {
            InputMode::SlashMenu(m) => assert_eq!(m.candidates[0].display, "/help"),
            _ => panic!("expected slash menu"),
        }
    }

    #[test]
    fn accept_selection_replaces_token() {
        let cmds = &[("/help", "/help")];
        let mut i = Input::new();
        i.insert_char('/');
        i.maybe_open_slash_menu(cmds);
        i.insert_char('h');
        i.refilter_slash_menu(cmds);
        i.accept_menu_selection();
        assert_eq!(i.buffer, "/help");
        assert!(matches!(i.mode, InputMode::Idle));
    }

    #[test]
    fn detects_at_token_at_start_of_buffer() {
        let mut i = Input::new();
        i.insert_char('@');
        i.insert_char('s');
        let (start, tok) = i.active_at_token().unwrap();
        assert_eq!(start, 0);
        assert_eq!(tok, "s");
    }

    #[test]
    fn detects_at_token_after_whitespace() {
        let mut i = Input::new();
        for c in "hello @sr".chars() {
            i.insert_char(c);
        }
        let (start, tok) = i.active_at_token().unwrap();
        assert_eq!(start, 6);
        assert_eq!(tok, "sr");
    }

    #[test]
    fn ignores_at_inside_word() {
        let mut i = Input::new();
        for c in "user@host".chars() {
            i.insert_char(c);
        }
        assert!(i.active_at_token().is_none());
    }

    #[test]
    fn history_up_down_round_trip() {
        let mut i = Input::from_history(vec!["one".into(), "two".into()]);
        i.history_up();
        assert_eq!(i.buffer, "two");
        i.history_up();
        assert_eq!(i.buffer, "one");
        i.history_down();
        assert_eq!(i.buffer, "two");
        i.history_down();
        assert_eq!(i.buffer, "");
        assert_eq!(i.history_cursor, None);
    }
}
