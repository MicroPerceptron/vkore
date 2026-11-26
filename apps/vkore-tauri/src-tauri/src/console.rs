//! A machine's COM2 shell, shown in the window.
//!
//! The engine's `Console` is a bounded client with a terminal model: it parses
//! the guest's bytes (so no escape sequence ever reaches the page), keeps a
//! bounded screen and scrollback, and bounds pending input. This module only
//! owns it on a thread of its own, takes input over a bounded channel, and turns
//! the screen into the plain values the page draws. Guest text is escaped, and
//! the payload is cut to its newest [`MAX_CHARS`] characters.

use std::path::Path;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use serde::Serialize;
use vkored::console::{COLS, Color, Console, Output, ROWS};

use crate::shown::{escape_text, shown};

/// The most input one command may carry, in bytes.
pub const MAX_INPUT_BYTES: usize = 4096;
/// The most characters of screen and scrollback one update carries.
pub const MAX_CHARS: usize = 64 << 10;
/// Commands waiting for the console thread; a flood is refused, not queued.
pub const QUEUE: usize = 256;
/// The fewest time between two screens sent to the window.
const FRAME: Duration = Duration::from_millis(33);
/// Reads per turn, so a guest flood cannot starve input.
const READS_PER_TURN: usize = 8;

/// What the window asks of the console.
#[derive(Debug)]
pub enum Command {
    /// Terminal input as typed, escape sequences included.
    Input(Vec<u8>),
    /// Plain text, wrapped in bracketed paste when the guest asked for it.
    Paste(String),
    Resize {
        rows: u16,
        cols: u16,
    },
}

/// A colour in a run: the terminal's default, one of 256 indexed, or true colour.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Paint {
    Default(Option<()>),
    Indexed(u8),
    Rgb(String),
}

impl Paint {
    fn from(color: Color) -> Self {
        match color {
            Color::Default => Paint::Default(None),
            Color::Idx(index) => Paint::Indexed(index),
            Color::Rgb(r, g, b) => Paint::Rgb(format!("#{r:02x}{g:02x}{b:02x}")),
        }
    }
}

/// A stretch of text drawn in one style.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Span {
    pub text: String,
    pub fg: Paint,
    pub bg: Paint,
    pub bold: bool,
    pub dim: bool,
    pub inverse: bool,
    pub underline: bool,
    pub cursor: bool,
}

/// The console as the window draws it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Screen {
    pub rows: u16,
    pub cols: u16,
    pub connected: bool,
    /// Why input was refused or the console closed, when it was.
    pub error: String,
    /// Cursor keys send `ESC O` rather than `ESC [` while this is set.
    pub application_cursor: bool,
    /// Where the guest's cursor is, as a character offset into the text of `spans`
    /// (padded with spaces when the cursor is past the end), when it is shown and
    /// at the end of the output, where a shell's prompt keeps it.
    pub cursor: Option<usize>,
    pub spans: Vec<Span>,
}

impl Screen {
    /// A console that never connected, or has closed.
    pub fn closed(error: &str) -> Self {
        Self {
            rows: 0,
            cols: 0,
            connected: false,
            error: shown(error),
            application_cursor: false,
            cursor: None,
            spans: Vec::new(),
        }
    }

    fn of(console: &Console, note: Option<&str>) -> Self {
        let output: &Output = console.output();
        let error = note.or_else(|| console.error()).unwrap_or_default();
        let (runs, cursor) = with_cursor_padding(output);
        let mut mapped = cursor;
        let spans = runs
            .iter()
            .map(|run| Span {
                text: escape_text(&run.text),
                fg: Paint::from(run.style.foreground),
                bg: Paint::from(run.style.background),
                bold: run.style.bold,
                dim: run.style.dim,
                inverse: run.style.inverse,
                underline: run.style.underline,
                cursor: false,
            })
            .collect::<Vec<_>>();
        // Escaping lengthens hidden characters, so the offset is carried across it.
        if let Some(offset) = cursor {
            mapped = Some(escaped_offset(&runs, offset));
        }
        let (spans, cursor) = bound(spans, mapped);
        Self {
            rows: output.rows,
            cols: output.cols,
            connected: console.connected(),
            error: shown(error),
            application_cursor: output.application_cursor,
            cursor,
            spans,
        }
    }
}

