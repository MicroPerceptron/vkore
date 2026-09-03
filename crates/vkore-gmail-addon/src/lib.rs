use base64::Engine;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::time::Duration;
use vkore_addon_sdk::{
    ADDON_MANIFEST_VERSION, AddonEntrypoint, AddonManifest, ExecutionDomain,
    INVOCATION_PROTOCOL_VERSION, InvocationApproval, MediationMode, ResourceEffect,
    ResultSensitivity, RiskClass, ServiceBroker, ServiceBrokerError, ServiceMethod, ServiceRequest,
    ServiceResponse, ToolManifest,
};
use zeroize::Zeroizing;

pub const CONNECTION_KIND: &str = "google.gmail.oauth";
pub const DESTINATION: &str = "gmail.googleapis.com:443";
pub const SCOPE_READONLY: &str = "https://www.googleapis.com/auth/gmail.readonly";
pub const SCOPE_SEND: &str = "https://www.googleapis.com/auth/gmail.send";
pub const SCOPE_COMPOSE: &str = "https://www.googleapis.com/auth/gmail.compose";
pub const SCOPE_MODIFY: &str = "https://www.googleapis.com/auth/gmail.modify";
pub const SCOPE_FULL: &str = "https://mail.google.com/";

pub fn manifest(package_hash: impl Into<String>) -> AddonManifest {
    AddonManifest {
        manifest_version: ADDON_MANIFEST_VERSION,
        id: "gmail".into(),
        name: "Gmail".into(),
        version: "0.1.0".into(),
        package_hash: package_hash.into(),
        sdk_protocol: INVOCATION_PROTOCOL_VERSION,
        entrypoint: AddonEntrypoint::BrokeredProcess {
            executable: executable_name(),
            arguments: Vec::new(),
        },
        tools: vec![
            tool(
                "search",
                "Search the connected Gmail mailbox and return bounded message identifiers.",
                json!({"type":"object","properties":{"query":{"type":"string"},"max_results":{"type":"integer"},"include_spam_trash":{"type":"boolean"}},"required":["query"],"additionalProperties":false}),
                "search",
                RiskClass::ReadOnly,
                MediationMode::PerCall,
                ResultSensitivity::Sensitive,
            ),
            tool(
                "read",
                "Read one exact Gmail message by identifier.",
                json!({"type":"object","properties":{"message_id":{"type":"string"},"format":{"type":"string"}},"required":["message_id"],"additionalProperties":false}),
                "read",
                RiskClass::ReadOnly,
                MediationMode::PerCall,
                ResultSensitivity::Secret,
            ),
            tool(
                "send",
                "Send one plain-text email from the connected Gmail account.",
                json!({"type":"object","properties":{"to":{"type":"array"},"cc":{"type":"array"},"subject":{"type":"string"},"body":{"type":"string"}},"required":["to","subject","body"],"additionalProperties":false}),
                "send",
                RiskClass::ExternalWrite,
                MediationMode::UserConfirm,
                ResultSensitivity::Sensitive,
            ),
            tool(
                "create_draft",
                "Create one plain-text draft in the connected Gmail account.",
                json!({"type":"object","properties":{"to":{"type":"array"},"cc":{"type":"array"},"subject":{"type":"string"},"body":{"type":"string"}},"required":["to","subject","body"],"additionalProperties":false}),
                "create_draft",
                RiskClass::ExternalWrite,
                MediationMode::UserConfirm,
                ResultSensitivity::Sensitive,
            ),
            tool(
                "trash",
                "Move one exact Gmail message to trash.",
                json!({"type":"object","properties":{"message_id":{"type":"string"}},"required":["message_id"],"additionalProperties":false}),
                "trash",
                RiskClass::Destructive,
                MediationMode::UserConfirm,
                ResultSensitivity::Sensitive,
            ),
        ],
    }
}

fn tool(
    name: &str,
    description: &str,
    input_schema: Value,
    operation: &str,
    risk: RiskClass,
    minimum_mediation: MediationMode,
    sensitivity: ResultSensitivity,
) -> ToolManifest {
    ToolManifest {
        name: name.into(),
        description: description.into(),
        domain: ExecutionDomain::Host,
        input_schema,
        result_schema: json!({"type":"object","properties":{"status":{"type":"integer"},"response":{"type":"object"}}}),
        effects: vec![ResourceEffect {
            resource_type: "gmail_account".into(),
            operation: operation.into(),
            external: !matches!(risk, RiskClass::ReadOnly),
            destructive: matches!(risk, RiskClass::Destructive),
        }],
        risk,
        minimum_mediation,
        required_connections: [CONNECTION_KIND.to_owned()].into_iter().collect(),
        network_destinations: [DESTINATION.to_owned()].into_iter().collect(),
        result_sensitivity: sensitivity,
        timeout_ms: 15_000,
        maximum_input_bytes: 65_536,
        maximum_output_bytes: 512 * 1024,
    }
}

