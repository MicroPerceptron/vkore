//! The part of the OpenAI chat protocol the agent speaks.
//!
//! This is the contract with whatever serves the model, so it is kept small:
//! `POST {base}/chat/completions` with `model`, `messages`, `tools` and
//! `stream: true`, answered as server-sent events. Nothing else is sent. Fields
//! a server adds are ignored.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    fn of(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self::of(Role::System, content)
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::of(Role::User, content)
    }

    pub fn assistant(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            tool_calls,
            ..Self::of(Role::Assistant, content)
        }
    }

    pub fn tool(call: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            tool_call_id: Some(call.into()),
            ..Self::of(Role::Tool, content)
        }
    }

    /// The bytes this message adds to a request, near enough to bound history.
    pub fn weight(&self) -> usize {
        self.content.len()
            + self
                .tool_calls
                .iter()
                .map(|call| call.function.name.len() + call.function.arguments.len())
                .sum::<usize>()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: FunctionCall,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FunctionCall {
    pub name: String,
    /// The arguments as the model wrote them: JSON text, not yet trusted.
    pub arguments: String,
}

impl ToolCall {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            kind: "function",
            function: FunctionCall {
                name: name.into(),
                arguments: arguments.into(),
            },
        }
    }
}

/// A function the model may call.
#[derive(Clone, Debug, Serialize)]
pub struct Tool {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: Function,
}

#[derive(Clone, Debug, Serialize)]
pub struct Function {
    pub name: &'static str,
    pub description: &'static str,
    /// A JSON Schema object.
    pub parameters: Value,
}

#[derive(Debug, Serialize)]
pub struct Request<'a> {
    pub model: &'a str,
    pub messages: &'a [Message],
    #[serde(skip_serializing_if = "<[Tool]>::is_empty")]
    pub tools: &'a [Tool],
    pub stream: bool,
}

/// One server-sent event of a streamed reply.
#[derive(Debug, Default, Deserialize)]
pub struct Chunk {
    #[serde(default)]
    pub choices: Vec<ChunkChoice>,
    #[serde(default)]
    pub error: Option<ApiError>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ChunkChoice {
    #[serde(default)]
    pub index: u32,
    #[serde(default)]
    pub delta: Delta,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Delta {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub refusal: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<ToolCallDelta>>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ToolCallDelta {
    #[serde(default)]
    pub index: Option<u32>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub function: Option<FunctionDelta>,
}

#[derive(Debug, Default, Deserialize)]
pub struct FunctionDelta {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ApiError {
    #[serde(default)]
    pub message: String,
}
