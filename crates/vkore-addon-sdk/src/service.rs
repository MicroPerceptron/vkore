use crate::{AgentRef, INVOCATION_PROTOCOL_VERSION};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum ServiceMethod {
    Get,
    Post,
}

impl ServiceMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }
}

/// A credential-free request plan emitted by a brokered add-on process.
///
/// It deliberately cannot carry arbitrary URLs, headers, or credentials.
/// vKore binds `connection` to a live credential, checks `destination` against
/// the tool manifest and runtime policy, and constructs the HTTPS request.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ServiceRequest {
    pub protocol_version: u16,
    pub invocation_id: String,
    pub agent: AgentRef,
    pub package_hash: String,
    pub tool: String,
    pub connection: String,
    pub connection_kind: String,
    pub destination: String,
    pub method: ServiceMethod,
    pub path: String,
    #[serde(default)]
    pub query: BTreeMap<String, String>,
    pub body: Option<Value>,
}

impl ServiceRequest {
    pub fn validate_shape(&self) -> Result<(), ServiceBrokerError> {
        if self.protocol_version != INVOCATION_PROTOCOL_VERSION {
            return Err(ServiceBrokerError::InvalidRequest("protocol".into()));
        }
        if self.connection.is_empty()
            || self.connection_kind.is_empty()
            || self.destination.is_empty()
            || self.path.is_empty()
            || !self.path.starts_with('/')
            || self.path.contains("://")
            || self.path.contains('?')
            || self.path.contains('#')
            || self.path.split('/').any(|component| component == "..")
            || self.path.bytes().any(|byte| matches!(byte, b'\r' | b'\n'))
            || self.destination.contains('/')
            || self
                .destination
                .bytes()
                .any(|byte| matches!(byte, b'\r' | b'\n'))
        {
            return Err(ServiceBrokerError::InvalidRequest("target".into()));
        }
        if self.query.iter().any(|(name, value)| {
            name.is_empty()
                || name
                    .bytes()
                    .chain(value.bytes())
                    .any(|byte| matches!(byte, b'\r' | b'\n'))
        }) {
            return Err(ServiceBrokerError::InvalidRequest("query".into()));
        }
        if self.method == ServiceMethod::Get && self.body.is_some() {
            return Err(ServiceBrokerError::InvalidRequest("GET body".into()));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ServiceResponse {
    pub status: u16,
    pub body: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServiceBrokerError {
    UnknownConnection,
    DestinationDenied,
    InvalidRequest(String),
    ResponseTooLarge,
    Transport(String),
}

impl fmt::Display for ServiceBrokerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for ServiceBrokerError {}

/// Trusted vKore boundary which attaches credentials and performs a service call.
/// Add-on processes receive neither this object nor the credentials it owns.
pub trait ServiceBroker {
    fn execute(
        &self,
        request: &ServiceRequest,
        maximum_response_bytes: usize,
    ) -> Result<ServiceResponse, ServiceBrokerError>;
}
