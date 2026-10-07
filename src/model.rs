//! Terminal semantics and selection, independent of the window and GPU.
use std::fmt::Write;

pub const INITIAL_ROWS: u16 = 24;
pub const INITIAL_COLS: u16 = 80;
// vt100 needs two columns for wide glyphs and two rows for safe autowrap.
pub const MIN_ROWS: u16 = 2;
pub const MIN_COLS: u16 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub row: u16,
    pub col: u16,
}

#[derive(Default)]
struct Callbacks {
    replies: Vec<u8>,
    title: Option<String>,
}

impl vt100::Callbacks for Callbacks {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = Some(String::from_utf8_lossy(title).into_owned());
    }

    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        first: Option<u8>,
        second: Option<u8>,
        params: &[&[u16]],
        command: char,
    ) {
        if first.is_some() || second.is_some() {
            return;
        }
        let param = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        let reply = match (command, param) {
            ('n', 5) => "\x1b[0n".to_owned(),
            ('n', 6) => {
                let (row, col) = screen.cursor_position();
                format!("\x1b[{};{}R", row + 1, col + 1)
            }
            ('c', 0) => "\x1b[?1;2c".to_owned(),
            _ => return,
        };
        self.replies.extend_from_slice(reply.as_bytes());
    }
}

pub struct Terminal {
    parser: vt100::Parser<Callbacks>,
    pub selection: Option<(Position, Position)>,
}

