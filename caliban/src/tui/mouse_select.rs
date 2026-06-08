//! Mouse-driven text selection for the transcript pane (IE3).
//!
//! See the TUI ergonomics design (mouse drag-select + OSC-52; shipped). This
//! module provides two orthogonal pieces:
//!
//! - [`PositionMap`] — a per-frame `(row, col) → str` mapping built by
//!   the renderer as it lays out each styled span. The mouse handler
//!   uses it to extract the text the user dragged across.
//! - [`MouseSelection`] — a small state machine driven by
//!   `MouseEventKind::Down(Left)` / `Drag(Left)` / `Up(Left)` events
//!   from crossterm. Holds the in-progress or completed selection
//!   range; orthogonal to scroll-wheel handling.
//!
//! Both are pure / unit-testable. The render + event-loop wiring lives
//! in `render.rs` and `events.rs::handle_mouse` respectively.

/// A (row, col) coordinate in terminal cells. Origin top-left.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Cell {
    pub(crate) row: u16,
    pub(crate) col: u16,
}

impl Cell {
    #[must_use]
    pub(crate) const fn new(row: u16, col: u16) -> Self {
        Self { row, col }
    }
}

/// Per-frame map from a terminal cell to the character drawn at that
/// cell. Built by the renderer as it lays out the transcript; queried
/// by the mouse handler on `Up(Left)` to extract the selected text.
///
/// Cells are addressed as `(row, col)`; missing cells (gaps, padding)
/// resolve to `None`. `extract_range` walks the rectangle / linear
/// span between two cells in reading order and concatenates the chars
/// it finds, joining cell rows with `\n`.
#[derive(Debug, Default, Clone)]
pub(crate) struct PositionMap {
    /// Dense vec indexed by row, each entry a sparse vec of (col, char).
    /// Sparse to keep the map small for partially-filled rows (statusline,
    /// borders, etc.). Rows beyond `rows.len()` are missing.
    rows: Vec<Vec<(u16, char)>>,
}

impl PositionMap {
    /// Construct an empty map.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Record a single character at `(row, col)`. Replaces any previous
    /// char at that cell. Used by the renderer as it lays out each
    /// glyph; called per cell so the map's shape matches what the user
    /// sees on screen.
    pub(crate) fn record(&mut self, row: u16, col: u16, ch: char) {
        let r = row as usize;
        if self.rows.len() <= r {
            self.rows.resize(r + 1, Vec::new());
        }
        let bucket = &mut self.rows[r];
        // Replace any existing entry for this column to keep last-write wins.
        match bucket.binary_search_by_key(&col, |(c, _)| *c) {
            Ok(idx) => bucket[idx] = (col, ch),
            Err(idx) => bucket.insert(idx, (col, ch)),
        }
    }

    /// Reset to empty without freeing capacity. Called at the start of
    /// each render frame.
    pub(crate) fn clear(&mut self) {
        for row in &mut self.rows {
            row.clear();
        }
    }

    /// Read a single cell, if recorded.
    #[must_use]
    #[allow(dead_code, reason = "convenience accessor used by unit tests")]
    pub(crate) fn get(&self, row: u16, col: u16) -> Option<char> {
        let r = row as usize;
        if r >= self.rows.len() {
            return None;
        }
        let bucket = &self.rows[r];
        bucket
            .binary_search_by_key(&col, |(c, _)| *c)
            .ok()
            .map(|i| bucket[i].1)
    }

    /// Extract the text the user dragged across, from `start` to `end`
    /// (inclusive of both endpoints). Order is normalised so the
    /// caller can pass `start` and `end` in either order. Rows are
    /// joined by `\n`; within a row, columns are walked in ascending
    /// order and recorded chars are concatenated (missing cells are
    /// skipped). For a single-row selection only the column range on
    /// that row is read; for a multi-row selection the *first* row
    /// reads from `start.col` to end of recorded cells, intermediate
    /// rows read their full recorded extent, and the *last* row reads
    /// from column 0 to `end.col`.
    #[must_use]
    pub(crate) fn extract_range(&self, start: Cell, end: Cell) -> String {
        let (a, b) = if (start.row, start.col) <= (end.row, end.col) {
            (start, end)
        } else {
            (end, start)
        };
        let mut out = String::new();
        if a.row == b.row {
            self.append_row_range(a.row, a.col, b.col, &mut out);
            return out;
        }
        // First row: a.col .. row max
        self.append_row_range(a.row, a.col, u16::MAX, &mut out);
        for row in (a.row + 1)..b.row {
            out.push('\n');
            self.append_row_range(row, 0, u16::MAX, &mut out);
        }
        out.push('\n');
        self.append_row_range(b.row, 0, b.col, &mut out);
        out
    }

