//! What the model may ask for, and the host that carries it out.
//!
//! A model's tool call is untrusted text until [`Action::parse`] has turned it
//! into one of these actions. Every tool acts on the one machine the agent was
//! enabled for, so no call names a machine.

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::protocol::{Function, Tool};

/// The most characters the agent may type at once.
pub const MAX_TYPED: usize = 400;
const MAX_DEPTH: usize = 32;
const MAX_NAME_BYTES: usize = 255;

/// A path on the machine's volume: under a terminal's home, or from the root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilePath {
    pub terminal: Option<u8>,
    pub names: Vec<String>,
}

impl std::fmt::Display for FilePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.terminal {
            Some(terminal) => write!(f, "terminal {terminal}: /{}", self.names.join("/")),
            None => write!(f, "volume: /{}", self.names.join("/")),
        }
    }
}

/// One thing the agent can do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    MachineStatus,
    ReadConsole,
    ListFiles(FilePath),
    ReadFile(FilePath),
    StartMachine,
    StopMachine,
    TypeConsole { text: String, enter: bool },
}

/// The console's newest text, and whether it still accepts input.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Terminal {
    pub text: String,
    pub connected: bool,
}

/// One machine, as the front end that owns it can act on it. Errors are words
/// fit to show to the person and to tell the model.
pub trait Host {
    fn status(&mut self) -> Result<String, String>;
    fn console(&mut self) -> Result<Terminal, String>;
    fn list_files(&mut self, at: &FilePath) -> Result<String, String>;
    fn read_file(&mut self, at: &FilePath) -> Result<String, String>;
    fn start(&mut self) -> Result<(), String>;
    fn stop(&mut self) -> Result<(), String>;
    /// Send `input` to the guest's shell exactly as given.
    fn type_console(&mut self, input: &str) -> Result<(), String>;
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoArguments {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathArguments {
    #[serde(default)]
    path: String,
    #[serde(default)]
    terminal: Option<u8>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TypeArguments {
    text: String,
    #[serde(default = "yes")]
    enter: bool,
}

fn yes() -> bool {
    true
}

fn read<T: DeserializeOwned>(arguments: &str) -> Result<T, String> {
    // Some servers send nothing at all for a tool without arguments.
    let text = if arguments.trim().is_empty() {
        "{}"
    } else {
        arguments
    };
    serde_json::from_str(text).map_err(|error| format!("the arguments do not fit: {error}"))
}

fn path(arguments: PathArguments, file: bool) -> Result<FilePath, String> {
    let names: Vec<String> = arguments
        .path
        .split('/')
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect();
    if names.len() > MAX_DEPTH {
        return Err("that path is too deep".into());
    }
    if names.iter().any(|name| {
        name == "."
            || name == ".."
            || name.len() > MAX_NAME_BYTES
            || name.chars().any(char::is_control)
    }) {
        return Err("that path has a name that is not allowed".into());
    }
    if file && names.is_empty() {
        return Err("name a file".into());
    }
    Ok(FilePath {
        terminal: arguments.terminal,
        names,
    })
}

impl Action {
    /// Turn a model's call into an action, or say why it is not one.
    pub fn parse(name: &str, arguments: &str) -> Result<Self, String> {
        let none = |action| read::<NoArguments>(arguments).map(|_| action);
        match name {
            "machine_status" => none(Self::MachineStatus),
            "read_console" => none(Self::ReadConsole),
            "start_machine" => none(Self::StartMachine),
            "stop_machine" => none(Self::StopMachine),
            "list_files" => path(read(arguments)?, false).map(Self::ListFiles),
            "read_file" => path(read(arguments)?, true).map(Self::ReadFile),
            "type_console" => {
                let TypeArguments { text, enter } = read(arguments)?;
                // Printable ASCII only, so what a person approves is exactly what is
                // sent: no control keys, escape sequences or look-alike characters.
                if !text.bytes().all(|byte| (0x20..0x7f).contains(&byte)) {
                    return Err("only printable ASCII can be typed, one line at a time; set enter to press Enter".into());
                }
                if text.len() > MAX_TYPED {
                    return Err(format!("type at most {MAX_TYPED} characters at once"));
                }
                if text.is_empty() && !enter {
                    return Err("there is nothing to type".into());
                }
                Ok(Self::TypeConsole { text, enter })
            }
            _ => Err("there is no such tool".into()),
        }
    }

    /// Whether running it changes the machine.
    pub fn changes_machine(&self) -> bool {
        matches!(
            self,
            Self::StartMachine | Self::StopMachine | Self::TypeConsole { .. }
        )
    }

    /// The tool's name.
    pub fn operation(&self) -> &'static str {
        match self {
            Self::MachineStatus => "machine_status",
            Self::ReadConsole => "read_console",
            Self::ListFiles(_) => "list_files",
            Self::ReadFile(_) => "read_file",
            Self::StartMachine => "start_machine",
            Self::StopMachine => "stop_machine",
            Self::TypeConsole { .. } => "type_console",
        }
    }

    /// A few words for a person.
    pub fn title(&self) -> &'static str {
        match self {
            Self::MachineStatus => "Check the machine",
            Self::ReadConsole => "Read the console",
            Self::ListFiles(_) => "List files",
            Self::ReadFile(_) => "Read a file",
            Self::StartMachine => "Start the machine",
            Self::StopMachine => "Stop the machine",
            Self::TypeConsole { .. } => "Type into the console",
        }
    }

    /// Exactly what it acts on: the text to be typed, or the path.
    pub fn detail(&self) -> String {
        match self {
            Self::ListFiles(at) | Self::ReadFile(at) => at.to_string(),
            Self::TypeConsole { text, enter: true } => format!("{text}⏎"),
            Self::TypeConsole { text, enter: false } => text.clone(),
            _ => String::new(),
        }
    }
}

fn tool(
    name: &'static str,
    description: &'static str,
    properties: Value,
    required: &[&str],
) -> Tool {
    Tool {
        kind: "function",
        function: Function {
            name,
            description,
            parameters: json!({
                "type": "object",
                "properties": properties,
                "required": required,
                "additionalProperties": false,
            }),
        },
    }
}

/// The tools offered to the model.
pub fn definitions() -> Vec<Tool> {
    let place = || {
        json!({
            "path": {"type": "string", "description": "Slash-separated path, such as notes/todo.txt"},
            "terminal": {"type": "integer", "minimum": 0, "maximum": 255, "description": "A terminal's home to start from; omit to start at the volume root"},
        })
    };
    vec![
        tool(
            "machine_status",
            "The machine's state, boot progress, hardware and recent boot events.",
            json!({}),
            &[],
        ),
        tool(
            "read_console",
            "The newest output of the machine's console shell.",
            json!({}),
            &[],
        ),
        tool(
            "type_console",
            "Type one line into the machine's console shell and return what the console shows afterwards. The person must approve it.",
            json!({
                "text": {"type": "string", "description": "Printable ASCII, one line"},
                "enter": {"type": "boolean", "description": "Press Enter afterwards (default true)"},
            }),
            &["text"],
        ),
        tool(
            "list_files",
            "List a folder on the machine's disk. Only while the machine is stopped.",
            place(),
            &[],
        ),
        tool(
            "read_file",
            "Read the beginning of a file on the machine's disk. Only while the machine is stopped.",
            place(),
            &["path"],
        ),
        tool(
            "start_machine",
            "Boot the machine. The person must approve it.",
            json!({}),
            &[],
        ),
        tool(
            "stop_machine",
            "Power the machine off. The person must approve it.",
            json!({}),
            &[],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_offered_tool_parses() {
        for tool in definitions() {
            let arguments = match tool.function.name {
                "type_console" => r#"{"text":"help"}"#,
                "read_file" => r#"{"path":"a"}"#,
                _ => "",
            };
            let action = Action::parse(tool.function.name, arguments).unwrap();
            assert_eq!(action.operation(), tool.function.name);
        }
    }

    #[test]
    fn typed_text_is_printable_ascii_on_one_line() {
        for text in [
            "a\nb",
            "a\rb",
            "\u{1b}[A",
            "\u{3}",
            "tab\there",
            "r\u{202e}m",
            "caf\u{e9}",
            "\u{7f}",
        ] {
            let arguments = json!({ "text": text }).to_string();
            assert!(
                Action::parse("type_console", &arguments).is_err(),
                "{text:?}"
            );
        }
        let long = json!({ "text": "a".repeat(MAX_TYPED + 1) }).to_string();
        assert!(Action::parse("type_console", &long).is_err());
        assert!(Action::parse("type_console", r#"{"text":"","enter":false}"#).is_err());
        assert_eq!(
            Action::parse("type_console", r#"{"text":""}"#)
                .unwrap()
                .detail(),
            "⏎"
        );
    }

    #[test]
    fn arguments_that_were_not_approved_are_not_silently_dropped() {
        assert!(Action::parse("stop_machine", r#"{"force":true}"#).is_err());
        assert!(Action::parse("type_console", r#"{"text":"ls","then":"rm"}"#).is_err());
        assert!(Action::parse("format_disk", "{}").is_err());
        assert!(Action::parse("read_console", "[1]").is_err());
    }

    #[test]
    fn paths_stay_inside_where_they_start() {
        for bad in ["../x", "a/../../b", "./a", ""] {
            let arguments = json!({ "path": bad }).to_string();
            assert!(Action::parse("read_file", &arguments).is_err(), "{bad:?}");
        }
        let listed = Action::parse("list_files", r#"{"path":"/a//b/","terminal":1}"#).unwrap();
        assert_eq!(listed.detail(), "terminal 1: /a/b");
    }
}
