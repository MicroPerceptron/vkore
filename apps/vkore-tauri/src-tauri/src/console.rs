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
use vkored::console::{Color, Console, Output};

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
            spans: Vec::new(),
        }
    }

    fn of(console: &Console, note: Option<&str>) -> Self {
        let output: &Output = console.output();
        let error = note.or_else(|| console.error()).unwrap_or_default();
        Self {
            rows: output.rows,
            cols: output.cols,
            connected: console.connected(),
            error: shown(error),
            application_cursor: output.application_cursor,
            spans: bound(
                output
                    .runs
                    .iter()
                    .map(|run| Span {
                        text: escape_text(&run.text),
                        fg: Paint::from(run.style.foreground),
                        bg: Paint::from(run.style.background),
                        bold: run.style.bold,
                        dim: run.style.dim,
                        inverse: run.style.inverse,
                        underline: run.style.underline,
                        cursor: run.style.cursor,
                    })
                    .collect(),
            ),
        }
    }
}

/// Keep the newest `MAX_CHARS` characters, cutting whole spans from the front
/// and the first kept span at a character boundary.
fn bound(mut spans: Vec<Span>) -> Vec<Span> {
    let mut total: usize = spans.iter().map(|span| span.text.chars().count()).sum();
    let mut cut = 0;
    while total > MAX_CHARS && cut < spans.len() {
        let length = spans[cut].text.chars().count();
        if total - length >= MAX_CHARS {
            total -= length;
            cut += 1;
        } else {
            let drop = total - MAX_CHARS;
            let at = spans[cut]
                .text
                .char_indices()
                .nth(drop)
                .map_or(spans[cut].text.len(), |(at, _)| at);
            spans[cut].text.drain(..at);
            total = MAX_CHARS;
        }
    }
    spans.drain(..cut);
    spans
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
                    dirty |= console.resize(rows, cols);
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
        let spans = bound(vec![span(&big), span(&"b".repeat(100))]);
        let total: usize = spans.iter().map(|s| s.text.chars().count()).sum();
        assert_eq!(total, MAX_CHARS);
        assert!(spans.last().unwrap().text.ends_with('b'));
        assert_eq!(spans.last().unwrap().text.chars().count(), 100);
        // Whole spans go first; a short screen is untouched.
        assert_eq!(bound(vec![span("hi")]).len(), 1);
        let many = bound(vec![span(&"x".repeat(MAX_CHARS)), span("y")]);
        assert_eq!(
            many.iter().map(|s| s.text.chars().count()).sum::<usize>(),
            MAX_CHARS
        );
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
}