    fn append_row_range(&self, row: u16, lo: u16, hi: u16, out: &mut String) {
        let r = row as usize;
        if r >= self.rows.len() {
            return;
        }
        for &(c, ch) in &self.rows[r] {
            if c >= lo && c <= hi {
                out.push(ch);
            } else if c > hi {
                break;
            }
        }
    }
}

/// State machine for an in-progress or just-completed mouse selection.
/// Driven by crossterm `MouseEventKind::Down(Left)` / `Drag(Left)` /
/// `Up(Left)` events; orthogonal to scroll-wheel handling. The
/// rendered selection highlight reads `range()` each frame; the
/// clipboard write fires on `Up(Left)` and reads the same range.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) enum MouseSelection {
    /// No selection in progress. Initial state and the state after
    /// `Down` on a non-left button or `cancel()`.
    #[default]
    Idle,
    /// Left button is down; drag may or may not have moved yet.
    /// `start == end` for a click-without-drag.
    Selecting { start: Cell, end: Cell },
    /// Selection just completed (`Up(Left)`); range is final until
    /// the next `Down`. The render layer overlays the highlight; the
    /// event layer triggers the clipboard write once, then returns
    /// to `Idle` on the next `Down`.
    Done { start: Cell, end: Cell },
}

impl MouseSelection {
    /// `Down(Left)` at `cell` — anchor a new selection.
    pub(crate) fn on_down(&mut self, cell: Cell) {
        *self = Self::Selecting {
            start: cell,
            end: cell,
        };
    }

    /// `Drag(Left)` to `cell` — extend the current selection.
    /// No-op unless currently `Selecting`.
    pub(crate) fn on_drag(&mut self, cell: Cell) {
        if let Self::Selecting { start, .. } = *self {
            *self = Self::Selecting { start, end: cell };
        }
    }

    /// `Up(Left)` at `cell` — finalise the selection.
    /// No-op unless currently `Selecting`.
    pub(crate) fn on_up(&mut self, cell: Cell) {
        if let Self::Selecting { start, .. } = *self {
            *self = Self::Done { start, end: cell };
        }
    }

    /// Reset to `Idle`. Called when a non-left button is pressed, or
    /// when the user cancels mid-drag (e.g. presses Esc).
    pub(crate) fn cancel(&mut self) {
        *self = Self::Idle;
    }

    /// The currently-selected range, if any. Returned for both
    /// `Selecting` (live, draws the highlight) and `Done` (just
    /// completed, still visible until the next `Down`).
    #[must_use]
    pub(crate) fn range(&self) -> Option<(Cell, Cell)> {
        match self {
            Self::Idle => None,
            Self::Selecting { start, end } | Self::Done { start, end } => Some((*start, *end)),
        }
    }
}

#[cfg(test)]
mod position_map_tests {
    use super::*;

    #[test]
    fn empty_map_reads_none_for_any_cell() {
        let m = PositionMap::new();
        assert!(m.get(0, 0).is_none());
        assert!(m.get(100, 100).is_none());
    }

    #[test]
    fn record_then_get_round_trips() {
        let mut m = PositionMap::new();
        m.record(2, 5, 'X');
        assert_eq!(m.get(2, 5), Some('X'));
        assert_eq!(m.get(2, 4), None);
        assert_eq!(m.get(3, 5), None);
    }

    #[test]
    fn record_overwrites_at_same_cell() {
        let mut m = PositionMap::new();
        m.record(0, 0, 'a');
        m.record(0, 0, 'b');
        assert_eq!(m.get(0, 0), Some('b'));
    }

    #[test]
    fn extract_single_row_returns_substring() {
        let mut m = PositionMap::new();
        for (i, ch) in "hello".chars().enumerate() {
            m.record(0, u16::try_from(i).unwrap(), ch);
        }
        assert_eq!(m.extract_range(Cell::new(0, 0), Cell::new(0, 4)), "hello");
        assert_eq!(m.extract_range(Cell::new(0, 1), Cell::new(0, 3)), "ell");
    }

    #[test]
    fn extract_normalises_reversed_endpoints() {
        let mut m = PositionMap::new();
        for (i, ch) in "world".chars().enumerate() {
            m.record(0, u16::try_from(i).unwrap(), ch);
        }
        // start > end: should produce the same result as start < end.
        assert_eq!(m.extract_range(Cell::new(0, 4), Cell::new(0, 0)), "world");
    }