pub fn request(approval: &InvocationApproval) -> Result<ServiceRequest, String> {
    let invocation = &approval.invocation;
    let connection = invocation
        .resources
        .get("connection")
        .filter(|value| !value.is_empty())
        .cloned()
        .ok_or_else(|| "a Gmail connection resource is required".to_owned())?;
    let (method, path, query, body) = match invocation.tool.as_str() {
        "search" => search_request(&invocation.arguments)?,
        "read" => read_request(&invocation.arguments)?,
        "send" => message_request(&invocation.arguments, false)?,
        "create_draft" => message_request(&invocation.arguments, true)?,
        "trash" => trash_request(&invocation.arguments)?,
        _ => return Err("unknown Gmail tool".into()),
    };
    Ok(ServiceRequest {
        protocol_version: INVOCATION_PROTOCOL_VERSION,
        invocation_id: invocation.invocation_id.clone(),
        agent: invocation.agent.clone(),
        package_hash: invocation.package_hash.clone(),
        tool: invocation.tool.clone(),
        connection,
        connection_kind: CONNECTION_KIND.into(),
        destination: DESTINATION.into(),
        method,
        path,
        query,
        body,
    })
}

type RequestParts = (
    ServiceMethod,
    String,
    BTreeMap<String, String>,
    Option<Value>,
);

fn search_request(arguments: &BTreeMap<String, Value>) -> Result<RequestParts, String> {
    let query = string_arg(arguments, "query", 4_096)?;
    let maximum = arguments
        .get("max_results")
        .map(|value| value.as_u64().ok_or("max_results must be an integer"))
        .transpose()?
        .unwrap_or(25);
    if !(1..=100).contains(&maximum) {
        return Err("max_results must be between 1 and 100".into());
    }
    let include_spam = arguments
        .get("include_spam_trash")
        .map(|value| value.as_bool().ok_or("include_spam_trash must be boolean"))
        .transpose()?
        .unwrap_or(false);
    Ok((
        ServiceMethod::Get,
        "/gmail/v1/users/me/messages".into(),
        [
            ("q".into(), query.into()),
            ("maxResults".into(), maximum.to_string()),
            ("includeSpamTrash".into(), include_spam.to_string()),
        ]
        .into_iter()
        .collect(),
        None,
    ))
}

fn read_request(arguments: &BTreeMap<String, Value>) -> Result<RequestParts, String> {
    let message_id = identifier_arg(arguments, "message_id")?;
    let format = arguments
        .get("format")
        .map(|value| value.as_str().ok_or("format must be a string"))
        .transpose()?
        .unwrap_or("full");
    if !matches!(format, "full" | "metadata" | "minimal" | "raw") {
        return Err("unsupported Gmail message format".into());
    }
    Ok((
        ServiceMethod::Get,
        format!("/gmail/v1/users/me/messages/{message_id}"),
        [("format".into(), format.into())].into_iter().collect(),
        None,
    ))
}

fn message_request(
    arguments: &BTreeMap<String, Value>,
    draft: bool,
) -> Result<RequestParts, String> {
    let to = address_list(arguments, "to", true)?;
    let cc = address_list(arguments, "cc", false)?;
    let subject = string_arg(arguments, "subject", 998)?;
    if subject.contains(['\r', '\n']) {
        return Err("subject contains a header break".into());
    }
    let body = string_arg(arguments, "body", 48 * 1024)?;
    let mut message = format!(
        "To: {}\r\nSubject: {}\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=UTF-8\r\nContent-Transfer-Encoding: 8bit\r\n",
        to.join(", "),
        subject
    );
    if !cc.is_empty() {
        message.push_str(&format!("Cc: {}\r\n", cc.join(", ")));
    }
    message.push_str("\r\n");
    message.push_str(body);
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(message.as_bytes());
    let (path, body) = if draft {
        ("/gmail/v1/users/me/drafts", json!({"message":{"raw":raw}}))
    } else {
        ("/gmail/v1/users/me/messages/send", json!({"raw":raw}))
    };
    Ok((
        ServiceMethod::Post,
        path.into(),
        BTreeMap::new(),
        Some(body),
    ))
}

