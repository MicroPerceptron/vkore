//! Account controls are a view over vkored's optional download authorization.
use crate::{Message, Page, UiElement, UserEvent, theme, typography::Typography, widgets};
use iced_core::Length;
use iced_widget::{button, column, container, row};
use iced_winit::winit::event_loop::EventLoopProxy;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use vkored::account::{Account, SignIn};

#[derive(Debug, Clone)]
pub enum Action {
    SignIn,
    Cancel,
    SignOut,
    OpenBrowser,
    Review,
}
#[derive(Debug)]
pub enum Reply {
    Challenge(u64, SignIn),
    Finished(u64, Result<(), String>),
}
#[derive(Default)]
pub struct State {
    pub account: Account,
    generation: u64,
    cancel: Option<Arc<AtomicBool>>,
    challenge: Option<SignIn>,
    busy: bool,
    error: String,
}
impl Drop for State {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Release);
        }
    }
}
impl State {
    #[cfg(test)]
    pub fn preview_sign_in(&mut self) {
        self.busy = true;
        self.challenge = Some(SignIn {
            code: "AB12CD34EF56".into(),
            url: "https://dev.microperceptron.com/download-account?device=1".into(),
        });
    }
    pub fn update(&mut self, action: Action, proxy: &EventLoopProxy<UserEvent>) {
        if matches!(action, Action::OpenBrowser) {
            let url = "https://dev.microperceptron.com/download-account?device=1";
            if let Err(error) = open_browser(url) {
                self.error = error;
            }
            return;
        }
        if matches!(action, Action::Cancel) {
            if let Some(cancel) = &self.cancel {
                cancel.store(true, Ordering::Release);
            }
            self.error = "Cancelling sign-in…".into();
            return;
        }
        if self.busy {
            return;
        }
        self.generation += 1;
        self.error.clear();
        self.challenge = None;
        self.busy = true;
        let generation = self.generation;
        let account = self.account.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        let proxy = proxy.clone();
        let result = std::thread::Builder::new()
            .name("vkore-account".into())
            .spawn(move || {
                let result = if matches!(action, Action::SignOut) {
                    account.sign_out()
                } else {
                    account
                        .sign_in(&cancel, |challenge| {
                            let _ = proxy.send_event(UserEvent::Account(Reply::Challenge(
                                generation, challenge,
                            )));
                        })
                        .map(|_| ())
                };
                let _ = proxy.send_event(UserEvent::Account(Reply::Finished(generation, result)));
            });
        if let Err(error) = result {
            self.busy = false;
            self.error = error.to_string();
        }
    }
    pub fn reply(&mut self, reply: Reply) -> bool {
        match reply {
            Reply::Challenge(generation, challenge) if generation == self.generation => {
                self.challenge = Some(challenge);
                if let Err(error) =
                    open_browser("https://dev.microperceptron.com/download-account?device=1")
                {
                    self.error = error;
                }
            }
            Reply::Finished(generation, result) if generation == self.generation => {
                self.busy = false;
                self.cancel = None;
                self.challenge = None;
                self.error = result.err().unwrap_or_default();
                return true;
            }
            _ => {}
        }
        false
    }
}
fn open_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "linux")]
    let result = std::process::Command::new("xdg-open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("rundll32.exe")
        .arg("url.dll,FileProtocolHandler")
        .arg(url)
        .spawn();
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let result: std::io::Result<std::process::Child> =
        Err(std::io::Error::other("unsupported host"));
    match result {
        Ok(mut child) => { std::thread::spawn(move || { let _ = child.wait(); }); Ok(()) }
        Err(_) => Err("Could not open the browser. Open dev.microperceptron.com/download-account?device=1 and enter the code below.".into()),
    }
}
pub fn view(state: &State, ty: Typography) -> UiElement<'_> {
    let control = |label, action| {
        button(ty.control(label))
            .padding([8, 14])
            .style(widgets::button_style(widgets::Button::Secondary))
            .on_press(Message::Account(action))
    };
    let mut content = column![
        ty.body("Account", 18., theme::TEXT),
        ty.body(
            "Optional for your workspace. Required to download official images.",
            14.,
            theme::TEXT_SECONDARY
        ),
    ]
    .spacing(14);
    if let Some(status) = state.account.status() {
        content = content.push(ty.data(format!("Signed in · {}", status.email), theme::SUCCESS))
            .push(ty.data(format!("Agreement accepted · {} · {}", status.receipt.version, status.receipt.accepted_at), theme::TEXT_MUTED))
            .push(ty.body("Sign-in lasts up to one hour. Downloads check authorization each time. Sign in again when it expires.", 12., theme::TEXT_MUTED));
        if !state.busy {
            content = content.push(
                row![
                    control("Review agreement", Action::Review),
                    control("Sign out", Action::SignOut)
                ]
                .spacing(8)
                .wrap(),
            );
        }
    } else if !state.busy {
        content = content
            .push(ty.badge("Not signed in", theme::TEXT_MUTED))
            .push(control("Sign in with Google", Action::SignIn));
    }
    if let Some(challenge) = &state.challenge {
        content = content
            .push(ty.body(
                "Enter this code in the browser, then review and accept the agreement.",
                14.,
                theme::TEXT,
            ))
            .push(
                container(ty.data(&challenge.code, theme::MAGENTA).size(ty.size(22.)))
                    .padding(16)
                    .style(widgets::card),
            )
            .push(
                row![
                    control("Open browser", Action::OpenBrowser),
                    control("Cancel", Action::Cancel)
                ]
                .spacing(8)
                .wrap(),
            );
    } else if state.busy {
        content = content.push(ty.body("Contacting the account service…", 14., theme::TEXT_MUTED));
    }
    if !state.error.is_empty() {
        content = content.push(ty.data(&state.error, theme::ERROR));
    }
    content = content.push(ty.body("Local images, running machines, volumes and the local API remain available offline. Credentials are not saved to disk.", 12., theme::TEXT_MUTED));
    container(content)
        .id("settings-account")
        .padding(24)
        .width(Length::Fill)
        .style(widgets::card)
        .into()
}
pub fn download_hint(ty: Typography) -> UiElement<'static> {
    button(ty.control("Account · Sign in for downloads"))
        .padding([8, 12])
        .style(widgets::button_style(widgets::Button::Secondary))
        .on_press(Message::Show(Page::Settings))
        .into()
}
