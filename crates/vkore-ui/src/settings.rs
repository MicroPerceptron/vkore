//! Desktop preferences and controls over the runtime's local API.
use std::{
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
};

use iced_core::{Length, alignment};
use iced_widget::{button, column, container, row, text_input};
use vkored::inference::{Server, Unavailable};

use crate::{Message, UiElement, fonts, theme, widgets};

const DEFAULT_PORT: u16 = 8080;

#[derive(Debug, Clone)]
pub enum SettingsMessage {
    Port(String),
    SavePort,
    ToggleApi,
}

/// Editing a port never changes a live listener. `api` owns the running
/// server; `port` is the saved choice for its next start, and `port_input`
/// is the uncommitted field. The runtime still owns listener policy/lifecycle.
pub struct State {
    root: PathBuf,
    port: u16,
    port_input: String,
    api: Option<Server>,
    error: String,
}

impl State {
    pub fn new(root: PathBuf) -> Self {
        let (port, error) = match load_port(&root) {
            Ok(port) => (port, String::new()),
            Err(error) => (
                DEFAULT_PORT,
                format!("Could not load API port; using {DEFAULT_PORT}: {error}"),
            ),
        };
        Self {
            root,
            port,
            port_input: port.to_string(),
            api: None,
            error,
        }
    }

    pub fn update(&mut self, message: SettingsMessage) {
        match message {
            SettingsMessage::Port(value) => {
                // Numeric fields stay bounded and within the bundled font.
                if value.len() <= 5 && value.bytes().all(|byte| byte.is_ascii_digit()) {
                    self.port_input = value;
                    self.error.clear();
                } else {
                    self.error = "Enter a port from 1 to 65535 using digits only".into();
                }
            }
            SettingsMessage::SavePort => {
                self.save_port();
            }
            SettingsMessage::ToggleApi => {
                if self.api.is_some() {
                    self.stop();
                    self.error.clear();
                } else if self.save_port() {
                    match Server::start(
                        SocketAddr::from((Ipv4Addr::LOCALHOST, self.port)),
                        Arc::new(Unavailable),
                    ) {
                        Ok(api) => {
                            self.api = Some(api);
                            self.error.clear();
                        }
                        Err(error) => {
                            self.error =
                                format!("Could not start local API on port {}: {error}", self.port)
                        }
                    }
                }
            }
        }
    }

    fn save_port(&mut self) -> bool {
        let result = parse_port(&self.port_input).and_then(|port| {
            let mut file =
                tempfile::NamedTempFile::new_in(&self.root).map_err(|error| error.to_string())?;
            writeln!(file, "{port}").map_err(|error| error.to_string())?;
            file.as_file()
                .sync_all()
                .map_err(|error| error.to_string())?;
            file.persist(self.root.join("api-port"))
                .map_err(|error| error.error.to_string())?;
            Ok(port)
        });
        match result {
            Ok(port) => {
                self.port = port;
                self.port_input = port.to_string();
                self.error.clear();
                true
            }
            Err(error) => {
                self.error = format!("Port was not saved: {error}");
                false
            }
        }
    }

    pub fn stop(&mut self) {
        // Dropping the runtime owner closes the listener and cancels requests.
        self.api.take();
    }
}

fn parse_port(value: &str) -> Result<u16, String> {
    let text = value.trim();
    if text.is_empty() || text.len() > 5 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("enter a port from 1 to 65535".into());
    }
    text.parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or_else(|| "enter a port from 1 to 65535".into())
}

fn load_port(root: &Path) -> Result<u16, String> {
    let file = match std::fs::File::open(root.join("api-port")) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(DEFAULT_PORT),
        Err(error) => return Err(error.to_string()),
    };
    let mut text = String::new();
    file.take(17)
        .read_to_string(&mut text)
        .map_err(|error| error.to_string())?;
    if text.len() > 16 {
        return Err("saved port exceeds 16 bytes".into());
    }
    parse_port(&text)
}

fn wrap(message: SettingsMessage) -> Message {
    Message::Settings(message)
}

