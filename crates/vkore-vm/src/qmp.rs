//! QEMU's machine protocol over a private Unix socket: commands answered in
//! order, events kept until read.
//!
//! Every command carries an `id`, which QEMU echoes in its reply. A reply can
//! come late -- under TCG a busy guest can hold QEMU's main loop for seconds --
//! and a command that stopped waiting must not leave its reply to be taken for
//! the next command's, or for a broken stream: a reply to an earlier id is
//! dropped. A line is kept across reads, so a timeout that falls inside one
//! loses nothing.

use std::{
    collections::VecDeque,
    fmt,
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::{Duration, Instant},
};

use qapi_qmp as api;
use serde_json::{Value, json};

/// How long a command waits for its reply.
const REPLY_WAIT: Duration = Duration::from_secs(10);

/// The longest line QMP sends here; a longer one is not QEMU's.
const MAX_LINE: u64 = 1 << 20;
const MAX_EVENTS: usize = 256;
const MAX_EVENT_BYTES: usize = 4 << 20;

/// Known events use the upstream QAPI vocabulary. Newer QEMU events retain
/// their envelope so a new optional event does not break an older client.
#[derive(Debug)]
pub enum Event {
    Known(Box<api::Event>),
    Unknown(Value),
}

impl Event {
    pub fn into_value(self) -> Value {
        match self {
            Self::Known(event) => serde_json::to_value(event).expect("QAPI event serializes"),
            Self::Unknown(value) => value,
        }
    }
}

/// A QMP connection, past capability negotiation.
pub struct Qmp {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    events: VecDeque<(Event, usize)>,
    event_bytes: usize,
    /// The line being read when a timeout fell; its rest is still to come.
    partial: Vec<u8>,
    /// The id the next command carries; every earlier one has been sent.
    next_id: u64,
    reply_wait: Duration,
}

/// What a QMP exchange met.
#[derive(Debug)]
pub enum QmpError {
    Io(io::Error),
    /// QEMU refused the command: its error class and description.
    Command {
        class: String,
        description: String,
    },
    /// A line that is not QMP.
    Protocol(String),
}

impl fmt::Display for QmpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "QMP: {error}"),
            Self::Command { class, description } => write!(f, "QMP {class}: {description}"),
            Self::Protocol(line) => write!(f, "QMP: unexpected {line:?}"),
        }
    }
}

impl std::error::Error for QmpError {}

impl From<io::Error> for QmpError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl Qmp {
    /// Connect to `socket`, waiting up to `patience` for QEMU to create it,
    /// and leave capability negotiation behind.
    pub fn connect(socket: &Path, patience: Duration) -> Result<Self, QmpError> {
        Self::connect_while(socket, patience, || Ok(true))
    }