fn trash_request(arguments: &BTreeMap<String, Value>) -> Result<RequestParts, String> {
    let message_id = identifier_arg(arguments, "message_id")?;
    Ok((
        ServiceMethod::Post,
        format!("/gmail/v1/users/me/messages/{message_id}/trash"),
        BTreeMap::new(),
        None,
    ))
}

fn string_arg<'a>(
    arguments: &'a BTreeMap<String, Value>,
    name: &str,
    maximum: usize,
) -> Result<&'a str, String> {
    let value = arguments
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{name} must be a string"))?;
    if value.len() > maximum {
        return Err(format!("{name} is too large"));
    }
    Ok(value)
}

fn identifier_arg(arguments: &BTreeMap<String, Value>, name: &str) -> Result<String, String> {
    let value = string_arg(arguments, name, 256)?;
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(format!("{name} is invalid"));
    }
    Ok(value.into())
}

fn address_list(
    arguments: &BTreeMap<String, Value>,
    name: &str,
    required: bool,
) -> Result<Vec<String>, String> {
    let Some(values) = arguments.get(name) else {
        return if required {
            Err(format!("{name} is required"))
        } else {
            Ok(Vec::new())
        };
    };
    let values = values
        .as_array()
        .ok_or_else(|| format!("{name} must be an array"))?;
    if values.len() > 20 || (required && values.is_empty()) {
        return Err(format!("{name} has an invalid recipient count"));
    }
    values
        .iter()
        .map(|value| {
            let address = value
                .as_str()
                .ok_or_else(|| format!("{name} contains a non-string"))?;
            let mut parts = address.split('@');
            if address.len() > 320
                || address.contains(['\r', '\n', ' ', ',', '<', '>'])
                || parts.next().is_none_or(str::is_empty)
                || parts.next().is_none_or(str::is_empty)
                || parts.next().is_some()
            {
                return Err(format!("{name} contains an invalid address"));
            }
            Ok(address.to_owned())
        })
        .collect()
}

struct GmailConnection {
    token: Zeroizing<String>,
    scopes: BTreeSet<String>,
}

/// In-memory Gmail OAuth broker. Tokens are attached only in this trusted
/// boundary, are zeroized on drop, and are never serialized into an add-on call.
pub struct GmailCredentialBroker {
    agent: ureq::Agent,
    connections: BTreeMap<String, GmailConnection>,
}

impl Default for GmailCredentialBroker {
    fn default() -> Self {
        Self::new(Duration::from_secs(15))
    }
}

impl GmailCredentialBroker {
    pub fn new(timeout: Duration) -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .redirects(0)
                .timeout(timeout)
                .build(),
            connections: BTreeMap::new(),
        }
    }

    pub fn connect(
        &mut self,
        id: impl Into<String>,
        access_token: impl Into<String>,
        scopes: impl IntoIterator<Item = String>,
    ) -> Result<(), ServiceBrokerError> {
        let id = id.into();
        let access_token = access_token.into();
        let scopes: BTreeSet<String> = scopes.into_iter().collect();
        if id.is_empty() || access_token.is_empty() || scopes.is_empty() {
            return Err(ServiceBrokerError::InvalidRequest(
                "connection id, token, and scopes are required".into(),
            ));
        }
        self.connections.insert(
            id,
            GmailConnection {
                token: Zeroizing::new(access_token),
                scopes,
            },
        );
        Ok(())
    }

    pub fn disconnect(&mut self, id: &str) -> bool {
        self.connections.remove(id).is_some()
    }
}

