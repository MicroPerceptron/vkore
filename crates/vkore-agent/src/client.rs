//! One streamed chat completion, read within bounds.
//!
//! The reply is read on a thread of its own and handed over line by line, so a
//! person's Stop takes effect at once even while the server is silent; the
//! abandoned reader ends at its next line or read timeout and drops the
//! connection. Nothing is retried: a request that may have run is not replayed.

use std::io::{BufRead, BufReader, Read};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use crate::config::Endpoint;
use crate::protocol::{Chunk, Message, Request, Tool, ToolCall, ToolCallDelta};

/// The longest one line of the stream may be.
pub const MAX_LINE_BYTES: usize = 256 << 10;
/// The most a whole stream may carry.
pub const MAX_STREAM_BYTES: usize = 8 << 20;
/// The most text one reply may hold.
pub const MAX_TEXT_BYTES: usize = 64 << 10;
/// The most tool calls one reply may make.
pub const MAX_CALLS: usize = 8;
/// The most argument text one tool call may carry.
pub const MAX_ARGUMENT_BYTES: usize = 8 << 10;
const MAX_ERROR_BYTES: u64 = 8 << 10;
const CONNECT: Duration = Duration::from_secs(10);
/// How long the server may stay silent between two reads.
const IDLE: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(50);

/// A finished reply.
#[derive(Debug, Default, PartialEq)]
pub struct Reply {
    pub text: String,
    pub calls: Vec<ToolCall>,
    /// The server stopped at its own length limit.
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelError {
    /// The person stopped it.
    Cancelled,
    /// Anything else, in words fit to show.
    Failed(String),
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("stopped"),
            Self::Failed(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for ModelError {}

fn failed(why: impl Into<String>) -> ModelError {
    ModelError::Failed(why.into())
}

/// Something that answers a conversation. [`HttpModel`] is the real one.
pub trait Model {
    /// Complete `messages`, calling `text` with each piece as it arrives.
    fn complete(
        &mut self,
        messages: &[Message],
        tools: &[Tool],
        text: &mut dyn FnMut(&str),
        cancel: &AtomicBool,
    ) -> Result<Reply, ModelError>;
}

/// A model behind an OpenAI-compatible endpoint.
pub struct HttpModel {
    endpoint: Endpoint,
    agent: ureq::Agent,
}

impl HttpModel {
    pub fn new(endpoint: Endpoint) -> Result<Self, ModelError> {
        let tls = native_tls::TlsConnector::new()
            .map_err(|_| failed("could not initialize secure connections"))?;
        let agent = ureq::AgentBuilder::new()
            .tls_connector(Arc::new(tls))
            // A redirect could carry the key, and the machine's text, elsewhere.
            .redirects(0)
            .timeout_connect(CONNECT)
            .timeout_read(IDLE)
            .timeout_write(IDLE)
            .user_agent(concat!("vKore/", env!("CARGO_PKG_VERSION")))
            .build();
        Ok(Self { endpoint, agent })
    }
}

/// What the reader thread passes on.
enum Piece {
    Line(Vec<u8>),
    End,
    Failed(String),
}

/// `text` with the key removed, in case a server or a library echoes it.
fn scrub(endpoint: &Endpoint, text: &str) -> String {
    let text: String = text.chars().take(500).collect();
    match endpoint.key() {
        Some(key) => text.replace(key.expose(), "[key]"),
        None => text,
    }
}

fn error_message(endpoint: &Endpoint, status: u16, response: ureq::Response) -> String {
    let mut body = Vec::new();
    let _ = response
        .into_reader()
        .take(MAX_ERROR_BYTES)
        .read_to_end(&mut body);
    let said = serde_json::from_slice::<Chunk>(&body)
        .ok()
        .and_then(|chunk| chunk.error)
        .map(|error| error.message)
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| String::from_utf8_lossy(&body).trim().to_owned());
    let said = scrub(endpoint, &said);
    if said.is_empty() {
        format!("{} answered {status}", endpoint.host())
    } else {
        format!("{} answered {status}: {said}", endpoint.host())
    }
}

/// Read one line of at most [`MAX_LINE_BYTES`]; `None` at the end.
fn read_line(reader: &mut impl BufRead) -> Result<Option<Vec<u8>>, String> {
    let mut line = Vec::new();
    let read = reader
        .take(MAX_LINE_BYTES as u64 + 1)
        .read_until(b'\n', &mut line)
        .map_err(|error| format!("the reply stopped arriving: {error}"))?;
    if read == 0 {
        return Ok(None);
    }
    if line.len() > MAX_LINE_BYTES {
        return Err("the reply has a line longer than the agent accepts".into());
    }
    Ok(Some(line))
}

/// Gathers a reply from the stream's events.
#[derive(Default)]
struct Gather {
    reply: Reply,
    /// Each call's index on the wire, beside the call in `reply.calls`.
    indexes: Vec<u32>,
    finished: bool,
    done: bool,
}

impl Gather {
    fn line(&mut self, line: &[u8], text: &mut dyn FnMut(&str)) -> Result<(), ModelError> {
        let Some(data) = line.strip_prefix(b"data:") else {
            // Comments, other fields and the blank lines between events.
            return Ok(());
        };
        let data = data.trim_ascii();
        if data.is_empty() {
            return Ok(());
        }
        if data == b"[DONE]" {
            self.done = true;
            return Ok(());
        }
        let chunk: Chunk = serde_json::from_slice(data)
            .map_err(|_| failed("the server sent an event that is not a chat completion chunk"))?;
        if let Some(error) = chunk.error {
            let said: String = error.message.chars().take(500).collect();
            return Err(failed(format!("the server reported an error: {said}")));
        }
        for choice in chunk.choices.into_iter().filter(|choice| choice.index == 0) {
            for piece in [choice.delta.content, choice.delta.refusal]
                .into_iter()
                .flatten()
            {
                if self.reply.text.len() + piece.len() > MAX_TEXT_BYTES {
                    return Err(failed("the reply is longer than the agent accepts"));
                }
                self.reply.text.push_str(&piece);
                text(&piece);
            }
            for delta in choice.delta.tool_calls.into_iter().flatten() {
                self.call(delta)?;
            }
            if let Some(reason) = choice.finish_reason {
                self.finished = true;
                self.reply.truncated |= reason == "length";
            }
        }
        Ok(())
    }

    /// A piece of a tool call. Calls are told apart by `index`; a server that
    /// leaves it out starts a new call by giving a new `id`.
    fn call(&mut self, delta: ToolCallDelta) -> Result<(), ModelError> {
        let id = delta.id.filter(|id| !id.is_empty());
        let at = match delta.index {
            Some(index) => self.indexes.iter().position(|known| *known == index),
            None => match &id {
                Some(id) => self.reply.calls.iter().position(|call| call.id == *id),
                None => self.reply.calls.len().checked_sub(1),
            },
        };
        let at = match at {
            Some(at) => at,
            None => {
                if self.reply.calls.len() == MAX_CALLS {
                    return Err(failed(
                        "the reply makes more tool calls than the agent accepts",
                    ));
                }
                self.indexes
                    .push(delta.index.unwrap_or(self.reply.calls.len() as u32));
                self.reply.calls.push(ToolCall::new("", "", ""));
                self.reply.calls.len() - 1
            }
        };
        let call = &mut self.reply.calls[at];
        if let Some(id) = id {
            if id.len() > 128 {
                return Err(failed(
                    "a tool call has an id longer than the agent accepts",
                ));
            }
            call.id = id;
        }
        if let Some(function) = delta.function {
            if let Some(name) = function.name.filter(|name| !name.is_empty()) {
                // Some servers repeat the whole name in every piece.
                if call.function.name != name {
                    call.function.name.push_str(&name);
                }
            }
            if let Some(arguments) = function.arguments {
                call.function.arguments.push_str(&arguments);
            }
            if call.function.name.len() > 64 || call.function.arguments.len() > MAX_ARGUMENT_BYTES {
                return Err(failed("a tool call is larger than the agent accepts"));
            }
        }
        Ok(())
    }

    fn finish(mut self) -> Result<Reply, ModelError> {
        if !self.done && !self.finished {
            return Err(failed("the reply ended before it was complete"));
        }
        // Every call needs an id of its own for its result to answer.
        for at in 0..self.reply.calls.len() {
            let id = &self.reply.calls[at].id;
            let repeated = self.reply.calls[..at].iter().any(|call| call.id == *id);
            if id.is_empty() || repeated {
                self.reply.calls[at].id = format!("call_{at}");
            }
        }
        Ok(self.reply)
    }
}

impl Model for HttpModel {
    fn complete(
        &mut self,
        messages: &[Message],
        tools: &[Tool],
        text: &mut dyn FnMut(&str),
        cancel: &AtomicBool,
    ) -> Result<Reply, ModelError> {
        if cancel.load(Ordering::Relaxed) {
            return Err(ModelError::Cancelled);
        }
        let body = serde_json::to_vec(&Request {
            model: self.endpoint.model(),
            messages,
            tools,
            stream: true,
        })
        .map_err(|error| failed(error.to_string()))?;
        let mut request = self
            .agent
            .post(self.endpoint.completions())
            .set("content-type", "application/json")
            .set("accept", "text/event-stream");
        if let Some(key) = self.endpoint.key() {
            request = request.set("authorization", &format!("Bearer {}", key.expose()));
        }
        let (send, pieces) = mpsc::sync_channel::<Piece>(32);
        let endpoint = self.endpoint.clone();
        thread::Builder::new()
            .name("vkore-agent-reply".into())
            .spawn(move || {
                let response = match request.send_bytes(&body) {
                    Ok(response) if response.status() == 200 => response,
                    Ok(response) => {
                        let status = response.status();
                        let why = error_message(&endpoint, status, response);
                        let _ = send.send(Piece::Failed(why));
                        return;
                    }
                    Err(ureq::Error::Status(status, response)) => {
                        let why = error_message(&endpoint, status, response);
                        let _ = send.send(Piece::Failed(why));
                        return;
                    }
                    Err(error) => {
                        let why = format!(
                            "could not reach {}: {}",
                            endpoint.host(),
                            scrub(&endpoint, &error.to_string())
                        );
                        let _ = send.send(Piece::Failed(why));
                        return;
                    }
                };
                let mut reader = BufReader::new(response.into_reader());
                let mut total = 0;
                loop {
                    let piece = match read_line(&mut reader) {
                        Ok(Some(line)) => {
                            total += line.len();
                            if total > MAX_STREAM_BYTES {
                                Piece::Failed("the reply is larger than the agent accepts".into())
                            } else {
                                Piece::Line(line)
                            }
                        }
                        Ok(None) => Piece::End,
                        Err(why) => Piece::Failed(scrub(&endpoint, &why)),
                    };
                    let last = !matches!(piece, Piece::Line(_));
                    // A failed send means nobody is waiting any more.
                    if send.send(piece).is_err() || last {
                        return;
                    }
                }
            })
            .map_err(|error| failed(error.to_string()))?;

        let mut gather = Gather::default();
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(ModelError::Cancelled);
            }
            match pieces.recv_timeout(POLL) {
                Ok(Piece::Line(line)) => {
                    gather.line(&line, text).map_err(|error| match error {
                        // An error event is the server's own words.
                        ModelError::Failed(why) => failed(scrub(&self.endpoint, &why)),
                        other => other,
                    })?;
                    if gather.done {
                        return gather.finish();
                    }
                }
                Ok(Piece::End) => return gather.finish(),
                Ok(Piece::Failed(why)) => return Err(failed(why)),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(failed("the reply reader stopped unexpectedly"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Server, sse};

    fn ask(server: &Server) -> (Result<Reply, ModelError>, String) {
        let mut model = HttpModel::new(server.endpoint(Some("sk-test-secret"))).unwrap();
        let mut seen = String::new();
        let reply = model.complete(
            &[Message::user("hello")],
            &crate::tools::definitions(),
            &mut |piece| seen.push_str(piece),
            &AtomicBool::new(false),
        );
        (reply, seen)
    }

    #[test]
    fn text_streams_and_the_request_is_the_agreed_subset() {
        let server = Server::new(vec![sse(&[
            r#"{"choices":[{"index":0,"delta":{"role":"assistant","content":""}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"content":"Hel"}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"content":"lo"},"finish_reason":"stop"}]}"#,
            r#"{"choices":[],"usage":{"total_tokens":3}}"#,
        ])]);
        let (reply, seen) = ask(&server);
        assert_eq!(reply.unwrap().text, "Hello");
        assert_eq!(seen, "Hello");
        let request = server.request(0);
        assert!(request.head.starts_with("POST /v1/chat/completions "));
        assert!(
            request
                .head
                .to_ascii_lowercase()
                .contains("authorization: bearer sk-test-secret")
        );
        let keys: Vec<&String> = request.body.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["messages", "model", "stream", "tools"]);
        assert_eq!(request.body["stream"], true);
    }

    #[test]
    fn tool_calls_are_gathered_from_their_pieces() {
        let server = Server::new(vec![sse(&[
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"a","type":"function","function":{"name":"type_console","arguments":""}}]}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"text\":"}}]}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"id":"b","function":{"name":"read_console","arguments":"{}"}}]}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"ls\"}"}}]},"finish_reason":"tool_calls"}]}"#,
        ])]);
        let calls = ask(&server).0.unwrap().calls;
        assert_eq!(
            calls,
            vec![
                ToolCall::new("a", "type_console", r#"{"text":"ls"}"#),
                ToolCall::new("b", "read_console", "{}"),
            ]
        );
    }

    #[test]
    fn calls_without_indexes_or_ids_are_still_told_apart() {
        let server = Server::new(vec![sse(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"id":"x","function":{"name":"read_console","arguments":"{}"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"id":"y","function":{"name":"machine_status","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#,
        ])]);
        assert_eq!(ask(&server).0.unwrap().calls.len(), 2);
        let server = Server::new(vec![sse(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"read_console","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#,
        ])]);
        assert_eq!(ask(&server).0.unwrap().calls[0].id, "call_0");
    }

    #[test]
    fn an_error_status_is_reported_without_the_key() {
        let server = Server::new(vec![(
            401,
            "application/json",
            r#"{"error":{"message":"bad key sk-test-secret"}}"#.into(),
        )]);
        let error = ask(&server).0.unwrap_err().to_string();
        assert!(
            error.contains("401") && error.contains("bad key [key]"),
            "{error}"
        );
    }

    #[test]
    fn an_error_event_is_reported_without_the_key() {
        let server = Server::new(vec![sse(&[
            r#"{"error":{"message":"quota for sk-test-secret"}}"#,
        ])]);
        let error = ask(&server).0.unwrap_err().to_string();
        assert!(error.contains("quota for [key]"), "{error}");
    }

    #[test]
    fn a_redirect_is_not_followed() {
        let server = Server::new(vec![(302, "text/plain", String::new())]);
        let error = ask(&server).0.unwrap_err().to_string();
        assert!(error.contains("302"), "{error}");
        assert_eq!(server.requests(), 1);
    }

    #[test]
    fn a_stream_that_ends_early_or_lies_is_refused() {
        for events in [
            r#"data: {"choices":[{"delta":{"content":"half"}}]}"#.to_owned() + "\n\n",
            "data: not json\n\n".to_owned(),
            String::new(),
        ] {
            let server = Server::new(vec![(200, "text/event-stream", events)]);
            assert!(ask(&server).0.is_err());
        }
    }

    #[test]
    fn oversized_lines_and_replies_are_refused() {
        let long = format!("data: {}\n\n", "x".repeat(MAX_LINE_BYTES));
        let server = Server::new(vec![(200, "text/event-stream", long)]);
        assert!(
            ask(&server)
                .0
                .unwrap_err()
                .to_string()
                .contains("line longer")
        );

        let piece = format!(
            r#"{{"choices":[{{"delta":{{"content":"{}"}}}}]}}"#,
            "y".repeat(MAX_TEXT_BYTES / 2 + 1)
        );
        let server = Server::new(vec![sse(&[&piece, &piece])]);
        assert!(
            ask(&server)
                .0
                .unwrap_err()
                .to_string()
                .contains("longer than")
        );
    }

    #[test]
    fn stop_takes_effect_while_the_server_is_silent() {
        let server = Server::silent();
        let mut model = HttpModel::new(server.endpoint(None)).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            flag.store(true, Ordering::Relaxed);
        });
        let began = std::time::Instant::now();
        let reply = model.complete(&[Message::user("x")], &[], &mut |_| {}, &cancel);
        assert_eq!(reply.unwrap_err(), ModelError::Cancelled);
        assert!(began.elapsed() < Duration::from_secs(3));
    }
}
