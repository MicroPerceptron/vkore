//! A bounded client and terminal screen for the existing COM2 shell socket.
mod terminal;
use std::{
    collections::VecDeque,
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    path::Path,
};
pub use terminal::{COLS, Color, MAX_COLS, MAX_ROWS, Output, ROWS, Run, Style};

pub const MAX_LINE_BYTES: usize = 1024;
pub const MAX_PENDING_BYTES: usize = 16 << 10;
const TRANSCRIPT_BYTES: usize = 16 << 10;
const READ_BYTES: usize = 4096;

#[derive(Debug)]
pub struct Console {
    stream: Option<UnixStream>,
    transcript: VecDeque<u8>,
    pending: Vec<u8>,
    sent: usize,
    error: Option<String>,
    terminal: Box<terminal::Terminal>,
}

impl Console {
    /// Connect from the machine worker, never the desktop event loop.
    pub fn connect(path: &Path) -> io::Result<Self> {
        Self::from_stream(UnixStream::connect(path)?)
    }

    fn from_stream(stream: UnixStream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream: Some(stream),
            transcript: VecDeque::new(),
            pending: Vec::new(),
            sent: 0,
            error: None,
            terminal: Box::default(),
        })
    }

    pub fn connected(&self) -> bool {
        self.stream.is_some()
    }

    pub fn can_send(&self) -> bool {
        self.connected() && self.pending.is_empty()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn transcript(&self) -> String {
        self.terminal.output.text()
    }

    pub fn output(&self) -> &Output {
        &self.terminal.output
    }

    /// Resize the host terminal grid. The COM2 stream has no guest resize
    /// protocol: changing geometry never injects bytes into the shell input.
    pub fn resize(&mut self, rows: u16, cols: u16) -> bool {
        self.terminal.resize(rows, cols)
    }

    /// Queue terminal input verbatim. Preserve typeahead across partial writes,
    /// bound pending bytes, and never replay any input after disconnection.
    pub fn send_input(&mut self, bytes: &[u8]) -> Result<(), &'static str> {
        if !self.connected() {
            return Err("The console is disconnected");
        }
        if bytes.len() > MAX_PENDING_BYTES - (self.pending.len() - self.sent) {
            return Err("Console input is full; wait for the guest to catch up");
        }
        if self.sent != 0 {
            self.pending.drain(..self.sent);
            self.sent = 0;
        }
        self.pending.extend_from_slice(bytes);
        Ok(())
    }

    /// Paste plain text, honoring the guest's bracketed-paste mode. Control
    /// sequences in clipboard content are refused instead of being executed.
    pub fn send_paste(&mut self, text: &str) -> Result<(), &'static str> {
        if text.len() > MAX_PENDING_BYTES - 12
            || text
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\r' | '\n' | '\t'))
        {
            return Err("Paste at most 16372 bytes of plain text without control sequences");
        }
        let normalized = text.replace("\r\n", "\n").replace('\n', "\r");
        let bytes = if self.output().bracketed_paste {
            format!("\x1b[200~{normalized}\x1b[201~").into_bytes()
        } else {
            normalized.into_bytes()
        };
        self.send_input(&bytes)
    }

    /// Queue one explicit line; no multiline paste, hidden control keys or
    /// automatic retry. A partial send is never replayed after a disconnect.
    pub fn send_line(&mut self, line: &str) -> Result<(), &'static str> {
        if line.len() > MAX_LINE_BYTES || line.chars().any(char::is_control) {
            return Err("Use one line of at most 1024 bytes, without control characters");
        }
        if !self.can_send() {
            return Err("The console is disconnected or is still sending the previous line");
        }
        self.pending.extend_from_slice(line.as_bytes());
        self.pending.push(b'\r');
        Ok(())
    }

    /// One nonblocking write and one bounded read per desktop tick. A guest
    /// flood cannot monopolize a frame or accumulate an unbounded event queue.
    pub fn poll(&mut self) -> bool {
        let Some(stream) = &mut self.stream else {
            return false;
        };
        let mut changed = false;
        if !self.pending.is_empty() {
            match stream.write(&self.pending[self.sent..]) {
                Ok(0) => return self.disconnect(Some("Console write closed".into())),
                Ok(n) => {
                    self.sent += n;
                    if self.sent == self.pending.len() {
                        self.pending.clear();
                        self.sent = 0;
                        changed = true;
                    }
                }
                Err(error) if transient(&error) => {}
                Err(error) => return self.disconnect(Some(error.to_string())),
            }
        }
        let mut bytes = [0; READ_BYTES];
        match stream.read(&mut bytes) {
            Ok(0) => self.disconnect(None),
            Ok(n) => {
                let remove = (self.transcript.len() + n).saturating_sub(TRANSCRIPT_BYTES);
                self.transcript.drain(..remove);
                self.transcript.extend(&bytes[..n]);
                self.terminal.process(&bytes[..n]);
                true
            }
            Err(error) if transient(&error) => changed,
            Err(error) => self.disconnect(Some(error.to_string())),
        }
    }

    fn disconnect(&mut self, error: Option<String>) -> bool {
        self.stream = None;
        self.error = if !self.pending.is_empty() {
            Some(
                "Console disconnected while sending; input may have been only partly delivered"
                    .into(),
            )
        } else {
            error
        };
        self.pending.clear();
        self.sent = 0;
        true
    }
}

