//! Loopback-only live inspector for model, validation, and authority events.
//!
//! The daemon event loop owns this state directly. It is a bounded inert
//! projection and never stores a capability, host fd, guest handle, or path
//! resolver. No lock or observer thread can become an authority side channel.

use crate::{AuthorityObservation, BackendRole, InferenceObservation, ObservationKind};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};

const EVENTS_MAX: usize = 4096;
const ACCEPTS_PER_POLL: usize = 8;
const REQUEST_MAX: usize = 8192;

#[derive(Debug)]
pub enum LiveUiError {
    Io(io::Error),
    NonLoopback,
}

impl From<io::Error> for LiveUiError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

pub struct LiveUi {
    listener: TcpListener,
    events: VecDeque<Value>,
    next_sequence: u64,
}

impl LiveUi {
    pub fn bind(address: SocketAddr) -> Result<Self, LiveUiError> {
        if !address.ip().is_loopback() {
            return Err(LiveUiError::NonLoopback);
        }
        let listener = TcpListener::bind(address)?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            events: VecDeque::with_capacity(EVENTS_MAX),
            next_sequence: 1,
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    pub fn record(&mut self, observation: InferenceObservation) {
        let sequence = self.next_sequence;
        let Some(next) = sequence.checked_add(1) else {
            return;
        };
        self.next_sequence = next;
        let role = match observation.role {
            BackendRole::Broker => "broker",
            BackendRole::Agent => "agent",
            BackendRole::Workspace => "workspace",
        };
        let kind = match observation.kind {
            ObservationKind::Input => "input",
            ObservationKind::ModelPrompt => "model_prompt",
            ObservationKind::ModelOutputChunk => "model_output_chunk",
            ObservationKind::RawModelCompletion => "raw_model_completion",
            ObservationKind::ReturnedModelFields => "returned_model_fields",
            ObservationKind::Validation => "validation",
            ObservationKind::AssistantMessage => "assistant_message",
            ObservationKind::ToolCall => "tool_call",
            ObservationKind::ToolResult => "tool_result",
            ObservationKind::BrokerProposal => "broker_proposal",
        };
        if self.events.len() == EVENTS_MAX {
            self.events.pop_front();
        }
        self.events.push_back(json!({
            "sequence": sequence,
            "role": role,
            "route": hex(&observation.route.0),
            "generation": observation.generation.get(),
            "request": hex(&observation.request.0),
            "turn": observation.turn,
            "kind": kind,
            "payload": observation.payload,
            "accepted": observation.accepted,
            "detail": observation.detail,
        }));
    }

    /// Append a resident-broker attestation already validated on the
    /// structurally broker-only HostLink route. The record is display data;
    /// it cannot drive authority in either direction.
    pub fn record_authority(&mut self, event: Value) {
        let sequence = self.next_sequence;
        let Some(next) = sequence.checked_add(1) else {
            return;
        };
        self.next_sequence = next;
        if self.events.len() == EVENTS_MAX {
            self.events.pop_front();
        }
        self.events.push_back(json!({
            "sequence": sequence,
            "role": "resident_broker",
            "kind": "authority_state",
            "payload": event,
            "accepted": true,
            "detail": "reported after concrete authority transaction commit",
        }));
    }

    pub fn record_authority_observation(&mut self, event: AuthorityObservation) {
        let state = match event.state {
            kore_hostlink::AuthorityState::Installed => "installed",
            kore_hostlink::AuthorityState::Revoked => "revoked",
            kore_hostlink::AuthorityState::Teardown => "teardown",
        };
        let kind = match event.kind {
            kore_hostlink::AuthorityKind::Object => "object",
            kore_hostlink::AuthorityKind::View => "view",
            kore_hostlink::AuthorityKind::Executable => "executable",
            kore_hostlink::AuthorityKind::Budget => "budget",
            kore_hostlink::AuthorityKind::Agent => "agent",
        };
        self.record_authority(json!({
            "state": state,
            "kind": kind,
            "agent_slot": event.agent_slot,
            "generation": event.generation,
            "rights": event.rights,
            "label": event.label,
        }));
    }

    pub fn poll(&mut self) -> Result<(), LiveUiError> {
        for _ in 0..ACCEPTS_PER_POLL {
            let (mut stream, _) = match self.listener.accept() {
                Ok(pair) => pair,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error.into()),
            };
            stream.set_nonblocking(false)?;
            stream.set_read_timeout(Some(std::time::Duration::from_millis(50)))?;
            let _ = self.serve(&mut stream);
        }
        Ok(())
    }

    fn serve(&self, stream: &mut TcpStream) -> io::Result<()> {
        let mut request = [0u8; REQUEST_MAX];
        let read = stream.read(&mut request)?;
        let first = request[..read]
            .split(|byte| *byte == b'\n')
            .next()
            .unwrap_or_default();
        let (status, content_type, body) = if first.starts_with(b"GET /api/events ") {
            (
                "200 OK",
                "application/json; charset=utf-8",
                serde_json::to_vec(&self.events).unwrap_or_else(|_| b"[]".to_vec()),
            )
        } else if first.starts_with(b"GET / ") {
            (
                "200 OK",
                "application/json; charset=utf-8",
                br#"{"client":"vkore-ui","renderer":"axiom","message":"Use the native vKore UI for live inspection."}"#.to_vec(),
            )
        } else {
            (
                "404 Not Found",
                "text/plain; charset=utf-8",
                b"not found".to_vec(),
            )
        };
        let header = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(header.as_bytes())?;
        stream.write_all(&body)
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_non_loopback_and_bounds_events() {
        assert!(matches!(
            LiveUi::bind("0.0.0.0:0".parse().unwrap()),
            Err(LiveUiError::NonLoopback)
        ));
        let mut ui = LiveUi::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        for index in 0..EVENTS_MAX + 3 {
            ui.record_authority(json!({"index": index}));
        }
        assert_eq!(ui.events.len(), EVENTS_MAX);
        assert_eq!(ui.events.front().unwrap()["payload"]["index"], 3);
    }

    #[test]
    fn serves_native_client_discovery_and_json_events() {
        let mut ui = LiveUi::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        ui.record_authority(json!({
            "state": "installed",
            "kind": "object",
            "agent_slot": 1,
            "generation": 4,
            "rights": 1,
            "label": "workspace/docs-architecture.md",
        }));
        let address = ui.local_addr().unwrap();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_polling = stop.clone();
        let poller = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !stop_polling.load(std::sync::atomic::Ordering::Relaxed)
                && std::time::Instant::now() < deadline
            {
                ui.poll().unwrap();
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        });
        let mut client = TcpStream::connect(address).unwrap();
        client.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
        client
            .write_all(b"GET /api/events HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("workspace/docs-architecture.md"));
        assert!(response.contains("authority_state"));
        let mut discovery = TcpStream::connect(address).unwrap();
        discovery.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
        discovery
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        discovery.read_to_string(&mut response).unwrap();
        assert!(response.contains("application/json"));
        assert!(response.contains("vkore-ui"));
        assert!(response.contains("axiom"));
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        poller.join().unwrap();
    }
}
