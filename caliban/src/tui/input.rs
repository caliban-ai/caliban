//! Input state machine for the TUI prompt area.

/// Single-line input buffer with cursor and ↑/↓ history navigation.
#[derive(Debug, Default)]
pub(crate) struct Input {
    pub(crate) buffer: String,
    pub(crate) cursor: usize,
    pub(crate) history: Vec<String>,
    pub(crate) history_cursor: Option<usize>,
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
