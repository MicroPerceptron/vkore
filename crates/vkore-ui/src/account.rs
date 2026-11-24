//! Account controls are a view over vkored's optional download authorization.
use crate::{Message, Page, UiElement, UserEvent, theme, typography::Typography, widgets};
use iced_core::{Length, alignment, clipboard};
use iced_widget::{button, column, container, row};
use iced_winit::winit::event_loop::EventLoopProxy;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use vkored::account::{Account, SignIn};

#[derive(Debug, Clone)]
pub enum Action {
    CheckPersistence,
    SignIn,
    Cancel,
    SignOut,
    OpenBrowser,
    CopyCode,
    Review,
}
#[derive(Debug)]
pub enum Reply {
    Restored { refresh_catalog: bool },
    Challenge(u64, SignIn),
    CodeCopied(u64, Result<(), clipboard::Error>),
    Finished(u64, Result<(), String>),
}
#[derive(Default)]
pub struct State {
    pub account: Account,
    generation: u64,
    cancel: Option<Arc<AtomicBool>>,
    challenge: Option<SignIn>,
    clipboard: Option<iced_winit::Clipboard>,
    copy_result: Option<Result<(), clipboard::Error>>,
    busy: bool,
    checking_storage: bool,
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
    pub fn restore(&mut self, proxy: EventLoopProxy<UserEvent>, refresh_catalog: bool) -> bool {
        if !self.can_check_storage() {
            return false;
        }
        let account = self.account.clone();
        let started = std::thread::Builder::new()
            .name("vkore-account-restore".into())
            .spawn(move || {
                account.restore();
                let _ = proxy.send_event(UserEvent::Account(Reply::Restored { refresh_catalog }));
            })
            .is_ok();
        self.checking_storage = started;
        started
    }

    pub fn checking_storage(&self) -> bool {
        self.checking_storage
    }

    pub fn can_check_storage(&self) -> bool {
        !self.busy && !self.checking_storage
    }