/// The transcript's runs and where the guest's cursor sits in them.
///
/// The engine marks the cursor only in its viewport grid, not in the transcript this
/// window draws (which reflows to the pane). A shell keeps its cursor on the last
/// line, so when the cursor is on the grid's last row that has anything on it, the
/// transcript's last line is that row and the cursor is `column` characters into it.
/// Anywhere else (a program moving the cursor about the screen) there is no caret.
/// A cursor past the end of the text, after a prompt's trailing space, is reached by
/// padding the last run with spaces.
fn with_cursor_padding(output: &Output) -> (Vec<vkored::console::Run>, Option<usize>) {
    let mut runs = output.runs.clone();
    let Some(offset) = cursor_offset(output) else {
        return (runs, None);
    };
    let total: usize = runs.iter().map(|run| run.text.chars().count()).sum();
    if offset > total {
        let pad = " ".repeat(offset - total);
        match runs.last_mut() {
            Some(last) => last.text.push_str(&pad),
            None => runs.push(vkored::console::Run {
                text: pad,
                style: vkored::console::Style::default(),
            }),
        }
    }
    (runs, Some(offset))
}

/// The cursor's character offset into the transcript, when it is on the last line.
fn cursor_offset(output: &Output) -> Option<usize> {
    let grid: String = output.grid.iter().map(|run| run.text.as_str()).collect();
    let mut before = String::new();
    let mut at = None;
    for run in &output.grid {
        if run.style.cursor {
            at = Some(before.chars().count());
            break;
        }
        before.push_str(&run.text);
    }
    let at = at?;
    let chars: Vec<char> = grid.chars().collect();
    // The grid's physical rows are separated by newlines.
    let row_start = chars[..at]
        .iter()
        .rposition(|c| *c == '\n')
        .map_or(0, |i| i + 1);
    let column = at - row_start;
    let row_end = chars[at..]
        .iter()
        .position(|c| *c == '\n')
        .map_or(chars.len(), |i| at + i);
    // Anything on a later row means the cursor is not at the end of the output.
    if chars[row_end..].iter().any(|c| !c.is_whitespace()) {
        return None;
    }
    // The transcript's last line is this row, except that the row also holds the
    // cursor's own cell: a blank one is not in the transcript, but a space the guest
    // wrote is. Match the row against the transcript's real tail to tell which.
    let transcript: String = output.runs.iter().map(|run| run.text.as_str()).collect();
    let row: String = chars[row_start..row_end].iter().collect();
    let content = if transcript.ends_with(&row) {
        row.chars().count()
    } else if let Some(shorter) = row
        .strip_suffix(' ')
        .filter(|shorter| transcript.ends_with(shorter))
    {
        shorter.chars().count()
    } else {
        row.trim_end_matches(' ').chars().count()
    };
    let total = transcript.chars().count();
    Some(total.saturating_sub(content) + column)
}

/// `offset` (in characters of the unescaped runs) as an offset into their escaped text.
fn escaped_offset(runs: &[vkored::console::Run], offset: usize) -> usize {
    let mut remaining = offset;
    let mut mapped = 0;
    for run in runs {
        for c in run.text.chars() {
            if remaining == 0 {
                return mapped;
            }
            remaining -= 1;
            mapped += escape_text(c.encode_utf8(&mut [0; 4])).chars().count();
        }
    }
    mapped
}

