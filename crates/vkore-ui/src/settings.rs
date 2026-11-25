//! Desktop preferences and controls over the runtime's local API.
use crate::typography::{FontPreset, TextSize, Typography, font_preset_label, text_size_label};
use std::{
    net::{Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::Arc,
};

use iced_core::{Length, alignment};
use iced_widget::{button, column, container, row, text_input};
use vkored::{
    account::CredentialPersistence,
    inference::{Server, Unavailable},
    state::desktop::{Appearance, DATABASE_NAME, Preference, Preferences, Store, parse_port},
};

use crate::{Message, Page, UiElement, account, fonts, theme, visuals, widgets};

#[derive(Debug, Clone)]
pub enum SettingsMessage {
    Port(String),
    SavePort,
    ToggleApi,
    TextSize(TextSize),
    Font(FontPreset),
    ResetTypography,
}

/// Editing a port never changes a live listener. `api` owns the running
/// server; `port` is the saved choice for its next start, and `port_input`
/// is the uncommitted field. The runtime still owns listener policy/lifecycle.
pub struct State {
    pub account: crate::account::State,
    pub typography: Typography,
    pub saved_appearance: Appearance,
    typography_error: String,
    store: Option<Store>,
    storage_warnings: Vec<String>,
    root: PathBuf,
    port: u16,
    port_input: String,
    api: Option<Server>,
    error: String,
}

impl State {
    pub fn new(root: PathBuf) -> Self {
        let (store, preferences, storage_warnings) = match Store::open(&root) {
            Ok((store, preferences, warnings)) => (Some(store), preferences, warnings),
            Err(error) => (
                None,
                Preferences::default(),
                vec![format!(
                    "Could not open the state database. Using default preferences. {error}"
                )],
            ),
        };
        Self {
            account: Default::default(),
            typography: Typography {
                text_size: preferences.text_size,
                preset: preferences.font_preset,
            },
            saved_appearance: preferences.appearance,
            typography_error: String::new(),
            store,
            storage_warnings,
            root,
            port: preferences.api_port,
            port_input: preferences.api_port.to_string(),
            api: None,
            error: String::new(),
        }
    }

    pub fn update(&mut self, message: SettingsMessage) {
        match message {
            SettingsMessage::TextSize(size) => {
                self.typography.text_size = size;
                self.save_typography(Preference::TextSize(size));
            }
            SettingsMessage::Font(preset) => {
                self.typography.preset = preset;
                self.save_typography(Preference::Font(preset));
            }
            SettingsMessage::ResetTypography => {
                self.typography = Typography::default();
                self.save_typography(Preference::ResetTypography);
            }
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

    fn save(&self, choice: Preference) -> Result<(), String> {
        self.store
            .as_ref()
            .ok_or("State database is unavailable")?
            .save(choice)
    }

    pub fn save_appearance(&mut self, appearance: Appearance) -> Result<(), String> {
        self.save(Preference::Appearance(appearance))?;
        self.saved_appearance = appearance;
        Ok(())
    }

    fn save_typography(&mut self, choice: Preference) {
        self.typography_error = self
            .save(choice)
            .err()
            .map(|error| {
                format!("Text changed for this session. Could not save preferences. {error}")
            })
            .unwrap_or_default();
    }

    fn save_port(&mut self) -> bool {
        let result = parse_port(&self.port_input).and_then(|port| {
            self.save(Preference::ApiPort(port))?;
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

fn wrap(message: SettingsMessage) -> Message {
    Message::Settings(message)
}

pub fn view(
    appearance: theme::Appearance,
    system_dark: Option<bool>,
    state: &State,
) -> UiElement<'_> {
    let ty = state.typography;
    widgets::page_frame(
        column![
            ty.page_header(
                crate::visuals::Icon::Settings,
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
    let ty = state.typography;
    let choice = |label, caption, value| {
        let selected = appearance == value;
        button(
            column![
                crate::visuals::appearance_preview(value),
                row![
                    ty.body(label, 14., theme::TEXT),
                    iced_widget::space::horizontal(),
                    ty.mono(if selected { "Selected" } else { "" }, theme::MAGENTA)
                ],
                ty.body(caption, 12., theme::TEXT_MUTED),
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
                ty.body("Appearance", 18., theme::TEXT)
            ]
            .spacing(10)
            .align_y(alignment::Vertical::Center),
            ty.body(
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
            typography_controls(state),
            ty.body(
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
            ty.body("Local API", 18., theme::TEXT),
            iced_widget::space::horizontal(),
            ty.badge(
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
        ty.body(
            "Connect an OpenAI-compatible client on this computer.",
            theme::SIZE_SM,
            theme::TEXT_SECONDARY
        ),
        ty.label("Connection", theme::TEXT_MUTED),
        container(ty.data(address, theme::TEXT))
            .padding(14)
            .width(Length::Fill)
            .style(|appearance| theme::panel(appearance, theme::PANEL_FILE, None, 6.)),
        row![
            ty.body("Port", 14., theme::TEXT_SECONDARY),
            text_input("8080", &state.port_input)
                .on_input(|value| wrap(SettingsMessage::Port(value)))
                .on_submit(wrap(SettingsMessage::SavePort))
                .font(fonts::MONO)
                .size(ty.size(theme::SIZE_SM))
                .padding([7, 10])
                .width(100)
                .style(widgets::field),
            button(ty.control("Save port"))
                .height(ty.control_height())
                .padding([0, 14])
                .style(widgets::button_style(widgets::Button::Secondary))
                .on_press(wrap(SettingsMessage::SavePort)),
            button(ty.control(if state.api.is_some() {
                "Stop API"
            } else {
                "Start API"
            }))
            .height(ty.control_height())
            .padding([0, 14])
            .style(widgets::button_style(if state.api.is_some() {
                widgets::Button::Danger
            } else {
                widgets::Button::Primary
            }))
            .on_press(wrap(SettingsMessage::ToggleApi)),
        ]
        .spacing(theme::SPACE_2)
        .align_y(alignment::Vertical::Center)
        .wrap(),
        ty.data(port_status, theme::TEXT_MUTED),
        ty.body(
            "The server is local to this computer. Guest inference is not connected yet.",
            theme::SIZE_SM,
            theme::TEXT_MUTED
        ),
    ]
    .spacing(theme::SPACE_4);
    if !state.error.is_empty() {
        api = api.push(ty.data(state.error.as_str(), theme::ERROR));
    }
    let api = container(api)
        .id("settings-api")
        .padding(theme::SPACE_6)
        .width(Length::Fill)
        .style(widgets::card);

    let device = device_settings(state);
    let content: UiElement<'_> = if wide {
        row![
            appearance,
            column![api, device]
                .spacing(theme::SPACE_6)
                .width(Length::Fill),
        ]
        .spacing(theme::SPACE_6)
        .into()
    } else {
        column![appearance, api, device]
            .spacing(theme::SPACE_6)
            .into()
    };
    widgets::scroll(content).height(Length::Fill).into()
}

fn device_settings(state: &State) -> UiElement<'_> {
    let ty = state.typography;
    let account = &state.account.account;
    let checking = state.account.checking_storage();
    let persistence = account.persistence();
    let (label, tone) = if checking {
        ("Checking…", theme::TEXT_MUTED)
    } else {
        match persistence {
            CredentialPersistence::NotChecked => ("Unchecked", theme::TEXT_MUTED),
            CredentialPersistence::NoSavedSignIn => ("No saved sign-in", theme::TEXT_MUTED),
            CredentialPersistence::Saved => ("Saved locally", theme::SUCCESS),
            CredentialPersistence::Temporary => ("Temporary sign-in", theme::WARNING),
            CredentialPersistence::Expired => ("Sign-in expired", theme::TEXT_MUTED),
            CredentialPersistence::StoreUnavailable => ("Store unavailable", theme::WARNING),
            CredentialPersistence::Invalid => ("Invalid saved sign-in", theme::WARNING),
            CredentialPersistence::SignOutIncomplete => ("Sign-out incomplete", theme::WARNING),
        }
    };
    let mut storage = column![
        row![
            visuals::icon(visuals::Icon::Machine, theme::TEXT_SECONDARY, 22.),
            ty.body("This device", 18., theme::TEXT),
        ]
        .spacing(theme::SPACE_2)
        .align_y(alignment::Vertical::Center),
        ty.label("State directory", theme::TEXT_MUTED),
        container(
            ty.data(state.root.display().to_string(), theme::TEXT)
                .wrapping(iced_core::text::Wrapping::WordOrGlyph),
        )
        .padding(14)
        .width(Length::Fill)
        .style(|appearance| theme::panel(appearance, theme::PANEL_FILE, None, 6.)),
        ty.body(
            format!("Desktop preferences database · {DATABASE_NAME}"),
            theme::SIZE_SM,
            theme::TEXT_MUTED,
        ),
    ]
    .spacing(theme::SPACE_4);
    for warning in &state.storage_warnings {
        storage = storage.push(
            ty.data(warning, theme::WARNING)
                .wrapping(iced_core::text::Wrapping::WordOrGlyph),
        );
    }
    let mut content = storage
        .push(ty.label("Credential store", theme::TEXT_MUTED))
        .push(
            row![
                ty.body(account.storage_provider(), 14., theme::TEXT),
                ty.badge(label, tone),
            ]
            .spacing(theme::SPACE_2)
            .align_y(alignment::Vertical::Center)
            .wrap(),
        );
    if !checking && let Some(warning) = persistence.warning() {
        content = content.push(ty.body(warning, theme::SIZE_SM, theme::WARNING));
    }
    content = content.push(
        row![
            button(ty.control("Refresh status"))
                .height(ty.control_height())
                .padding([0, 14])
                .style(widgets::button_style(widgets::Button::Secondary))
                .on_press_maybe(
                    state
                        .account
                        .can_check_storage()
                        .then_some(Message::Account(account::Action::CheckPersistence),)
                ),
            button(ty.control("Manage account"))
                .height(ty.control_height())
                .padding([0, 14])
                .style(widgets::button_style(widgets::Button::Secondary))
                .on_press(Message::Show(Page::Account)),
        ]
        .spacing(theme::SPACE_2)
        .wrap(),
    );
    container(content)
        .id("settings-device")
        .padding(theme::SPACE_6)
        .width(Length::Fill)
        .style(widgets::card)
        .into()
}

fn typography_controls(state: &State) -> UiElement<'_> {
    let ty = state.typography;
    let mut sizes = row![].spacing(6);
    for size in TextSize::ALL {
        sizes = sizes.push(
            button(ty.body(text_size_label(size), 13., theme::TEXT))
                .padding([8, 10])
                .style(widgets::button_style(widgets::Button::Choice(
                    ty.text_size == size,
                )))
                .on_press(wrap(SettingsMessage::TextSize(size))),
        );
    }
    let mut presets = row![].spacing(6);
    for preset in FontPreset::ALL {
        presets = presets.push(
            button(
                ty.body(font_preset_label(preset), 13., theme::TEXT)
                    .font(Typography { preset, ..ty }.font()),
            )
            .padding([8, 10])
            .style(widgets::button_style(widgets::Button::Choice(
                ty.preset == preset,
            )))
            .on_press(wrap(SettingsMessage::Font(preset))),
        );
    }
    let mut content = column![
        ty.body("Text", 18., theme::TEXT),
        ty.label("Size", theme::TEXT_MUTED), sizes.wrap(),
        ty.label("Interface font", theme::TEXT_MUTED), presets.wrap(),
        container(column![
            ty.body("Your workspace, your way.", 16., theme::TEXT),
            ty.data("Kore / models / inference.log", theme::TEXT_SECONDARY),
        ].spacing(8)).padding(14).width(Length::Fill)
            .style(|appearance| theme::panel(appearance, theme::PANEL_FILE, Some(theme::SEPARATOR), 8.)),
        ty.body("Changes apply immediately and are saved automatically. Page titles keep their display face; paths and logs stay monospace.", 12., theme::TEXT_MUTED),
        button(ty.body("Reset text settings", 13., theme::TEXT)).padding([7, 10])
            .style(widgets::button_style(widgets::Button::Secondary))
            .on_press(wrap(SettingsMessage::ResetTypography)),
    ].spacing(12);
    if !state.typography_error.is_empty() {
        content = content.push(ty.data(state.typography_error.as_str(), theme::ERROR));
    }
    container(content)
        .padding(iced_core::Padding::default().top(12))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{ErrorKind, Read},
        net::{TcpListener, TcpStream},
        time::Duration,
    };
    use vkored::state::desktop::DEFAULT_PORT;

    fn assert_disconnected(mut connection: TcpStream) {
        match connection.read(&mut [0]) {
            Ok(0) => {}
            Err(error) if error.kind() == ErrorKind::ConnectionReset => {}
            other => panic!("the stopped server retained its connection: {other:?}"),
        }
    }

    #[test]
    fn typography_applies_saves_and_resets_without_changing_api_preferences() {
        let root = tempfile::tempdir().unwrap();
        let mut state = State::new(root.path().to_owned());
        state.update(SettingsMessage::TextSize(TextSize::ExtraLarge));
        state.update(SettingsMessage::Font(FontPreset::Sans));
        assert_eq!(state.typography.text_size, TextSize::ExtraLarge);
        assert_eq!(
            State::new(root.path().to_owned()).typography,
            state.typography
        );
        assert_eq!(state.port, DEFAULT_PORT);
        state.update(SettingsMessage::ResetTypography);
        assert_eq!(
            State::new(root.path().to_owned()).typography,
            Typography::default()
        );
        std::fs::remove_dir_all(root.path()).unwrap();
        state.update(SettingsMessage::Font(FontPreset::Mono));
        assert_eq!(state.typography.preset, FontPreset::Mono);
        assert!(state.typography_error.contains("Could not save"));
    }

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
            assert_eq!(State::new(root.path().to_owned()).port, 9090);
        }
        for invalid in ["123456", "abc", "日本語"] {
            let before = state.port_input.clone();
            state.update(SettingsMessage::Port(invalid.into()));
            assert_eq!(state.port_input, before);
            assert!(!state.error.is_empty());
        }
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
        let first_connection = TcpStream::connect(first).unwrap();
        // Configure the socket while it is live: macOS can reject this after reset.
        first_connection
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let next = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let next_address = next.local_addr().unwrap();
        state.update(SettingsMessage::Port(next_address.port().to_string()));
        state.update(SettingsMessage::SavePort);
        assert_eq!(state.api.as_ref().unwrap().address, first);
        assert_eq!(State::new(root.path().to_owned()).port, next_address.port());
        state.update(SettingsMessage::ToggleApi);
        assert!(state.api.is_none());
        assert_disconnected(first_connection);
        drop(next);
        state.update(SettingsMessage::ToggleApi);
        assert_eq!(state.api.as_ref().unwrap().address, next_address);
        let next_connection = TcpStream::connect(next_address).unwrap();
        // Configure the socket while it is live: macOS can reject this after reset.
        next_connection
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let reopened = State::new(root.path().to_owned());
        assert_eq!(reopened.port, next_address.port());
        assert!(reopened.api.is_none());
        drop(state);
        // Check this server's connection. A new connection after Drop could
        // reach another parallel test that has reused the ephemeral port.
        assert_disconnected(next_connection);
    }
}
