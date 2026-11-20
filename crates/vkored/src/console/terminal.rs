//! Bounded terminal state. vt100/vte parse across reads; default callbacks do
//! not change host titles, clipboard, window size, or send terminal replies.
pub use vt100::Color;

const ROWS: u16 = 24;
const COLS: u16 = 120;
const HISTORY: usize = 104;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub foreground: Color,
    pub background: Color,
    pub bold: bool,
    pub dim: bool,
    pub inverse: bool,
    pub underline: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub style: Style,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Output {
    pub runs: Vec<Run>,
}

impl Output {
    pub fn text(&self) -> String {
        self.runs.iter().map(|run| run.text.as_str()).collect()
    }

    fn push(&mut self, text: &str, style: Style) {
        if let Some(last) = self.runs.last_mut()
            && last.style == style
        {
            last.text.push_str(text);
        } else {
            self.runs.push(Run {
                text: text.into(),
                style,
            });
        }
    }

    fn row(&mut self, screen: &vt100::Screen, row: u16) {
        let end = (0..COLS)
            .rev()
            .find(|col| {
                let cell = screen.cell(row, *col).unwrap();
                cell.has_contents() || cell.bgcolor() != Color::Default || cell.inverse()
            })
            .map_or(0, |col| col + 1);
        for col in 0..end {
            let cell = screen.cell(row, col).unwrap();
            if cell.is_wide_continuation() {
                continue;
            }
            self.push(
                if cell.has_contents() {
                    cell.contents()
                } else {
                    " "
                },
                Style {
                    foreground: cell.fgcolor(),
                    background: cell.bgcolor(),
                    bold: cell.bold(),
                    dim: cell.dim(),
                    inverse: cell.inverse(),
                    underline: cell.underline(),
                },
            );
        }
    }
}

pub(super) struct Terminal {
    parser: vt100::Parser,
    pub output: Output,
}

impl std::fmt::Debug for Terminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Terminal").finish_non_exhaustive()
    }
}

impl Default for Terminal {
    fn default() -> Self {
        Self {
            parser: vt100::Parser::new(ROWS.try_into().unwrap(), COLS.try_into().unwrap(), HISTORY),
            output: Output::default(),
        }
    }
}

impl Terminal {
    pub fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
        let screen = self.parser.screen_mut();
        let mut output = Output::default();
        // Walk retained rows without cloning the entire terminal grid. Restore
        // the viewport before the next parse so cursor semantics stay intact.
        // A row the guest's long line merely wrapped into continues the same
        // line: no newline, so the pane reflows it at its own width instead of
        // showing a hard break at column COLS (often mid-word).
        for offset in (1..=screen.scrollback_len()).rev() {
            screen.set_scrollback(offset);
            output.row(screen, 0);
            if !screen.row_wrapped(0) {
                output.push("\n", Style::default());
            }
        }
        screen.set_scrollback(0);
        let end = (0..ROWS)
            .rev()
            .find(|row| {
                *row <= screen.cursor_position().0
                    || (0..COLS).any(|col| screen.cell(*row, col).unwrap().has_contents())
            })
            .unwrap_or(0);
        for row in 0..=end {
            output.row(screen, row);
            if row < end && !screen.row_wrapped(row) {
                output.push("\n", Style::default());
            }
        }
        self.output = output;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_sequences_and_utf8_work_at_every_read_boundary() {
        let bytes = "\x1b[1;36mDEVICES\x1b[0m\r\n\x1b[?25hterminal:0> ls\r\nfs: \x1b[1;34mpictures/\x1b[m\r\ncafé".as_bytes();
        let mut whole = Terminal::default();
        whole.process(bytes);
        assert_eq!(
            whole.output.text(),
            "DEVICES\nterminal:0> ls\nfs: pictures/\ncafé"
        );
        assert!(
            whole.output.runs.iter().any(|r| r.text == "DEVICES"
                && r.style.foreground == Color::Idx(6)
                && r.style.bold)
        );
        for split in 0..=bytes.len() {
            let mut terminal = Terminal::default();
            terminal.process(&bytes[..split]);
            terminal.process(&bytes[split..]);
            assert_eq!(terminal.output, whole.output, "split {split}");
        }
    }

    #[test]
    fn a_line_longer_than_the_grid_stays_one_line_in_the_screen_and_in_scrollback() {
        let long: String = (0..COLS as usize * 2 + 7)
            .map(|i| (b'a' + (i % 26) as u8) as char)
            .collect();
        let mut terminal = Terminal::default();
        terminal.process(format!("{long}\r\nnext").as_bytes());
        assert_eq!(terminal.output.text(), format!("{long}\nnext"));
        // Pushed into scrollback, the wrapped rows still join.
        for _ in 0..ROWS {
            terminal.process(b"\r\nrow");
        }
        let text = terminal.output.text();
        assert!(text.starts_with(&format!("{long}\nnext\n")), "{text:?}");
    }

    #[test]
    fn cursor_edits_and_clear_are_applied_instead_of_only_stripping_codes() {
        let mut terminal = Terminal::default();
        terminal.process(b"progress 10%\rprogress 90%\x1b[K\r\nabc\x08D\x1b[1GZ");
        assert_eq!(terminal.output.text(), "progress 90%\nZbD");
        terminal.process(b"\x1b[1A\x1b[2K\rcomplete");
        assert_eq!(terminal.output.text(), "complete\nZbD");
        terminal.process(b"\x1b[2J\x1b[Hclear");
        assert_eq!(terminal.output.text(), "clear");
    }

    #[test]
    fn control_strings_and_scrollback_are_bounded_and_have_no_host_actions() {
        let mut terminal = Terminal::default();
        terminal.process(b"\x1b]52;c;");
        for _ in 0..256 {
            terminal.process(&[b'x'; 4096]);
        }
        terminal.process(b"\x07visible\x1bPignored\x1b\\\x1b]2;title\x1b\\");
        assert_eq!(terminal.output.text(), "visible");
        for _ in 0..1000 {
            terminal.process(b"row\r\n");
        }
        assert!(terminal.output.text().lines().count() <= ROWS as usize + HISTORY);
        assert!(terminal.parser.screen().scrollback_len() <= HISTORY);
        terminal.process(b"\x1b[8;65535;65535t");
        assert_eq!(
            terminal.parser.screen().size(),
            (ROWS.try_into().unwrap(), COLS.try_into().unwrap())
        );
    }
}