    #[cfg(test)]
    pub fn preview_sign_in(&mut self) {
        self.busy = true;
        self.challenge = Some(SignIn {
            code: "AB12CD34EF56".into(),
            url: "https://dev.microperceptron.com/download-account?device=1".into(),
        });
    }
    pub fn update(&mut self, action: Action, proxy: &EventLoopProxy<UserEvent>) {
        if matches!(action, Action::CheckPersistence) {
            self.restore(proxy.clone(), false);
            return;
        }
        if matches!(action, Action::CopyCode) {
            if let Some(challenge) = &self.challenge {
                let generation = self.generation;
                let proxy = proxy.clone();
                self.copy_result = None;
                self.clipboard
                    .get_or_insert_with(iced_winit::Clipboard::new)
                    .write(
                        clipboard::Content::Text(challenge.code.clone()),
                        move |result| {
                            let _ = proxy.send_event(UserEvent::Account(Reply::CodeCopied(
                                generation, result,
                            )));
                        },
                    );
            }
            return;
        }
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
            self.challenge = None;
            self.copy_result = None;
            self.error = "Cancelling sign-in…".into();
            return;
        }
        if !self.can_check_storage() {
            return;
        }
        self.generation += 1;
        self.error.clear();
        self.challenge = None;
        self.copy_result = None;
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
            Reply::Restored { refresh_catalog } => {
                self.checking_storage = false;
                return refresh_catalog;
            }
            Reply::Challenge(generation, challenge)
                if generation == self.generation
                    && self
                        .cancel
                        .as_ref()
                        .is_some_and(|cancel| !cancel.load(Ordering::Acquire)) =>
            {
                self.challenge = Some(challenge);
                if let Err(error) =
                    open_browser("https://dev.microperceptron.com/download-account?device=1")
                {
                    self.error = error;
                }
            }
            Reply::CodeCopied(generation, result)
                if generation == self.generation && self.challenge.is_some() =>
            {
                self.copy_result = Some(result);
            }
            Reply::Finished(generation, result) if generation == self.generation => {
                self.busy = false;
                self.cancel = None;
                self.challenge = None;
                self.copy_result = None;
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
/// Display-only summary. Download authorization remains in vkored and the server.
#[derive(Default)]
pub struct Preview {
    pub email: Option<String>,
    pub pending: bool,
}
impl State {
    pub fn preview(&self) -> Preview {
        Preview {
            email: self.account.status().map(|status| status.email),
            pending: self.busy,
        }
    }
}

pub fn page(state: &State, ty: Typography) -> UiElement<'_> {
    widgets::page_frame(
        column![
            ty.page_header(
                "Account",
                "Manage your sign-in and image download agreement."
            ),
            widgets::scroll(column![view(state, ty)]).height(Length::Fill),
        ]
        .spacing(theme::SPACE_6),
    )
}

pub fn view(state: &State, ty: Typography) -> UiElement<'_> {
    let control = |label, action| {
        button(ty.control(label))
            .padding([8, 14])
            .style(widgets::button_style(widgets::Button::Secondary))
            .on_press(Message::Account(action))
    };
    let mut content = column![
        ty.body("Image downloads", 18., theme::TEXT),
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
            .push(ty.body("Downloads check authorization each time. Sign in again when the one-hour session expires.", 12., theme::TEXT_MUTED));
        if state.can_check_storage() {
            content = content.push(
                row![
                    control("Review agreement", Action::Review),
                    control("Sign out", Action::SignOut)
                ]
                .spacing(8)
                .wrap(),
            );
        }
    } else if state.can_check_storage() {
        content = content
            .push(ty.badge("Not signed in", theme::TEXT_MUTED))
            .push(crate::google_sign_in::button(ty));
        if state.account.sign_out_incomplete() {
            content = content.push(control("Retry sign-out", Action::SignOut));
        }
    }
    if let Some(challenge) = &state.challenge {
        content = content
            .push(ty.body(
                "Copy the code into the browser, then review and accept the agreement.",
                14.,
                theme::TEXT,
            ))
            .push(
                row![
                    container(ty.data(&challenge.code, theme::MAGENTA).size(ty.size(22.)))
                        .padding(16)
                        .style(widgets::card),
                    control(
                        if matches!(state.copy_result, Some(Ok(()))) {
                            "Copied"
                        } else {
                            "Copy code"
                        },
                        Action::CopyCode,
                    ),
                ]
                .align_y(alignment::Vertical::Center)
                .spacing(8)
                .wrap(),
            )
            .push(
                row![
                    control("Open browser", Action::OpenBrowser),
                    control("Cancel", Action::Cancel)
                ]
                .spacing(8)
                .wrap(),
            );
        if matches!(state.copy_result, Some(Err(_))) {
            content = content.push(ty.body(
                "Could not copy the code. Try again or enter it in the browser.",
                12.,
                theme::ERROR,
            ));
        }
    } else if state.checking_storage() {
        content = content.push(ty.body("Checking saved sign-in…", 14., theme::TEXT_MUTED));
    } else if state.busy {
        content = content.push(ty.body("Contacting the account service…", 14., theme::TEXT_MUTED));
    }
    if !state.error.is_empty() {
        content = content.push(ty.data(&state.error, theme::ERROR));
    }
    if let Some(warning) = state.account.storage_warning()
        && warning != state.error
    {
        content = content.push(ty.body(warning, 12., theme::WARNING));
    }
    content = content.push(ty.body("Your system keyring can restore sign-in across launches. Local images, machines, volumes and the local API remain available offline.", 12., theme::TEXT_MUTED));
    container(content)
        .id("account-details")
        .padding(24)
        .width(Length::Fill)
        .style(widgets::card)
        .into()
}
pub fn download_hint(ty: Typography) -> UiElement<'static> {
    button(ty.control("Account · Sign in for downloads"))
        .padding([8, 12])
        .style(widgets::button_style(widgets::Button::Secondary))
        .on_press(Message::Show(Page::Account))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checking_persistence_only_refreshes_catalog_during_startup() {
        let mut state = State::default();
        for refresh_catalog in [false, true] {
            state.checking_storage = true;
            assert!(!state.can_check_storage());
            assert_eq!(
                state.reply(Reply::Restored { refresh_catalog }),
                refresh_catalog
            );
            assert!(state.can_check_storage());
        }
    }

    #[test]
    fn clipboard_feedback_belongs_to_the_active_sign_in() {
        let mut state = State::default();
        state.preview_sign_in();
        state.generation = 2;
        assert!(!state.reply(Reply::CodeCopied(1, Ok(()))));
        assert!(state.copy_result.is_none());
        assert!(!state.reply(Reply::CodeCopied(2, Ok(()))));
        assert!(matches!(state.copy_result, Some(Ok(()))));

        assert!(state.reply(Reply::Finished(2, Ok(()))));
        assert!(!state.reply(Reply::CodeCopied(2, Ok(()))));
        assert!(state.copy_result.is_none());
    }
}