impl ServiceBroker for GmailCredentialBroker {
    fn execute(
        &self,
        request: &ServiceRequest,
        maximum_response_bytes: usize,
    ) -> Result<ServiceResponse, ServiceBrokerError> {
        request.validate_shape()?;
        if request.destination != DESTINATION || request.connection_kind != CONNECTION_KIND {
            return Err(ServiceBrokerError::DestinationDenied);
        }
        validate_gmail_request(request)?;
        let connection = self
            .connections
            .get(&request.connection)
            .ok_or(ServiceBrokerError::UnknownConnection)?;
        if !scope_allows(&connection.scopes, request) {
            return Err(ServiceBrokerError::InvalidRequest(
                "connection lacks the required Gmail OAuth scope".into(),
            ));
        }
        let url = format!("https://{}{}", request.destination, request.path);
        let authorization = format!("Bearer {}", connection.token.as_str());
        let mut call = self
            .agent
            .request(request.method.as_str(), &url)
            .set("Authorization", &authorization)
            .set("Accept", "application/json");
        for (name, value) in &request.query {
            call = call.query(name, value);
        }
        let response = match &request.body {
            Some(body) => {
                let encoded = serde_json::to_vec(body)
                    .map_err(|error| ServiceBrokerError::InvalidRequest(error.to_string()))?;
                call.set("Content-Type", "application/json")
                    .send_bytes(&encoded)
            }
            None => call.call(),
        };
        let response = match response {
            Ok(response) => response,
            Err(ureq::Error::Status(_, response)) => response,
            Err(ureq::Error::Transport(error)) => {
                return Err(ServiceBrokerError::Transport(error.to_string()));
            }
        };
        let status = response.status();
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take((maximum_response_bytes + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| ServiceBrokerError::Transport(error.to_string()))?;
        if bytes.len() > maximum_response_bytes {
            return Err(ServiceBrokerError::ResponseTooLarge);
        }
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .map_err(|error| ServiceBrokerError::Transport(error.to_string()))?
        };
        Ok(ServiceResponse { status, body })
    }
}

fn scope_allows(scopes: &BTreeSet<String>, request: &ServiceRequest) -> bool {
    if scopes.contains(SCOPE_FULL) {
        return true;
    }
    match request.tool.as_str() {
        "search" | "read" => scopes.contains(SCOPE_READONLY) || scopes.contains(SCOPE_MODIFY),
        "send" => {
            scopes.contains(SCOPE_SEND)
                || scopes.contains(SCOPE_COMPOSE)
                || scopes.contains(SCOPE_MODIFY)
        }
        "create_draft" => scopes.contains(SCOPE_COMPOSE) || scopes.contains(SCOPE_MODIFY),
        "trash" => scopes.contains(SCOPE_MODIFY),
        _ => false,
    }
}

fn validate_gmail_request(request: &ServiceRequest) -> Result<(), ServiceBrokerError> {
    let invalid = || {
        ServiceBrokerError::InvalidRequest("request does not match the declared Gmail tool".into())
    };
    let valid = match request.tool.as_str() {
        "search" => {
            request.method == ServiceMethod::Get
                && request.path == "/gmail/v1/users/me/messages"
                && request.body.is_none()
                && request
                    .query
                    .keys()
                    .all(|key| matches!(key.as_str(), "q" | "maxResults" | "includeSpamTrash"))
                && request.query.contains_key("q")
                && request
                    .query
                    .get("maxResults")
                    .and_then(|value| value.parse::<u64>().ok())
                    .is_some_and(|value| (1..=100).contains(&value))
        }
        "read" => {
            request.method == ServiceMethod::Get
                && request.body.is_none()
                && request.query.keys().all(|key| key == "format")
                && request.query.get("format").is_some_and(|format| {
                    matches!(format.as_str(), "full" | "metadata" | "minimal" | "raw")
                })
                && request
                    .path
                    .strip_prefix("/gmail/v1/users/me/messages/")
                    .is_some_and(valid_identifier)
        }
        "send" => {
            request.method == ServiceMethod::Post
                && request.path == "/gmail/v1/users/me/messages/send"
                && request.query.is_empty()
                && request.body.as_ref().is_some_and(valid_raw_message)
        }
        "create_draft" => {
            request.method == ServiceMethod::Post
                && request.path == "/gmail/v1/users/me/drafts"
                && request.query.is_empty()
                && request
                    .body
                    .as_ref()
                    .and_then(|body| body.get("message"))
                    .is_some_and(valid_raw_message)
        }
        "trash" => {
            request.method == ServiceMethod::Post
                && request.query.is_empty()
                && request.body.is_none()
                && request
                    .path
                    .strip_prefix("/gmail/v1/users/me/messages/")
                    .and_then(|suffix| suffix.strip_suffix("/trash"))
                    .is_some_and(valid_identifier)
        }
        _ => false,
    };
    if valid { Ok(()) } else { Err(invalid()) }
}

fn valid_raw_message(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object.len() == 1
        && object
            .get("raw")
            .and_then(Value::as_str)
            .is_some_and(|raw| raw.len() <= 96 * 1024)
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn executable_name() -> String {
    if cfg!(windows) {
        "vkore-gmail-addon.exe".into()
    } else {
        "vkore-gmail-addon".into()
    }
}