fn transient(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resizing_never_injects_shell_input_or_discards_typeahead() {
        let (stream, mut guest) = UnixStream::pair().unwrap();
        guest.set_nonblocking(true).unwrap();
        let mut console = Console::from_stream(stream).unwrap();
        console.resize(20, 80);
        console.poll();
        let mut bytes = [0; 16];
        assert_eq!(
            guest.read(&mut bytes).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        console.send_input(b"help\r").unwrap();
        console.resize(40, 160);
        console.poll();
        let count = guest.read(&mut bytes).unwrap();
        assert_eq!(&bytes[..count], b"help\r");
        assert_eq!((console.output().rows, console.output().cols), (40, 160));
    }

    #[test]
    fn lines_are_bounded_explicit_and_never_replayed_after_disconnect() {
        let (stream, mut guest) = UnixStream::pair().unwrap();
        let mut console = Console::from_stream(stream).unwrap();
        for line in ["help\nreboot", "\u{1b}[A", &"x".repeat(MAX_LINE_BYTES + 1)] {
            assert!(console.send_line(line).is_err());
        }
        console.send_line("status devices").unwrap();
        assert!(console.send_line("reboot").is_err());
        assert!(console.poll());
        let mut line = [0; 15];
        guest.read_exact(&mut line).unwrap();
        assert_eq!(&line, b"status devices\r");
        assert!(console.can_send());
        guest.write_all(b"NPU bound\r\n> ").unwrap();
        assert!(console.poll());
        assert_eq!(console.transcript(), "NPU bound\n> ");
        drop(guest);
        assert!(console.poll());
        assert!(!console.connected());
        assert!(console.send_line("reboot").is_err());
    }

    #[test]
    fn raw_typeahead_is_bounded_ordered_and_never_locally_echoed() {
        let (stream, mut guest) = UnixStream::pair().unwrap();
        let mut console = Console::from_stream(stream).unwrap();
        for bytes in [
            b"st".as_slice(),
            b"atus",
            b"\x1b[D",
            b"\x7f",
            b"\x03",
            b"\r",
        ] {
            console.send_input(bytes).unwrap();
        }
        assert!(console.transcript().is_empty());
        assert!(console.poll());
        let mut input = [0; 12];
        guest.read_exact(&mut input).unwrap();
        assert_eq!(&input, b"status\x1b[D\x7f\x03\r");
        console.send_input(&vec![b'x'; MAX_PENDING_BYTES]).unwrap();
        assert!(console.send_input(b"y").is_err());
        assert_eq!(console.pending.len(), MAX_PENDING_BYTES);
        drop(guest);
        console.poll();
        assert!(!console.connected());
        assert!(console.pending.is_empty());
        assert!(console.send_input(b"reboot\r").is_err());
    }

    #[test]
    fn appending_after_a_partial_write_keeps_only_the_unsent_suffix() {
        let (stream, mut guest) = UnixStream::pair().unwrap();
        let mut console = Console::from_stream(stream).unwrap();
        console.send_input(b"abcd").unwrap();
        // Simulate the transport having accepted the first two bytes.
        console.sent = 2;
        console.send_input(b"ef").unwrap();
        console.poll();
        let mut bytes = [0; 4];
        guest.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"cdef");
    }

    #[test]
    fn paste_obeys_guest_mode_and_refuses_embedded_terminal_controls() {
        let (stream, mut guest) = UnixStream::pair().unwrap();
        let mut console = Console::from_stream(stream).unwrap();
        guest.write_all(b"\x1b[?2004h").unwrap();
        console.poll();
        console.send_paste("one\r\ntwo").unwrap();
        assert!(console.send_paste("\x1b[201~reboot\n").is_err());
        assert!(console.send_paste(&"x".repeat(MAX_PENDING_BYTES)).is_err());
        console.poll();
        let mut bytes = [0; 19];
        guest.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"\x1b[200~one\rtwo\x1b[201~");
    }

    #[test]
    fn a_flood_retains_only_the_bounded_tail_and_utf8_can_cross_reads() {
        let (stream, mut guest) = UnixStream::pair().unwrap();
        let mut console = Console::from_stream(stream).unwrap();
        for _ in 0..8 {
            guest.write_all(&[b'x'; READ_BYTES]).unwrap();
            console.poll();
        }
        assert_eq!(console.transcript.len(), TRANSCRIPT_BYTES);
        guest.write_all(&[0xc3]).unwrap();
        console.poll();
        guest.write_all(&[0xa9]).unwrap();
        console.poll();
        assert!(console.transcript().ends_with('é'));
        assert_eq!(console.transcript.len(), TRANSCRIPT_BYTES);
        assert!(!console.poll());
    }
}
