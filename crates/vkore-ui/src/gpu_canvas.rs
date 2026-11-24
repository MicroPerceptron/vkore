//! The optional canvas page samples one offscreen frame at a time. GPU work
//! and component calls run on a worker, outside the native UI event loop.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TrySendError},
    },
    thread,
    time::{Duration, Instant},
};

use iced_core::{Length, image::Handle};
use iced_widget::{column, container, image};
use vkored::gpu_canvas::{Canvas, HEIGHT, WIDTH};

use crate::{UiElement, UserEvent, theme, typography::Typography, widgets};
use iced_winit::winit::event_loop::EventLoopProxy;

const FRAME_INTERVAL: Duration = Duration::from_millis(66);

type Frame = Result<(Vec<u8>, String), String>;

pub struct Pane {
    frame: Option<Handle>,
    status: String,
    failed: bool,
    worker: Option<Worker>,
}

struct Worker {
    thread: thread::Thread,
    visible: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
    frames: Receiver<Frame>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        self.thread.unpark();
    }
}

impl Pane {
    pub fn open(path: Option<PathBuf>, proxy: EventLoopProxy<UserEvent>) -> Self {
        let mut pane = Self {
            frame: None,
            status: "Starting WASI-P2 component…".into(),
            failed: false,
            worker: None,
        };
        let (send, frames) = mpsc::sync_channel(1);
        let visible = Arc::new(AtomicBool::new(true));
        let stopped = Arc::new(AtomicBool::new(false));
        let run_visible = Arc::clone(&visible);
        let run_stopped = Arc::clone(&stopped);
        let worker = thread::Builder::new()
            .name("vkore-gpu-canvas".into())
            .spawn(move || {
                let run = || {
                    let mut canvas = match &path {
                        Some(path) => Canvas::open(path),
                        None => Canvas::from_bytes(crate::plugins::DEMO_WASM),
                    }
                    .map_err(|error| format!("{error:#}"))?;
                    let adapter = canvas.adapter().to_owned();
                    let start = Instant::now();
                    while !run_stopped.load(Ordering::Relaxed) {
                        if !run_visible.load(Ordering::Relaxed) {
                            thread::park();
                            continue;
                        }
                        let frame_start = Instant::now();
                        let pixels = canvas
                            .frame(start.elapsed())
                            .map_err(|error| format!("{error:#}"))?;
                        match send.try_send(Ok((pixels, adapter.clone()))) {
                            Ok(()) if proxy.send_event(UserEvent::CanvasReady).is_err() => break,
                            Err(TrySendError::Disconnected(_)) => break,
                            _ => {}
                        }
                        thread::park_timeout(FRAME_INTERVAL.saturating_sub(frame_start.elapsed()));
                    }
                    Ok::<_, String>(())
                };
                // Report initialization and rendering failures on the same lane.
                if let Err(error) = run()
                    && send.send(Err(error)).is_ok()
                {
                    let _ = proxy.send_event(UserEvent::CanvasReady);
                }
            });
        match worker {
            Ok(thread) => {
                pane.worker = Some(Worker {
                    thread: thread.thread().clone(),
                    visible,
                    stopped,
                    frames,
                });
            }
            Err(error) => {
                pane.failed = true;
                pane.status = error.to_string();
            }
        }
        pane
    }

    pub fn receive(&mut self) {
        let Some(frame) = self
            .worker
            .as_ref()
            .and_then(|worker| worker.frames.try_recv().ok())
        else {
            return;
        };
        match frame {
            Ok((pixels, adapter)) => {
                self.frame = Some(Handle::from_rgba(WIDTH, HEIGHT, pixels));
                self.status = adapter;
            }
            Err(error) => {
                self.failed = true;
                self.status = error;
                self.worker.take();
            }
        }
    }

    pub fn set_visible(&self, visible: bool) {
        if let Some(worker) = &self.worker {
            worker.visible.store(visible, Ordering::Relaxed);
            worker.thread.unpark();
        }
    }

    pub fn view(&self, ty: Typography) -> UiElement<'_> {
        view(ty, self.frame.as_ref(), &self.status, self.failed)
    }
}

pub(super) fn view<'a>(
    ty: Typography,
    frame: Option<&Handle>,
    status: &'a str,
    failed: bool,
) -> UiElement<'a> {
    let canvas: UiElement<'_> = if let Some(frame) = frame {
        image(frame.clone())
            .width(Length::Fill)
            .height(Length::Fill)
            .content_fit(iced_core::ContentFit::Contain)
            .into()
    } else {
        container(ty.body(
            if failed {
                "Canvas unavailable"
            } else {
                "Waiting for the first frame…"
            },
            18.,
            theme::TEXT,
        ))
        .center(Length::Fill)
        .into()
    };
    column![
        ty.body("Animated canvas", 24., theme::TEXT),
        container(canvas)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(widgets::card)
            .padding(theme::SPACE_4),
        ty.data(
            status,
            if failed {
                theme::ERROR
            } else {
                theme::TEXT_MUTED
            }
        )
        .wrapping(iced_core::text::Wrapping::WordOrGlyph),
    ]
    .spacing(theme::SPACE_4)
    .height(Length::Fill)
    .into()
}