    #[test]
    fn extract_multi_row_joins_with_newline() {
        let mut m = PositionMap::new();
        for (i, ch) in "ab".chars().enumerate() {
            m.record(0, u16::try_from(i).unwrap(), ch);
        }
        for (i, ch) in "cd".chars().enumerate() {
            m.record(1, u16::try_from(i).unwrap(), ch);
        }
        // (0,0) .. (1,1) covers full first row + full second row.
        assert_eq!(m.extract_range(Cell::new(0, 0), Cell::new(1, 1)), "ab\ncd");
    }

    #[test]
    fn extract_multi_row_partial_endpoints() {
        let mut m = PositionMap::new();
        for (i, ch) in "abcdef".chars().enumerate() {
            m.record(0, u16::try_from(i).unwrap(), ch);
        }
        for (i, ch) in "ghijkl".chars().enumerate() {
            m.record(1, u16::try_from(i).unwrap(), ch);
        }
        // (0,3) .. (1,2): "def" + "\n" + "ghi"
        assert_eq!(
            m.extract_range(Cell::new(0, 3), Cell::new(1, 2)),
            "def\nghi",
        );
    }

    #[test]
    fn extract_skips_missing_cells() {
        let mut m = PositionMap::new();
        m.record(0, 0, 'a');
        m.record(0, 5, 'b'); // gap at cols 1-4
        m.record(0, 6, 'c');
        // Reads the recorded cells only; gaps are skipped, not filled.
        assert_eq!(m.extract_range(Cell::new(0, 0), Cell::new(0, 10)), "abc");
    }

    #[test]
    fn clear_empties_map_but_keeps_capacity() {
        let mut m = PositionMap::new();
        m.record(0, 0, 'x');
        m.clear();
        assert!(m.get(0, 0).is_none());
        // Subsequent record still works.
        m.record(0, 0, 'y');
        assert_eq!(m.get(0, 0), Some('y'));
    }
}

#[cfg(test)]
mod mouse_selection_tests {
    use super::*;

    #[test]
    fn default_is_idle_with_no_range() {
        let s = MouseSelection::default();
        assert!(s.range().is_none());
    }

    #[test]
    fn on_down_transitions_to_selecting_with_collapsed_range() {
        let mut s = MouseSelection::default();
        s.on_down(Cell::new(2, 3));
        assert_eq!(s.range(), Some((Cell::new(2, 3), Cell::new(2, 3))));
    }

    #[test]
    fn on_drag_extends_end_only_when_selecting() {
        let mut s = MouseSelection::default();
        s.on_down(Cell::new(0, 0));
        s.on_drag(Cell::new(0, 5));
        assert_eq!(s.range(), Some((Cell::new(0, 0), Cell::new(0, 5))));
    }

    #[test]
    fn on_drag_without_prior_down_is_noop() {
        let mut s = MouseSelection::default();
        s.on_drag(Cell::new(0, 5));
        assert!(s.range().is_none());
    }

    #[test]
    fn on_up_finalises_to_done() {
        let mut s = MouseSelection::default();
        s.on_down(Cell::new(0, 0));
        s.on_drag(Cell::new(0, 5));
        s.on_up(Cell::new(0, 5));
        assert!(matches!(s, MouseSelection::Done { .. }));
        assert_eq!(s.range(), Some((Cell::new(0, 0), Cell::new(0, 5))));
    }

    #[test]
    fn on_up_without_prior_down_is_noop() {
        let mut s = MouseSelection::default();
        s.on_up(Cell::new(0, 0));
        assert!(s.range().is_none());
    }

    #[test]
    fn cancel_returns_to_idle() {
        let mut s = MouseSelection::default();
        s.on_down(Cell::new(0, 0));
        s.on_drag(Cell::new(0, 5));
        s.cancel();
        assert_eq!(s, MouseSelection::Idle);
        assert!(s.range().is_none());
    }

    #[test]
    fn new_down_after_done_starts_fresh() {
        let mut s = MouseSelection::default();
        s.on_down(Cell::new(0, 0));
        s.on_up(Cell::new(0, 5));
        // Done state still exposes the range.
        assert!(s.range().is_some());
        // A new Down resets the anchor.
        s.on_down(Cell::new(2, 2));
        assert_eq!(s.range(), Some((Cell::new(2, 2), Cell::new(2, 2))));
    }
}
