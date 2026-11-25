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

use crate::{
    Message, Page, UiElement, account, fonts, kit,
    kit::Status,
    motion::Beat,
    theme,
    visuals::{self, Icon},
    widgets,
};

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
    // A theme to pick: its preview, its name, and a tick when it is the one.
    let choice = |label: &'static str, caption: &'static str, value| {
        let selected = appearance == value;
        let kind = widgets::Button::Row(selected);
        kit::lift(
            kind,
            true,
            button(
                column![
                    crate::visuals::appearance_preview(value),
                    row![
                        ty.body(label, 14., theme::TEXT),
                        iced_widget::space::horizontal(),
                        if selected {
                            visuals::icon(Icon::Check, theme::MAGENTA, ty.size(16.))
                        } else {
                            iced_widget::space::horizontal().width(0).into()
                        },
                    ]
                    .align_y(alignment::Vertical::Center),
                    ty.body(caption, 12., theme::TEXT_MUTED),
                ]
                .spacing(10),
            )
            .width(Length::Fill)
            .padding(12)
            .style(widgets::button_style_fading(kind))
            .on_press(Message::Appearance(value)),
        )
    };
    let appearance_note = if appearance == theme::Appearance::System {
        match system_dark {
            Some(true) => "Following your system · Dark",
            Some(false) => "Following your system · Light",
            None => "No system preference reported · Using dark mode",
        }
    } else {
        "Your appearance preference is saved automatically."
    };
    let appearance = container(
        column![
            kit::lead(
                ty,
                Icon::Dashboard,
                ty.body("Appearance", 20., theme::TEXT),
                Some("Make this workspace feel at home."),
            ),
            row![
                choice("System", "Follow your device", theme::Appearance::System),
                choice("Light", "A brighter workspace", theme::Appearance::Light),
                choice("Dark", "Keep things subdued", theme::Appearance::Dark)
            ]
            .spacing(12),
            kit::note(ty, appearance_note),
            typography_controls(state),
        ]
        .spacing(theme::SPACE_4),
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
    // Running is alive, so the mark pulses; stopped is an outline.
    let running = state.api.is_some();
    let api_state = Status {
        beat: if running { Beat::Pulse } else { Beat::Hollow },
        tone: if running {
            theme::SUCCESS
        } else {
            theme::TEXT_MUTED
        },
        label: if running { "running" } else { "stopped" },
        steps: None,
    };
    let mut api = column![
        row![
            container(kit::lead(
                ty,
                Icon::Terminal,
                ty.body("Local API", 20., theme::TEXT),
                Some("Connect an OpenAI-compatible client on this computer."),
            ))
            .width(Length::Fill),
            kit::status(ty, api_state),
        ]
        .spacing(theme::SPACE_4)
        .align_y(alignment::Vertical::Top),
        container(
            column![
                kit::heading(ty, Icon::Globe, "Connection"),
                ty.data(address, theme::TEXT)
                    .wrapping(iced_core::text::Wrapping::WordOrGlyph),
            ]
            .spacing(theme::SPACE_1 + 2.),
        )
        .padding(theme::SPACE_2 + 4.)
        .width(Length::Fill)
        .style(widgets::well),
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
            kit::icon_label(
                ty,
                Icon::Check,
                "Save port",
                widgets::Button::Secondary,
                Some(wrap(SettingsMessage::SavePort)),
            ),
            if running {
                kit::icon_label(
                    ty,
                    Icon::Stop,
                    "Stop API",
                    widgets::Button::Danger,
                    Some(wrap(SettingsMessage::ToggleApi)),
                )
            } else {
                kit::icon_label(
                    ty,
                    Icon::Play,
                    "Start API",
                    widgets::Button::Primary,
                    Some(wrap(SettingsMessage::ToggleApi)),
                )
            },
        ]
        .spacing(theme::SPACE_2)
        .align_y(alignment::Vertical::Center)
        .wrap(),
        kit::note(ty, port_status),
        kit::note(
            ty,
            "The server is local to this computer. Guest inference is not connected yet.",
        ),
    ]
    .spacing(theme::SPACE_4);
    if !state.error.is_empty() {
        api = api.push(kit::error_data(ty, state.error.as_str()));
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
    // Where the saved sign-in stands, as a glyph and a tone with its words.
    let (label, icon, tone) = if checking {
        ("Checking…", Icon::Info, theme::TEXT_MUTED)
    } else {
        match persistence {
            CredentialPersistence::NotChecked => ("Unchecked", Icon::Info, theme::TEXT_MUTED),
            CredentialPersistence::NoSavedSignIn => {
                ("No saved sign-in", Icon::Info, theme::TEXT_MUTED)
            }
            CredentialPersistence::Saved => ("Saved locally", Icon::Check, theme::SUCCESS),
            CredentialPersistence::Temporary => ("Temporary sign-in", Icon::Alert, theme::WARNING),
            CredentialPersistence::Expired => ("Sign-in expired", Icon::Info, theme::TEXT_MUTED),
            CredentialPersistence::StoreUnavailable => {
                ("Store unavailable", Icon::Alert, theme::WARNING)
            }
            CredentialPersistence::Invalid => {
                ("Invalid saved sign-in", Icon::Alert, theme::WARNING)
            }
            CredentialPersistence::SignOutIncomplete => {
                ("Sign-out incomplete", Icon::Alert, theme::WARNING)
            }
        }
    };
    let mut content = column![
        kit::lead(
            ty,
            Icon::Machine,
            ty.body("This device", 20., theme::TEXT),
            Some("Where vKore keeps your workspace."),
        ),
        container(
            column![
                kit::heading(ty, Icon::Folder, "State directory"),
                ty.data(state.root.display().to_string(), theme::TEXT)
                    .wrapping(iced_core::text::Wrapping::WordOrGlyph),
                kit::figure(
                    ty,
                    Icon::Volume,
                    format!("Preferences database · {DATABASE_NAME}"),
                ),
            ]
            .spacing(theme::SPACE_1 + 2.),
        )
        .padding(theme::SPACE_2 + 4.)
        .width(Length::Fill)
        .style(widgets::well),
    ]
    .spacing(theme::SPACE_4);
    for warning in &state.storage_warnings {
        content = content.push(kit::caution_data(ty, warning.as_str()));
    }
    content = content.push(
        container(
            column![
                kit::heading(ty, Icon::Lock, "Credential store"),
                row![
                    ty.body(account.storage_provider(), 14., theme::TEXT),
                    kit::chip(ty, icon, label, tone),
                ]
                .spacing(theme::SPACE_2)
                .align_y(alignment::Vertical::Center)
                .wrap(),
            ]
            .spacing(theme::SPACE_1 + 2.),
        )
        .padding(theme::SPACE_2 + 4.)
        .width(Length::Fill)
        .style(widgets::well),
    );
    if !checking && let Some(warning) = persistence.warning() {
        content = content.push(kit::caution(ty, warning));
    }
    content = content.push(
        row![
            kit::icon_label(
                ty,
                Icon::Refresh,
                "Refresh status",
                widgets::Button::Secondary,
                state
                    .account
                    .can_check_storage()
                    .then_some(Message::Account(account::Action::CheckPersistence)),
            ),
            kit::icon_label(
                ty,
                Icon::Account,
                "Manage account",
                widgets::Button::Secondary,
                Some(Message::Show(Page::Account)),
            ),
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
    let choice = |label: UiElement<'static>,
                  selected: bool,
                  message: SettingsMessage|
     -> UiElement<'static> {
        let kind = widgets::Button::Choice(selected);
        kit::lift(
            kind,
            true,
            button(label)
                .padding([8, 10])
                .style(widgets::button_style_fading(kind))
                .on_press(wrap(message)),
        )
    };
    let mut sizes = row![].spacing(6);
    for size in TextSize::ALL {
        sizes = sizes.push(choice(
            ty.body(text_size_label(size), 13., theme::TEXT).into(),
            ty.text_size == size,
            SettingsMessage::TextSize(size),
        ));
    }
    let mut presets = row![].spacing(6);
    for preset in FontPreset::ALL {
        presets = presets.push(choice(
            ty.body(font_preset_label(preset), 13., theme::TEXT)
                .font(Typography { preset, ..ty }.font())
                .into(),
            ty.preset == preset,
            SettingsMessage::Font(preset),
        ));
    }
    let mut content = column![
        row![
            visuals::icon(Icon::Text, theme::MAGENTA, ty.size(18.)),
            ty.body("Text", 17., theme::TEXT),
        ]
        .spacing(theme::SPACE_2)
        .align_y(alignment::Vertical::Center),
        kit::heading(ty, Icon::Maximize, "Size"),
        sizes.wrap(),
        kit::heading(ty, Icon::Text, "Interface font"),
        presets.wrap(),
        // A sample set in what is chosen, so a change is seen where it is made.
        container(
            column![
                ty.body("Your workspace, your way.", 16., theme::TEXT),
                ty.data("Kore / models / inference.log", theme::TEXT_SECONDARY),
            ]
            .spacing(8)
        )
        .padding(14)
        .width(Length::Fill)
        .style(widgets::well),
        kit::note(
            ty,
            "Changes apply immediately and are saved automatically. Page titles keep their display face; paths and logs stay monospace.",
        ),
        kit::icon_label(
            ty,
            Icon::Refresh,
            "Reset text settings",
            widgets::Button::Secondary,
            Some(wrap(SettingsMessage::ResetTypography)),
        ),
    ]
    .spacing(12);
    if !state.typography_error.is_empty() {
        content = content.push(kit::error_data(ty, state.typography_error.as_str()));
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
