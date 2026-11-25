//! Interactive browser surface for a machine's Plugins tile.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

use iced_core::{ContentFit, Length, Rectangle, Size, Theme, image::Handle, mouse};
use iced_widget::{canvas, column, image, row, stack};
use iced_winit::winit::event_loop::EventLoopProxy;
use vkored::{
    browser::{BrowserFrame, Session},
    plugins::Bundle,
};

use crate::{AxUi, Message, UiElement, UserEvent, plugins, theme, typography::Typography};

#[derive(Clone, Copy, Debug)]
pub enum Input {
    Click(u32, u32),
    Back,
}

struct Frame {
    image: Handle,
    size: Size,
    location: String,
    can_go_back: bool,
}

pub struct Pane {
    input: mpsc::SyncSender<Input>,
    output: mpsc::Receiver<Result<BrowserFrame, String>>,
    frame: Option<Frame>,
    pending: bool,
    status: String,
}

static WORKER_RUNNING: AtomicBool = AtomicBool::new(false);

struct Worker;

impl Drop for Worker {
    fn drop(&mut self) {
        WORKER_RUNNING.store(false, Ordering::Release);
    }
}

impl Pane {
    pub fn open(bundle: Bundle, proxy: EventLoopProxy<UserEvent>) -> std::io::Result<Self> {
        if WORKER_RUNNING.swap(true, Ordering::Acquire) {
            return Err(std::io::Error::other(
                "The previous browser is still closing.",
            ));
        }
        let worker = Worker;
        let (input, events) = mpsc::sync_channel(1);
        let (frames, output) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("vkore-browser".into())
            .spawn(move || {
                let _worker = worker;
                let publish = |frame: Result<BrowserFrame, String>| {
                    frames.send(frame).is_ok() && proxy.send_event(UserEvent::BrowserFrame).is_ok()
                };
                match Session::open(&bundle) {
                    Ok((mut session, first)) => {
                        if !publish(Ok(first)) {
                            return;
                        }
                        for input in events {
                            let frame = match input {
                                Input::Click(x, y) => session.click(x, y),
                                Input::Back => session.back(),
                            };
                            let failed = frame.is_err();
                            if !publish(frame.map_err(|error| format!("{error:#}"))) || failed {
                                break;
                            }
                        }
                    }
                    Err(error) => {
                        publish(Err(format!("{error:#}")));
                    }
                }
            })?;
        Ok(Self {
            input,
            output,
            frame: None,
            pending: true,
            status: "Starting browser…".into(),
        })
    }

    pub fn receive(&mut self) {
        let Ok(frame) = self.output.try_recv() else {
            return;
        };
        self.pending = false;
        match frame {
            Ok(frame) => {
                self.frame = Some(Frame {
                    image: Handle::from_rgba(frame.width, frame.height, frame.rgba),
                    size: Size::new(frame.width as f32, frame.height as f32),
                    location: frame.location,
                    can_go_back: frame.can_go_back,
                });
                self.status.clear();
            }
            Err(error) => {
                self.frame = None;
                self.status = error;
            }
        }
    }

    pub fn input(&mut self, input: Input) {
        if !self.pending && self.frame.is_some() && self.input.try_send(input).is_ok() {
            self.pending = true;
        }
    }

    pub fn view<'a>(&'a self, machine: &'a str, ty: Typography) -> UiElement<'a> {
        let mut toolbar = row![plugins::control(
            ty,
            "Close browser",
            Some(plugins::Action::CloseBrowser(machine.into()))
        ),]
        .spacing(theme::SPACE_2);
        if let Some(frame) = &self.frame {
            toolbar = toolbar
                .push(plugins::control(
                    ty,
                    "Back",
                    (frame.can_go_back && !self.pending)
                        .then(|| plugins::Action::BrowserInput(machine.into(), Input::Back)),
                ))
                .push(ty.data(&frame.location, theme::TEXT_MUTED));
            return column![
                toolbar
                    .align_y(iced_core::alignment::Vertical::Center)
                    .wrap(),
                stack![
                    image(frame.image.clone())
                        .content_fit(ContentFit::Contain)
                        .width(Length::Fill)
                        .height(Length::Fill),
                    canvas(HitTarget {
                        machine,
                        size: frame.size,
                        enabled: !self.pending
                    })
                    .width(Length::Fill)
                    .height(Length::Fill),
                ]
                .width(Length::Fill)
                .height(Length::Fill),
            ]
            .spacing(theme::SPACE_2)
            .height(Length::Fill)
            .into();
        }
        column![
            toolbar,
            ty.data(&self.status, theme::TEXT_MUTED)
                .wrapping(iced_core::text::Wrapping::WordOrGlyph),
        ]
        .spacing(theme::SPACE_2)
        .into()
    }
}

struct HitTarget<'a> {
    machine: &'a str,
    size: Size,
    enabled: bool,
}

impl canvas::Program<Message, Theme, AxUi> for HitTarget<'_> {
    type State = ();

    fn update(
        &self,
        _: &mut (),
        event: &canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        if !self.enabled
            || !matches!(
                event,
                canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
            )
        {
            return None;
        }
        let fitted = ContentFit::Contain.fit(self.size, bounds.size());
        let page = Rectangle {
            x: bounds.center_x() - fitted.width / 2.,
            y: bounds.center_y() - fitted.height / 2.,
            width: fitted.width,
            height: fitted.height,
        };
        let point = cursor.position_in(page)?;
        let x = (point.x / fitted.width * self.size.width) as u32;
        let y = (point.y / fitted.height * self.size.height) as u32;
        Some(
            canvas::Action::publish(Message::Plugin(plugins::Action::BrowserInput(
                self.machine.into(),
                Input::Click(x, y),
            )))
            .and_capture(),
        )
    }

    fn draw(
        &self,
        _: &(),
        _: &AxUi,
        _: &Theme,
        _: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<canvas::Geometry<AxUi>> {
        Vec::new()
    }
}