    /// Stop waiting immediately if the supervised emulator exits.
    pub(crate) fn connect_while(
        socket: &Path,
        patience: Duration,
        mut alive: impl FnMut() -> io::Result<bool>,
    ) -> Result<Self, QmpError> {
        let deadline = Instant::now() + patience;
        let stream = loop {
            if !alive()? {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "QEMU exited before QMP was ready",
                )
                .into());
            }
            match UnixStream::connect(socket) {
                Ok(stream) => break stream,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                    ) && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => return Err(error.into()),
            }
        };
        stream.set_write_timeout(Some(REPLY_WAIT))?;
        let writer = stream.try_clone()?;
        let mut qmp = Self {
            reader: BufReader::new(stream),
            writer,
            events: VecDeque::new(),
            event_bytes: 0,
            partial: Vec::new(),
            next_id: 1,
            reply_wait: REPLY_WAIT,
        };
        let greeting = qmp
            .read_message(deadline.saturating_duration_since(Instant::now()))?
            .ok_or_else(|| timed_out("its greeting", patience))?;
        if greeting.get("QMP").is_none() {
            return Err(QmpError::Protocol(greeting.to_string()));
        }
        qmp.reply_wait = deadline.saturating_duration_since(Instant::now());
        if qmp.reply_wait.is_zero() {
            return Err(timed_out("negotiation", patience));
        }
        qmp.writer.set_write_timeout(Some(qmp.reply_wait))?;
        qmp.execute(&api::qmp_capabilities { enable: None })?;
        qmp.reply_wait = REPLY_WAIT;
        Ok(qmp)
    }

    /// Send `command` and answer its id, without waiting for the reply.
    fn send<C: api::QmpCommand>(&mut self, command: &C) -> Result<u64, QmpError> {
        let arguments =
            serde_json::to_value(command).map_err(|error| QmpError::Protocol(error.to_string()))?;
        let command = C::NAME;
        if command.is_empty()
            || !command
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(QmpError::Protocol(format!("command name {command:?}")));
        }
        let id = self.next_id;
        let mut message = json!({ "execute": command, "id": id });
        message["arguments"] = arguments;
        let mut line = message.to_string();
        line.push('\n');
        self.writer.write_all(line.as_bytes())?;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| QmpError::Protocol("command ids exhausted".into()))?;
        Ok(id)
    }

    /// Run `command` and answer its `return` value; events read meanwhile
    /// are kept for [`Qmp::next_event`].
    pub fn execute<C: api::QmpCommand>(&mut self, command: &C) -> Result<C::Ok, QmpError> {
        let id = self.send(command)?;
        let deadline = Instant::now() + self.reply_wait;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let message = self
                .read_message(left)?
                .ok_or_else(|| timed_out(C::NAME, self.reply_wait))?;
            if let Some(reply) = self.file(message, Some(id))? {
                return serde_json::from_value(reply?)
                    .map_err(|error| QmpError::Protocol(format!("{} response: {error}", C::NAME)));
            }
        }
    }

    /// The next event, waiting up to `wait` for one; `None` when none came.
    pub fn next_event(&mut self, wait: Duration) -> Result<Option<Event>, QmpError> {
        let deadline = Instant::now() + wait;
        loop {
            if let Some((event, bytes)) = self.events.pop_front() {
                self.event_bytes -= bytes;
                return Ok(Some(event));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match self.read_message(left)? {
                Some(message) => {
                    self.file(message, None)?;
                }
                None => return Ok(None),
            }
        }
    }

    /// File `message`. An event joins the queue; a reply answers when it is
    /// the one `awaited`, and is dropped when it is late, its command having
    /// stopped waiting. Anything else is not QMP.
    fn file(
        &mut self,
        message: Value,
        awaited: Option<u64>,
    ) -> Result<Option<Result<Value, QmpError>>, QmpError> {
        if message.get("event").is_some() {
            let bytes = message.to_string().len();
            if self.events.len() >= MAX_EVENTS || self.event_bytes + bytes > MAX_EVENT_BYTES {
                return Err(QmpError::Protocol("event queue limit exceeded".into()));
            }
            if !message["event"].is_string() {
                return Err(QmpError::Protocol("event name is not a string".into()));
            }
            let event = match serde_json::from_value(message.clone()) {
                Ok(event) => Event::Known(Box::new(event)),
                Err(_) => Event::Unknown(message),
            };
            self.events.push_back((event, bytes));
            self.event_bytes += bytes;
            return Ok(None);
        }
        let reply = message.get("return").is_some() || message.get("error").is_some();
        match message.get("id").and_then(Value::as_u64) {
            Some(id) if reply && Some(id) == awaited => Ok(Some(match message.get("error") {
                None => Ok(message["return"].clone()),
                Some(error) => Err(QmpError::Command {
                    class: error["class"].as_str().unwrap_or("Error").to_owned(),
                    description: error["desc"].as_str().unwrap_or("").to_owned(),
                }),
            })),
            Some(id) if reply && id < self.next_id => Ok(None),
            _ => Err(QmpError::Protocol(message.to_string())),
        }
    }

    /// The next line, as JSON, waiting up to `wait` for it to complete;
    /// `None` when it did not. What was read of it stays for the next call.
    fn read_message(&mut self, wait: Duration) -> Result<Option<Value>, QmpError> {
        let deadline = Instant::now() + wait;
        loop {
            if self.partial.len() as u64 >= MAX_LINE {
                return Err(QmpError::Protocol(format!(
                    "a line longer than {MAX_LINE} bytes"
                )));
            }
            if self.reader.buffer().is_empty() {
                let remaining = deadline.saturating_duration_since(Instant::now());
                let timeout = rustix::event::Timespec::try_from(remaining)
                    .map_err(|_| QmpError::Protocol("QMP timeout is too large".into()))?;
                let mut descriptors = [rustix::event::PollFd::new(
                    self.reader.get_ref(),
                    rustix::event::PollFlags::IN,
                )];
                match rustix::event::poll(&mut descriptors, Some(&timeout)) {
                    Ok(0) => return Ok(None),
                    Ok(_) => {}
                    Err(rustix::io::Errno::INTR) => continue,
                    Err(error) => return Err(io::Error::from(error).into()),
                }
            }
            let buffer = self.reader.fill_buf()?;
            if buffer.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "QEMU closed its machine protocol",
                )
                .into());
            }
            let length = buffer
                .iter()
                .position(|b| *b == b'\n')
                .map_or(buffer.len(), |n| n + 1)
                .min(MAX_LINE as usize - self.partial.len());
            self.partial.extend_from_slice(&buffer[..length]);
            self.reader.consume(length);
            if self.partial.ends_with(b"\n") {
                let line = std::mem::take(&mut self.partial);
                return serde_json::from_slice(&line)
                    .map(Some)
                    .map_err(|error| QmpError::Protocol(format!("invalid JSON: {error}")));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
        }
    }

    /// `running`, `paused`, `shutdown`, … as QEMU reports the run state.
    pub fn status(&mut self) -> Result<api::StatusInfo, QmpError> {
        self.execute(&api::query_status {})
    }

    /// Release CPUs only after the supervisor has checked the stopped machine.
    pub fn resume(&mut self) -> Result<(), QmpError> {
        self.execute(&api::cont {}).map(drop)
    }

    /// Press the machine's ACPI power button.
    ///
    /// The reply is not waited for: a busy machine can hold QEMU's main loop
    /// for seconds, and a stop's grace period should be its only wait. The
    /// reply is dropped as late when it comes.
    pub fn power_button(&mut self) -> Result<(), QmpError> {
        self.send(&api::system_powerdown {}).map(drop)
    }

    /// End QEMU at once: the guest gets no notice. Not waited for either:
    /// the caller watches the process end.
    pub fn quit(&mut self) -> Result<(), QmpError> {
        self.send(&api::quit {}).map(drop)
    }

    /// Write the guest's screen to `path` as PPM.
    pub fn screendump(&mut self, path: &Path) -> Result<(), QmpError> {
        let filename = path
            .to_str()
            .ok_or_else(|| QmpError::Protocol(format!("{} is not UTF-8", path.display())))?;
        self.execute(&api::screendump {
            filename: filename.into(),
            device: None,
            head: None,
            format: None,
        })
        .map(drop)
    }

    /// Press and release keys together, as QEMU names them (`ctrl`,
    /// `print`, `a`, …), on the guest's keyboard.
    pub fn send_keys(&mut self, keys: &[&str]) -> Result<(), QmpError> {
        let keys = keys
            .iter()
            .map(|key| {
                key.parse()
                    .map(|data| api::KeyValue::qcode(api::QKeyCodeWrapper { data }))
                    .map_err(|_| QmpError::Protocol(format!("unknown QEMU key {key:?}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.execute(&api::send_key {
            keys,
            hold_time: None,
        })
        .map(drop)
    }
}

fn timed_out(what: &str, wait: Duration) -> QmpError {
    QmpError::Io(io::Error::new(
        io::ErrorKind::TimedOut,
        format!("QEMU did not answer {what} within {} s", wait.as_secs_f32()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    /// A scripted QEMU: the greeting, then `script` with the client's stream.
    fn qemu(
        script: impl FnOnce(&mut BufReader<UnixStream>, &mut UnixStream) + Send + 'static,
    ) -> (
        tempfile::TempDir,
        std::path::PathBuf,
        std::thread::JoinHandle<()>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("qmp.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            writeln!(
                stream,
                r#"{{"QMP": {{"version": {{}}, "capabilities": []}}}}"#
            )
            .unwrap();
            let (id, name) = command(&mut reader);
            assert_eq!(name, "qmp_capabilities");
            writeln!(stream, r#"{{"return": {{}}, "id": {id}}}"#).unwrap();
            script(&mut reader, &mut stream);
        });
        (dir, socket, server)
    }

    /// The next command the client sent: its id and name.
    fn command(reader: &mut BufReader<UnixStream>) -> (u64, String) {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let message: Value = serde_json::from_str(&line).unwrap();
        (
            message["id"].as_u64().expect("every command carries an id"),
            message["execute"].as_str().unwrap().to_owned(),
        )
    }

    #[test]
    fn negotiation_commands_and_events_in_order() {
        let (_dir, socket, server) = qemu(|reader, stream| {
            let (id, name) = command(reader);
            assert_eq!(name, "system_powerdown");
            writeln!(stream, r#"{{"event": "POWERDOWN", "timestamp": {{}}}}"#).unwrap();
            writeln!(stream, r#"{{"return": {{}}, "id": {id}}}"#).unwrap();
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert!(line.contains(r#""keys""#) && line.contains(r#""print""#));
            let id: Value = serde_json::from_str(&line).unwrap();
            let id = id["id"].as_u64().unwrap();
            writeln!(
                stream,
                r#"{{"error": {{"class": "GenericError", "desc": "no"}}, "id": {id}}}"#
            )
            .unwrap();
            writeln!(
                stream,
                r#"{{"event": "SHUTDOWN", "data": {{"reason": "guest-shutdown"}}}}"#
            )
            .unwrap();
        });
        let mut qmp = Qmp::connect(&socket, Duration::from_secs(5)).unwrap();
        qmp.power_button().unwrap();
        let sent = qmp.send_keys(&["print"]);
        assert!(matches!(
            sent,
            Err(QmpError::Command { class, .. }) if class == "GenericError"
        ));
        let first = qmp
            .next_event(Duration::from_secs(5))
            .unwrap()
            .unwrap()
            .into_value();
        assert_eq!(first["event"], "POWERDOWN");
        let second = qmp
            .next_event(Duration::from_secs(5))
            .unwrap()
            .unwrap()
            .into_value();
        assert_eq!(second["data"]["reason"], "guest-shutdown");
        server.join().unwrap();
        assert!(qmp.send_keys(&["not-a-key"]).is_err());
    }

    #[test]
    fn a_late_reply_is_dropped_not_taken_for_the_next_one() {
        let (_dir, socket, server) = qemu(|reader, stream| {
            // The power button's reply comes only after the next command,
            // as when a busy guest holds QEMU's main loop.
            let (button, _) = command(reader);
            let (status, name) = command(reader);
            assert_eq!(name, "query-status");
            writeln!(stream, r#"{{"return": {{}}, "id": {button}}}"#).unwrap();
            writeln!(
                stream,
                r#"{{"return": {{"status": "running", "running": true, "singlestep": false}}, "id": {status}}}"#
            )
            .unwrap();
            // And one that outlived its command's wait, read as an event is.
            let (slow, _) = command(reader);
            std::thread::sleep(Duration::from_millis(300));
            writeln!(stream, r#"{{"return": {{}}, "id": {slow}}}"#).unwrap();
            writeln!(stream, r#"{{"event": "STOP"}}"#).unwrap();
        });
        let mut qmp = Qmp::connect(&socket, Duration::from_secs(5)).unwrap();
        qmp.power_button().unwrap();
        assert!(qmp.status().unwrap().running);
        qmp.reply_wait = Duration::from_millis(50);
        assert!(matches!(
            qmp.status(),
            Err(QmpError::Io(error)) if error.kind() == io::ErrorKind::TimedOut
        ));
        let event = qmp
            .next_event(Duration::from_secs(5))
            .unwrap()
            .unwrap()
            .into_value();
        assert_eq!(event["event"], "STOP");
        server.join().unwrap();
    }

    #[test]
    fn a_timeout_inside_a_line_loses_nothing() {
        let (_dir, socket, server) = qemu(|_, stream| {
            write!(stream, r#"{{"event": "POW"#).unwrap();
            stream.flush().unwrap();
            std::thread::sleep(Duration::from_millis(300));
            writeln!(stream, r#"ERDOWN"}}"#).unwrap();
        });
        let mut qmp = Qmp::connect(&socket, Duration::from_secs(5)).unwrap();
        assert!(qmp.next_event(Duration::from_millis(50)).unwrap().is_none());
        let event = qmp
            .next_event(Duration::from_secs(5))
            .unwrap()
            .unwrap()
            .into_value();
        assert_eq!(event["event"], "POWERDOWN");
        server.join().unwrap();
    }

    #[test]
    fn a_line_past_the_bound_is_refused() {
        let (_dir, socket, server) = qemu(|_, stream| {
            let long = vec![b'x'; MAX_LINE as usize + 1];
            let _ = stream.write_all(&long);
        });
        let mut qmp = Qmp::connect(&socket, Duration::from_secs(5)).unwrap();
        let mut refused = None;
        for _ in 0..100 {
            match qmp.next_event(Duration::from_millis(100)) {
                Ok(None) => continue,
                other => {
                    refused = Some(other);
                    break;
                }
            }
        }
        assert!(matches!(refused, Some(Err(QmpError::Protocol(_)))));
        server.join().unwrap();
    }
    #[test]
    fn typed_responses_reject_missing_required_fields() {
        let (_dir, socket, server) = qemu(|reader, stream| {
            let (id, _) = command(reader);
            writeln!(stream, r#"{{"return": {{}}, "id": {id}}}"#).unwrap();
        });
        let mut qmp = Qmp::connect(&socket, Duration::from_secs(2)).unwrap();
        assert!(matches!(qmp.status(), Err(QmpError::Protocol(_))));
        server.join().unwrap();
    }

    #[test]
    fn event_flood_is_bounded_and_unknown_events_survive() {
        let (_dir, socket, server) = qemu(|_, _| {});
        let mut qmp = Qmp::connect(&socket, Duration::from_secs(2)).unwrap();
        server.join().unwrap();
        let event = json!({"event": "FUTURE_EVENT", "data": {"value": 7}});
        for _ in 0..MAX_EVENTS {
            qmp.file(event.clone(), None).unwrap();
        }
        assert!(matches!(
            qmp.file(event.clone(), None),
            Err(QmpError::Protocol(_))
        ));
        assert_eq!(
            qmp.next_event(Duration::ZERO)
                .unwrap()
                .unwrap()
                .into_value(),
            event
        );
        qmp.file(event, None).unwrap();
    }

    #[test]
    fn failed_process_does_not_wait_for_socket_deadline() {
        let start = Instant::now();
        assert!(
            Qmp::connect_while(Path::new("/no/socket"), Duration::from_secs(10), || Ok(
                false
            ))
            .is_err()
        );
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