pub fn view(
    appearance: theme::Appearance,
    system_dark: Option<bool>,
    state: &State,
) -> UiElement<'_> {
    widgets::page_frame(
        column![
            widgets::page_header(
                "Settings",
                "Personalize your workspace and connect your tools."
            ),
            iced_widget::responsive(move |size| panels(
                appearance,
                system_dark,
                state,
                size.width >= 1240.
            )),
        ]
        .spacing(theme::SPACE_6),
    )
}

fn panels(
    appearance: theme::Appearance,
    system_dark: Option<bool>,
    state: &State,
    wide: bool,
) -> UiElement<'_> {
    let choice = |label, caption, value| {
        let selected = appearance == value;
        button(
            column![
                crate::visuals::appearance_preview(value),
                row![
                    widgets::body(label, 14., theme::TEXT),
                    iced_widget::space::horizontal(),
                    widgets::mono(if selected { "Selected" } else { "" }, theme::MAGENTA)
                ],
                widgets::body(caption, 12., theme::TEXT_MUTED),
            ]
            .spacing(10),
        )
        .width(Length::Fill)
        .padding(12)
        .style(widgets::button_style(widgets::Button::Row(selected)))
        .on_press(Message::Appearance(value))
    };
    let appearance = container(
        column![
            row![
                crate::visuals::icon(crate::visuals::Icon::Settings, theme::TEXT_SECONDARY, 22.),
                widgets::body("Appearance", 18., theme::TEXT)
            ]
            .spacing(10)
            .align_y(alignment::Vertical::Center),
            widgets::body(
                "Make this workspace feel at home.",
                theme::SIZE_SM,
                theme::TEXT_MUTED
            ),
            row![
                choice("System", "Follow your device", theme::Appearance::System),
                choice("Light", "A brighter workspace", theme::Appearance::Light),
                choice("Dark", "Keep things subdued", theme::Appearance::Dark)
            ]
            .spacing(12),
            widgets::body(
                if appearance == theme::Appearance::System {
                    match system_dark {
                        Some(true) => "Following your system · Dark",
                        Some(false) => "Following your system · Light",
                        None => "No system preference reported · Using dark mode",
                    }
                } else {
                    "Your appearance preference is saved automatically."
                },
                12.,
                theme::TEXT_MUTED
            ),
        ]
        .spacing(16),
    )
    .padding(24)
    .id("settings-appearance")
    .width(Length::Fill)
    .style(widgets::card);

    let address = match &state.api {
        Some(api) => format!("http://{}/v1", api.address),
        None => format!("http://127.0.0.1:{}/v1", state.port),
    };
    let port_status = if parse_port(&state.port_input).ok() != Some(state.port) {
        "Port change not saved. Save it or start the API to apply it.".to_owned()
    } else if state
        .api
        .as_ref()
        .is_some_and(|api| api.address.port() != state.port)
    {
        format!(
            "Port {} saved. Stop and start the API to use it.",
            state.port
        )
    } else {
        format!(
            "Port {} saved for the next launch. The API starts only when requested.",
            state.port
        )
    };
    let mut api = column![
        row![
            crate::visuals::icon(crate::visuals::Icon::Terminal, theme::TEXT_SECONDARY, 22.),
            widgets::body("Local API", 18., theme::TEXT),
            iced_widget::space::horizontal(),
            widgets::badge(
                if state.api.is_some() {
                    "Running"
                } else {
                    "Stopped"
                },
                if state.api.is_some() {
                    theme::SUCCESS
                } else {
                    theme::TEXT_MUTED
                }
            ),
        ]
        .spacing(10)
        .align_y(alignment::Vertical::Center),
        widgets::body(
            "Connect an OpenAI-compatible client on this computer.",
            theme::SIZE_SM,
            theme::TEXT_SECONDARY
        ),
        widgets::label("Connection", theme::TEXT_MUTED),
        container(widgets::data(address, theme::TEXT))
            .padding(14)
            .width(Length::Fill)
            .style(|appearance| theme::panel(appearance, theme::PANEL_FILE, None, 6.)),
        row![
            widgets::body("Port", 14., theme::TEXT_SECONDARY),
            text_input("8080", &state.port_input)
                .on_input(|value| wrap(SettingsMessage::Port(value)))
                .on_submit(wrap(SettingsMessage::SavePort))
                .font(fonts::MONO)
                .size(theme::SIZE_SM)
                .padding([7, 10])
                .width(100)
                .style(widgets::field),
            button(widgets::control("Save port"))
                .height(theme::CONTROL_SM)
                .padding([0, 14])
                .style(widgets::button_style(widgets::Button::Secondary))
                .on_press(wrap(SettingsMessage::SavePort)),
            button(widgets::control(if state.api.is_some() {
                "Stop API"
            } else {
                "Start API"
            }))
            .height(theme::CONTROL_SM)
            .padding([0, 14])
            .style(widgets::button_style(if state.api.is_some() {
                widgets::Button::Danger
            } else {
                widgets::Button::Primary
            }))
            .on_press(wrap(SettingsMessage::ToggleApi)),
        ]
        .spacing(theme::SPACE_2)
        .align_y(alignment::Vertical::Center),
        widgets::data(port_status, theme::TEXT_MUTED),
        widgets::body(
            "The server is local to this computer. Guest inference is not connected yet.",
            theme::SIZE_SM,
            theme::TEXT_MUTED
        ),
    ]
    .spacing(theme::SPACE_4);
    if !state.error.is_empty() {
        api = api.push(widgets::data(state.error.as_str(), theme::ERROR));
    }
    let api = container(api)
        .id("settings-api")
        .padding(theme::SPACE_6)
        .width(Length::Fill)
        .style(widgets::card);

    let content: UiElement<'_> = if wide {
        row![appearance, api].spacing(theme::SPACE_6).into()
    } else {
        column![appearance, api].spacing(theme::SPACE_6).into()
    };
    widgets::scroll(content).height(Length::Fill).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};

    #[test]
    fn invalid_ports_do_not_replace_saved_preferences_or_start_the_api() {
        let root = tempfile::tempdir().unwrap();
        let mut state = State::new(root.path().to_owned());
        assert_eq!(state.port, DEFAULT_PORT);
        state.update(SettingsMessage::Port("9090".into()));
        state.update(SettingsMessage::SavePort);
        for invalid in ["", "0", "65536"] {
            state.update(SettingsMessage::Port(invalid.into()));
            state.update(SettingsMessage::ToggleApi);
            assert!(state.api.is_none());
            assert!(!state.error.is_empty());
            assert_eq!(load_port(root.path()).unwrap(), 9090);
        }
        for invalid in ["123456", "abc", "日本語"] {
            let before = state.port_input.clone();
            state.update(SettingsMessage::Port(invalid.into()));
            assert_eq!(state.port_input, before);
            assert!(!state.error.is_empty());
        }
        std::fs::write(root.path().join("api-port"), "9".repeat(100)).unwrap();
        let state = State::new(root.path().to_owned());
        assert_eq!(state.port, DEFAULT_PORT);
        assert!(!state.error.is_empty());
    }

    #[test]
    fn saved_port_and_active_address_have_separate_lifetimes() {
        let root = tempfile::tempdir().unwrap();
        let reservation = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let first = reservation.local_addr().unwrap();
        let mut state = State::new(root.path().to_owned());
        state.update(SettingsMessage::Port(first.port().to_string()));
        // A busy port is reported, with no apparent running server.
        state.update(SettingsMessage::ToggleApi);
        assert!(state.api.is_none());
        assert!(state.error.contains("Could not start"));
        drop(reservation);
        state.update(SettingsMessage::ToggleApi);
        assert_eq!(state.api.as_ref().unwrap().address, first);
        assert!(TcpStream::connect(first).is_ok());
        let next = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let next_address = next.local_addr().unwrap();
        state.update(SettingsMessage::Port(next_address.port().to_string()));
        state.update(SettingsMessage::SavePort);
        assert_eq!(state.api.as_ref().unwrap().address, first);
        assert_eq!(load_port(root.path()).unwrap(), next_address.port());
        state.update(SettingsMessage::ToggleApi);
        assert!(TcpStream::connect(first).is_err());
        drop(next);
        state.update(SettingsMessage::ToggleApi);
        assert_eq!(state.api.as_ref().unwrap().address, next_address);
        let reopened = State::new(root.path().to_owned());
        assert_eq!(reopened.port, next_address.port());
        assert!(reopened.api.is_none());
        drop(state);
        assert!(TcpStream::connect(next_address).is_err());
    }
}