impl Terminal {
    pub fn new(rows: u16, cols: u16, scrollback_lines: usize) -> Self {
        Self {
            parser: vt100::Parser::new_with_callbacks(
                rows.max(MIN_ROWS),
                cols.max(MIN_COLS),
                scrollback_lines,
                Callbacks::default(),
            ),
            selection: None,
        }
    }

    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    pub fn process(&mut self, bytes: &[u8]) {
        // Selection coordinates refer to the current visible grid.
        self.selection = None;
        self.parser.process(bytes);
    }

    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.parser.callbacks_mut().replies)
    }

    pub fn take_title(&mut self) -> Option<String> {
        self.parser.callbacks_mut().title.take()
    }

    pub fn resize(&mut self, rows: u16, cols: u16) -> bool {
        let size = (rows.max(MIN_ROWS), cols.max(MIN_COLS));
        if size == self.screen().size() {
            return false;
        }
        self.selection = None;
        self.parser.screen_mut().set_size(size.0, size.1);
        true
    }

    pub fn scroll(&mut self, rows: isize) {
        if self.screen().alternate_screen() {
            return;
        }
        self.selection = None;
        let offset = self.screen().scrollback().saturating_add_signed(rows);
        self.parser.screen_mut().set_scrollback(offset);
    }

    pub fn scroll_to_bottom(&mut self) {
        self.parser.screen_mut().set_scrollback(0);
    }

    fn selection_bounds(&self) -> Option<(Position, Position)> {
        let (a, b) = self.selection?;
        let (mut start, mut end) = (a.min(b), a.max(b));
        if self
            .screen()
            .cell(start.row, start.col)?
            .is_wide_continuation()
        {
            start.col = start.col.saturating_sub(1);
        }
        if self.screen().cell(end.row, end.col)?.is_wide() {
            end.col = (end.col + 1).min(self.screen().size().1 - 1);
        }
        Some((start, end))
    }

    pub fn selected(&self, row: u16, col: u16) -> bool {
        let pos = Position { row, col };
        self.selection_bounds()
            .is_some_and(|(start, end)| start <= pos && pos <= end)
    }

    pub fn selected_text(&self) -> String {
        let Some((start, end)) = self.selection_bounds() else {
            return String::new();
        };
        let mut result = String::new();
        for row in start.row..=end.row {
            let first = if row == start.row { start.col } else { 0 };
            let last = if row == end.row {
                end.col
            } else {
                self.screen().size().1 - 1
            };
            let mut line = String::new();
            for col in first..=last {
                if let Some(cell) = self.screen().cell(row, col) {
                    if cell.is_wide_continuation() {
                        continue;
                    }
                    line.push_str(if cell.has_contents() {
                        cell.contents()
                    } else {
                        " "
                    });
                }
            }
            if row != end.row && self.screen().row_wrapped(row) {
                result.push_str(&line);
            } else {
                result.push_str(line.trim_end_matches(' '));
            }
            if row != end.row && !self.screen().row_wrapped(row) {
                result.push('\n');
            }
        }
        result
    }

    pub fn demo(&mut self) {
        let (rows, cols) = self.screen().size();
        let mut data = String::from("\x1b[2J\x1b[Hegui + wgpu terminal\r\n");
        data.push_str("\x1b[1mBold\x1b[0m  \x1b[2mDim\x1b[0m  \x1b[3mItalic\x1b[0m  \x1b[4mUnderline\x1b[0m  \x1b[7mInverse\x1b[0m\r\n");
        data.push_str("Wide: A界B語C  Combining: e\u{301} a\u{308}  Fallback: λ → ∑\r\n");
        for color in 0..16 {
            let _ = write!(data, "\x1b[48;5;{color}m  ");
        }
        data.push_str("\x1b[0m\r\nDrag to select; Ctrl+Shift+C copies.\r\n");
        for row in 6..rows {
            let _ = write!(
                data,
                "\x1b[{};1H\x1b[38;5;{}m{:04} ",
                row + 1,
                16 + row % 216,
                row
            );
            for col in 5..cols {
                data.push(char::from(b'0' + (col % 10) as u8));
            }
        }
        data.push_str("\x1b[0m\x1b[5;1H");
        self.process(data.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_utf8_combining_wide_and_ansi_keep_their_columns() {
        let mut term = Terminal::new(2, 8, 10_000);
        for byte in "A界e\u{301}\x1b[31mZ".as_bytes() {
            term.process(&[*byte]);
        }
        let screen = term.screen();
        assert!(screen.cell(0, 1).unwrap().is_wide());
        assert!(screen.cell(0, 2).unwrap().is_wide_continuation());
        assert_eq!(screen.cell(0, 3).unwrap().contents(), "e\u{301}");
        assert_eq!(screen.cell(0, 4).unwrap().contents(), "Z");
        assert_eq!(screen.cell(0, 4).unwrap().fgcolor(), vt100::Color::Idx(1));
        assert_eq!(screen.cursor_position(), (0, 5));
    }

    #[test]
    fn selection_normalizes_wide_cells_and_soft_wraps() {
        let mut term = Terminal::new(3, 4, 10_000);
        term.process("A界BC\r\nD".as_bytes());
        term.selection = Some((Position { row: 1, col: 0 }, Position { row: 0, col: 2 }));
        assert_eq!(term.selected_text(), "界BC");
        assert!(term.selected(0, 1));
        term.selection = Some((Position { row: 1, col: 0 }, Position { row: 2, col: 0 }));
        assert_eq!(term.selected_text(), "C\nD");
        let mut term = Terminal::new(2, 4, 10_000);
        term.process(b"ab  cd");
        term.selection = Some((Position { row: 0, col: 0 }, Position { row: 1, col: 1 }));
        assert_eq!(term.selected_text(), "ab  cd");
    }

    #[test]
    fn scrolling_resize_alternate_screen_and_replies() {
        let mut term = Terminal::new(2, 8, 10_000);
        term.process(b"one\r\ntwo\r\nthree");
        term.scroll(1);
        assert!(term.screen().contents().starts_with("one"));
        term.scroll_to_bottom();
        term.process(b"\x1b[?1049hALT\x1b[?1h\x1b[?2004h");
        assert!(term.screen().alternate_screen());
        assert!(term.screen().application_cursor());
        assert!(term.screen().bracketed_paste());
        term.process(b"\x1b[?1049l\x1b[1;2H\x1b[6n\x1b[5n");
        assert!(term.screen().contents().contains("three"));
        assert_eq!(term.take_replies(), b"\x1b[1;2R\x1b[0n");
        assert!(term.resize(0, 0));
        assert_eq!(term.screen().size(), (2, 2));
        term.process("界界界".as_bytes());
    }

    #[test]
    fn configured_scrollback_limits_history() {
        for limit in [0, 1, 3] {
            let mut term = Terminal::new(2, 8, limit);
            term.process(b"one\r\ntwo\r\nthree\r\nfour\r\nfive");
            term.scroll(100);
            assert_eq!(term.screen().scrollback(), limit);
        }
    }
}