/// Keep the newest `MAX_CHARS` characters, cutting whole spans from the front
/// and the first kept span at a character boundary. `cursor` moves with the text
/// and is dropped if the text it pointed into was cut.
fn bound(mut spans: Vec<Span>, cursor: Option<usize>) -> (Vec<Span>, Option<usize>) {
    let mut total: usize = spans.iter().map(|span| span.text.chars().count()).sum();
    let mut cut = 0;
    let mut dropped = 0;
    while total > MAX_CHARS && cut < spans.len() {
        let length = spans[cut].text.chars().count();
        if total - length >= MAX_CHARS {
            total -= length;
            dropped += length;
            cut += 1;
        } else {
            let drop = total - MAX_CHARS;
            let at = spans[cut]
                .text
                .char_indices()
                .nth(drop)
                .map_or(spans[cut].text.len(), |(at, _)| at);
            spans[cut].text.drain(..at);
            dropped += drop;
            total = MAX_CHARS;
        }
    }
    spans.drain(..cut);
    (spans, cursor.and_then(|at| at.checked_sub(dropped)))
}

/// Own the console at `socket` until the window drops the sender or the guest
/// closes it, publishing the screen as it changes (at most once per frame) and
/// once more when it closes.
pub fn run(socket: &Path, commands: &Receiver<Command>, publish: &dyn Fn(Screen)) {
    let mut console = match Console::connect(socket) {
        Ok(console) => console,
        Err(error) => {
            publish(Screen::closed(&format!(
                "the console did not connect: {error}"
            )));
            return;
        }
    };
    let mut note: Option<&'static str> = None;
    let mut dirty = true;
    let mut last = Instant::now()
        .checked_sub(FRAME)
        .unwrap_or_else(Instant::now);
    loop {
        let first = commands.recv_timeout(Duration::from_millis(10));
        let mut next = match first {
            Ok(command) => Some(command),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        while let Some(command) = next.take() {
            let applied = match command {
                Command::Input(bytes) => console.send_input(&bytes),
                Command::Paste(text) => console.send_paste(&text),
                Command::Resize { rows, cols } => {
                    // Never below the engine's default grid: narrowing it crops what
                    // the guest has already printed (a banner ends mid-word), and the
                    // page reflows its text to the pane without needing a narrower one.
                    dirty |= console.resize(rows.max(ROWS), cols.max(COLS));
                    Ok(())
                }
            };
            if let Err(error) = applied {
                note = Some(error);
                dirty = true;
            } else if note.take().is_some() {
                dirty = true;
            }
            next = commands.try_recv().ok();
        }
        for _ in 0..READS_PER_TURN {
            if !console.poll() {
                break;
            }
            dirty = true;
        }
        if !console.connected() {
            publish(Screen::of(&console, note));
            return;
        }
        if dirty && last.elapsed() >= FRAME {
            publish(Screen::of(&console, note));
            dirty = false;
            last = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;
    use std::sync::{Arc, Mutex, mpsc};

    fn span(text: &str) -> Span {
        Span {
            text: text.into(),
            fg: Paint::Default(None),
            bg: Paint::Default(None),
            bold: false,
            dim: false,
            inverse: false,
            underline: false,
            cursor: false,
        }
    }

    #[test]
    fn the_screen_keeps_its_newest_characters() {
        let big = "a".repeat(MAX_CHARS - 10);
        let (spans, _) = bound(vec![span(&big), span(&"b".repeat(100))], None);
        let total: usize = spans.iter().map(|s| s.text.chars().count()).sum();
        assert_eq!(total, MAX_CHARS);
        assert!(spans.last().unwrap().text.ends_with('b'));
        assert_eq!(spans.last().unwrap().text.chars().count(), 100);
        // Whole spans go first; a short screen is untouched.
        let (short, at) = bound(vec![span("hi")], Some(2));
        assert_eq!((short.len(), at), (1, Some(2)));
        let (many, cursor) = bound(
            vec![span(&"x".repeat(MAX_CHARS)), span("y")],
            Some(MAX_CHARS + 1),
        );
        assert_eq!(cursor, Some(MAX_CHARS));
        assert_eq!(
            many.iter().map(|s| s.text.chars().count()).sum::<usize>(),
            MAX_CHARS
        );
    }

    #[test]
    fn a_screen_has_camel_case_keys() {
        let mut span = span("hi");
        span.fg = Paint::from(Color::Idx(3));
        let screen = Screen {
            rows: 24,
            cols: 80,
            connected: true,
            error: String::new(),
            application_cursor: false,
            cursor: Some(2),
            spans: vec![span],
        };
        let wire = serde_json::to_value(&screen).unwrap();
        crate::assert_camel_keys(&wire);
        assert_eq!(wire["applicationCursor"], false);
    }

    #[test]
    fn colours_serialise_to_what_the_page_reads() {
        let json = serde_json::to_string(&[
            Paint::from(Color::Default),
            Paint::from(Color::Idx(12)),
            Paint::from(Color::Rgb(255, 0, 16)),
        ])
        .unwrap();
        assert_eq!(json, r##"[null,12,"#ff0010"]"##);
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("vkore-tauri-console-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_guest_line_reaches_the_screen_and_typed_input_reaches_the_guest() {
        let dir = scratch("io");
        let socket = dir.join("com2.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let (send, commands) = mpsc::sync_channel(QUEUE);
        let screens: Arc<Mutex<Vec<Screen>>> = Arc::default();
        let seen = screens.clone();
        let worker = std::thread::spawn(move || {
            run(&socket, &commands, &move |screen| {
                seen.lock().unwrap().push(screen)
            });
        });
        let (mut guest, _) = listener.accept().unwrap();
        guest.write_all(b"kore> \x1b[31mhello\x1b[0m\r\n").unwrap();
        send.send(Command::Input(b"ls\r".to_vec())).unwrap();

        let mut typed = [0u8; 3];
        guest
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        guest.read_exact(&mut typed).unwrap();
        assert_eq!(&typed, b"ls\r");

        let deadline = Instant::now() + Duration::from_secs(3);
        let text = loop {
            let text: String = screens
                .lock()
                .unwrap()
                .last()
                .map(|screen| screen.spans.iter().map(|s| s.text.as_str()).collect())
                .unwrap_or_default();
            if text.contains("hello") || Instant::now() > deadline {
                break text;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(text.contains("kore> hello"), "{text:?}");
        assert!(
            !text.contains('\u{1b}'),
            "an escape sequence reached the page: {text:?}"
        );
        assert!(screens.lock().unwrap().last().unwrap().connected);

        // The guest hanging up closes the console and says so.
        drop(guest);
        worker.join().unwrap();
        assert!(!screens.lock().unwrap().last().unwrap().connected);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn dropping_the_sender_ends_the_thread() {
        let dir = scratch("drop");
        let socket = dir.join("com2.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let (send, commands) = mpsc::sync_channel::<Command>(QUEUE);
        let worker = std::thread::spawn(move || run(&socket, &commands, &|_| {}));
        let _guest = listener.accept().unwrap();
        drop(send);
        worker.join().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_socket_that_is_not_there_is_reported_not_retried() {
        let (_send, commands) = mpsc::sync_channel::<Command>(QUEUE);
        let seen: Mutex<Vec<Screen>> = Mutex::default();
        run(Path::new("/nonexistent/com2.sock"), &commands, &|s| {
            seen.lock().unwrap().push(s)
        });
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert!(!seen[0].connected && seen[0].error.contains("did not connect"));
    }
    /// A live console on a socket, to feed and watch.
    struct Rig {
        guest: std::os::unix::net::UnixStream,
        send: mpsc::SyncSender<Command>,
        screens: Arc<Mutex<Vec<Screen>>>,
        worker: Option<std::thread::JoinHandle<()>>,
        dir: std::path::PathBuf,
    }

    impl Rig {
        fn new(tag: &str) -> Self {
            let dir = scratch(tag);
            let socket = dir.join("com2.sock");
            let listener = UnixListener::bind(&socket).unwrap();
            let (send, commands) = mpsc::sync_channel(QUEUE);
            let screens: Arc<Mutex<Vec<Screen>>> = Arc::default();
            let seen = screens.clone();
            let worker = std::thread::spawn(move || {
                run(&socket, &commands, &move |screen| {
                    seen.lock().unwrap().push(screen)
                });
            });
            let (guest, _) = listener.accept().unwrap();
            Rig {
                guest,
                send,
                screens,
                worker: Some(worker),
                dir,
            }
        }

        fn feed(&mut self, bytes: &[u8]) {
            self.guest.write_all(bytes).unwrap();
        }

        /// The newest screen once `ready` holds for it.
        fn screen(&self, ready: impl Fn(&Screen) -> bool) -> Screen {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if let Some(screen) = self.screens.lock().unwrap().last().cloned().filter(&ready) {
                    return screen;
                }
                assert!(
                    Instant::now() < deadline,
                    "the console never showed what was fed"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let (dead, _) = mpsc::sync_channel(1);
            drop(std::mem::replace(&mut self.send, dead));
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn text(screen: &Screen) -> String {
        screen.spans.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn a_narrow_window_does_not_cut_what_the_guest_already_printed() {
        let mut rig = Rig::new("narrow");
        let banner = "kore 0.40.0 (8e0cd864d) - programs: <image> [args]; a command file is run with source <file> (help source)";
        rig.feed(format!("{banner}\r\nterminal:0> ").as_bytes());
        rig.screen(|s| text(s).contains("(help source)"));
        // The tile is only 56 columns wide: the engine's grid must not follow it down.
        rig.send
            .send(Command::Resize { rows: 10, cols: 56 })
            .unwrap();
        std::thread::sleep(Duration::from_millis(250));
        let screen = rig.screen(|_| true);
        assert!(
            text(&screen).contains(banner),
            "the banner was cut: {:?}",
            text(&screen)
        );
        assert_eq!((screen.cols, screen.rows), (COLS, ROWS));
        // A wider window is honoured.
        rig.send
            .send(Command::Resize {
                rows: 40,
                cols: 200,
            })
            .unwrap();
        let screen = rig.screen(|s| s.cols == 200);
        assert_eq!(screen.rows, 40);
        assert!(text(&screen).contains(banner));
    }

    #[test]
    fn the_cursor_is_drawn_where_the_guest_keeps_it() {
        let mut rig = Rig::new("cursor");
        // After a prompt with a trailing space, the cursor is past the last character.
        rig.feed(b"terminal:0> ");
        let screen = rig.screen(|s| text(s).contains("terminal:0>"));
        assert_eq!(screen.cursor, Some(12));
        assert_eq!(
            text(&screen).chars().count(),
            12,
            "padded to the cursor: {:?}",
            text(&screen)
        );
        crate::assert_camel_keys(&serde_json::to_value(&screen).unwrap());

        // Typing moves it; moving left to edit puts it back inside the line.
        rig.feed(b"abc");
        let screen = rig.screen(|s| text(s).contains("abc"));
        assert_eq!(screen.cursor, Some(15));
        rig.feed(b"\x1b[2D");
        let screen = rig.screen(|s| s.cursor == Some(13));
        assert_eq!(text(&screen), "terminal:0> abc");

        // A character the page escapes is longer, and the cursor offset follows it.
        rig.feed(b"\r\nx\xe2\x80\x8by");
        let screen = rig.screen(|s| text(s).ends_with("y"));
        assert_eq!(screen.cursor, Some(text(&screen).chars().count()));
    }

    #[test]
    fn no_cursor_is_drawn_when_the_guest_hides_it_or_is_not_at_the_end() {
        let mut rig = Rig::new("nocursor");
        rig.feed(b"one\r\ntwo\r\n");
        rig.screen(|s| text(s).contains("two"));
        // A program moving the cursor to the top of a screen with text below it.
        rig.feed(b"\x1b[1;1H");
        let screen = rig.screen(|s| s.cursor.is_none() && text(s).contains("two"));
        assert_eq!(screen.cursor, None);
        // And a hidden cursor stays hidden.
        rig.feed(b"\x1b[?25l\x1b[3;1H");
        let screen = rig.screen(|s| s.cursor.is_none());
        assert_eq!(screen.cursor, None);
    }
}
