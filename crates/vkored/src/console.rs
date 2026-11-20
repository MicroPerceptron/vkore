//! A bounded client and terminal screen for the existing COM2 shell socket.
mod terminal;
use std::{
    collections::VecDeque,
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    path::Path,
};
pub use terminal::{Color, Output, Run, Style};

pub const MAX_LINE_BYTES: usize = 1024;
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
                "Console disconnected while sending; the line may have been only partly delivered"
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
