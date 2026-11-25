//! A session library of validated Wasm bundles and per-machine attachments.

use std::{collections::BTreeMap, path::PathBuf, sync::Arc, thread};

use iced_core::{Length, alignment};
use iced_widget::{button, column, container, row};
use iced_winit::winit::event_loop::EventLoopProxy;
use vkored::plugins::Bundle;

use crate::{Message, UiElement, UserEvent, theme, typography::Typography, widgets};

pub const DEMO_WASM: &[u8] = include_bytes!("../assets/plugins/animated-canvas.wasm");
const MAX_LIBRARY_ENTRIES: usize = 16;
const MAX_ATTACHMENTS: usize = 32;
const MAX_RESIDENT_BYTES: usize = 128 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    AnimatedCanvas,
    Local(PathBuf),
}

impl Source {
    fn name(&self) -> String {
        match self {
            Self::AnimatedCanvas => "Animated canvas".into(),
            Self::Local(path) => path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy()
                .into_owned(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Action {
    Import,
    Load(String, Source),
    Reload(String, Source),
    Unload(String, Source),
    #[cfg(feature = "gpu-canvas")]
    Preview(bool),
    #[cfg(feature = "browser-preview")]
    OpenBrowser(String, Source),
    #[cfg(feature = "browser-preview")]
    CloseBrowser(String),
    #[cfg(feature = "browser-preview")]
    BrowserInput(String, crate::browser::Input),
}

#[derive(Debug)]
pub enum Reply {
    Imported(Result<Option<(Source, Bundle)>, String>),
    Reloaded(String, Source, Result<Bundle, String>),
}

#[derive(Default)]
pub struct State {
    library: BTreeMap<Source, Bundle>,
    attached: BTreeMap<String, BTreeMap<Source, Bundle>>,
    busy: bool,
    status: String,
    status_machine: Option<String>,
    #[cfg(feature = "gpu-canvas")]
    pub canvas: Option<crate::gpu_canvas::Pane>,
    #[cfg(feature = "browser-preview")]
    browser: Option<(String, Source, crate::browser::Pane)>,
}

impl State {
    pub fn update(&mut self, action: Action, proxy: &EventLoopProxy<UserEvent>) {
        if self.busy {
            return;
        }
        self.status_machine = match &action {
            #[cfg(feature = "browser-preview")]
            Action::OpenBrowser(machine, _)
            | Action::CloseBrowser(machine)
            | Action::BrowserInput(machine, _) => Some(machine.clone()),
            Action::Load(machine, _) | Action::Reload(machine, _) | Action::Unload(machine, _) => {
                Some(machine.clone())
            }
            _ => None,
        };
        match action {
            Action::Import => {
                if self.library.len() >= MAX_LIBRARY_ENTRIES {
                    self.status = "The plugin library is full for this session.".into();
                    return;
                }
                self.start(proxy, || {
                    let result = (|| {
                        let Some(path) = rfd::FileDialog::new()
                            .set_title("Add a Wasm plugin")
                            .add_filter("WebAssembly", &["wasm"])
                            .pick_file()
                        else {
                            return Ok(None);
                        };
                        let path = path.canonicalize().map_err(|error| error.to_string())?;
                        let bundle = Bundle::open(&path)?;
                        Ok(Some((Source::Local(path), bundle)))
                    })();
                    Reply::Imported(result)
                });
            }
            Action::Load(machine, source) => self.load(machine, source),
            Action::Reload(machine, source) => {
                if self
                    .attached
                    .get(&machine)
                    .is_none_or(|plugins| !plugins.contains_key(&source))
                {
                    return;
                }
                self.start(proxy, move || {
                    let result = match &source {
                        Source::Local(path) => Bundle::open(path),
                        Source::AnimatedCanvas => Bundle::validate(Arc::from(DEMO_WASM)),
                    };
                    Reply::Reloaded(machine, source, result)
                });
            }
            Action::Unload(machine, source) => self.unload(&machine, &source),
            #[cfg(feature = "browser-preview")]
            Action::OpenBrowser(machine, source) => {
                if let Some(bundle) = self
                    .attached
                    .get(&machine)
                    .and_then(|plugins| plugins.get(&source))
                    && bundle.supports_browser()
                {
                    match crate::browser::Pane::open(bundle.clone(), proxy.clone()) {
                        Ok(pane) => self.browser = Some((machine, source, pane)),
                        Err(error) => self.status = format!("Could not open browser. {error}"),
                    }
                }
            }
            #[cfg(feature = "browser-preview")]
            Action::CloseBrowser(machine) => {
                if self
                    .browser
                    .as_ref()
                    .is_some_and(|(name, _, _)| *name == machine)
                {
                    self.browser = None;
                }
            }
            #[cfg(feature = "browser-preview")]
            Action::BrowserInput(machine, input) => {
                if let Some((name, _, pane)) = &mut self.browser
                    && *name == machine
                {
                    pane.input(input);
                }
            }
            #[cfg(feature = "gpu-canvas")]
            Action::Preview(open) => {
                if open {
                    self.canvas = Some(crate::gpu_canvas::Pane::open(None, proxy.clone()));
                } else {
                    self.canvas = None;
                }
            }
        }
    }

    fn load(&mut self, machine: String, source: Source) {
        if self.attached.values().map(BTreeMap::len).sum::<usize>() >= MAX_ATTACHMENTS {
            self.status = "Unload a plugin before attaching another one.".into();
            return;
        }
        let bundle = match &source {
            Source::AnimatedCanvas => Bundle::validate(Arc::from(DEMO_WASM)).ok(),
            Source::Local(_) => self.library.get(&source).cloned(),
        };
        if let Some(bundle) = bundle
            && self.can_retain(&bundle)
        {
            self.status = format!("Loaded {}.", source.name());
            self.attached
                .entry(machine)
                .or_default()
                .insert(source, bundle);
        }
    }

    fn unload(&mut self, machine: &str, source: &Source) {
        if let Some(plugins) = self.attached.get_mut(machine)
            && plugins.remove(source).is_some()
        {
            #[cfg(feature = "browser-preview")]
            self.close_browser_for(machine, source);
            self.status = format!("Unloaded {}.", source.name());
            if self.attached.get(machine).is_some_and(BTreeMap::is_empty) {
                self.attached.remove(machine);
            }
        }
    }

    fn start(
        &mut self,
        proxy: &EventLoopProxy<UserEvent>,
        work: impl FnOnce() -> Reply + Send + 'static,
    ) {
        let proxy = proxy.clone();
        match thread::Builder::new()
            .name("vkore-plugins".into())
            .spawn(move || {
                let _ = proxy.send_event(UserEvent::Plugin(work()));
            }) {
            Ok(_) => {
                self.busy = true;
                self.status.clear();
            }
            Err(error) => self.status = error.to_string(),
        }
    }

    fn can_retain(&mut self, new: &Bundle) -> bool {
        let mut resident = BTreeMap::new();
        for bundle in self
            .library
            .values()
            .chain(self.attached.values().flat_map(BTreeMap::values))
            .chain(std::iter::once(new))
        {
            resident.insert(bundle.bytes().as_ptr(), bundle.bytes().len());
        }
        if resident.values().sum::<usize>() <= MAX_RESIDENT_BYTES {
            return true;
        }
        self.status = "The loaded plugin bundles exceed the session memory limit.".into();
        false
    }

    pub fn reply(&mut self, reply: Reply) {
        self.busy = false;
        match reply {
            Reply::Imported(Ok(Some((source, bundle)))) if self.can_retain(&bundle) => {
                self.status = format!("Added {} to the plugin library.", source.name());
                self.library.insert(source, bundle);
            }
            Reply::Reloaded(machine, source, Ok(bundle)) if self.can_retain(&bundle) => {
                if let Some(previous) = self
                    .attached
                    .get_mut(&machine)
                    .and_then(|plugins| plugins.get_mut(&source))
                {
                    *previous = bundle;
                    #[cfg(feature = "browser-preview")]
                    self.close_browser_for(&machine, &source);
                    self.status = format!("Reloaded {}.", source.name());
                }
            }
            Reply::Imported(Err(error)) => self.status = format!("Could not add plugin. {error}"),
            Reply::Reloaded(_, _, Err(error)) => {
                self.status =
                    format!("Reload failed. The loaded version is still available. {error}")
            }
            _ => {}
        }
    }

    pub fn retain_machines(&mut self, keep: impl Fn(&str) -> bool) {
        self.attached.retain(|name, _| keep(name));
        #[cfg(feature = "browser-preview")]
        if self
            .browser
            .as_ref()
            .is_some_and(|(name, _, _)| !keep(name))
        {
            self.browser = None;
        }
    }

    #[cfg(feature = "browser-preview")]
    fn close_browser_for(&mut self, machine: &str, source: &Source) {
        if self
            .browser
            .as_ref()
            .is_some_and(|(name, loaded, _)| name == machine && loaded == source)
        {
            self.browser = None;
        }
    }

    #[cfg(feature = "browser-preview")]
    pub fn receive_browser(&mut self) {
        if let Some((_, _, pane)) = &mut self.browser {
            pane.receive();
        }
    }

    pub fn view(&self, ty: Typography) -> UiElement<'_> {
        #[cfg(feature = "gpu-canvas")]
        if let Some(canvas) = &self.canvas {
            return widgets::page_frame(
                column![
                    control(ty, "Back to plugins", Some(Action::Preview(false))),
                    canvas.view(ty),
                ]
                .spacing(theme::SPACE_4)
                .height(Length::Fill),
            );
        }
        let mut demo = column![
            ty.body("Animated canvas", 16., theme::TEXT),
            ty.body(
                "Bundled example · Available to load in any machine.",
                13.,
                theme::TEXT_MUTED
            ),
        ]
        .spacing(theme::SPACE_2);
        #[cfg(feature = "gpu-canvas")]
        {
            demo = demo.push(control(
                ty,
                "Preview",
                (!self.busy).then_some(Action::Preview(true)),
            ));
        }
        #[cfg(not(feature = "gpu-canvas"))]
        {
            demo = demo.push(ty.body("Preview unavailable in this build.", 13., theme::TEXT_MUTED));
        }
        let mut entries = column![
            container(demo)
                .width(Length::Fill)
                .padding(theme::SPACE_4)
                .style(widgets::card)
        ]
        .spacing(theme::SPACE_2);
        for (source, bundle) in &self.library {
            let Source::Local(path) = source else {
                continue;
            };
            entries = entries.push(
                container(
                    column![
                        ty.data(source.name(), theme::TEXT)
                            .wrapping(iced_core::text::Wrapping::WordOrGlyph),
                        ty.body(bundle.kind(), 13., theme::TEXT_MUTED),
                        ty.data(path.display().to_string(), theme::TEXT_MUTED)
                            .wrapping(iced_core::text::Wrapping::WordOrGlyph),
                    ]
                    .spacing(theme::SPACE_2),
                )
                .width(Length::Fill)
                .padding(theme::SPACE_4)
                .style(widgets::card),
            );
        }
        widgets::page_frame(
            column![
                row![
                    ty.page_header(
                        "Plugins",
                        "Add local WebAssembly bundles to use with machines."
                    ),
                    iced_widget::space::horizontal(),
                    control(ty, "Add plugin", (!self.busy).then_some(Action::Import)),
                ]
                .align_y(alignment::Vertical::Center)
                .spacing(theme::SPACE_2),
                widgets::scroll(entries)
                    .width(Length::Fill.max(800))
                    .height(Length::Fill),
                self.status(ty, None),
            ]
            .spacing(theme::SPACE_4)
            .height(Length::Fill),
        )
    }

    pub fn machine<'a>(&'a self, machine: &'a str, ty: Typography) -> UiElement<'a> {
        #[cfg(feature = "browser-preview")]
        if let Some((name, _, pane)) = &self.browser
            && name == machine
        {
            return pane.view(machine, ty);
        }
        let attached = self.attached.get(machine);
        let mut entries = column![].width(Length::Fill).spacing(theme::SPACE_2);
        if let Some(attached) = attached {
            for (source, bundle) in attached {
                let controls = row![
                    control(
                        ty,
                        "Reload",
                        (!self.busy).then(|| Action::Reload(machine.into(), source.clone()))
                    ),
                    control(
                        ty,
                        "Unload",
                        (!self.busy).then(|| Action::Unload(machine.into(), source.clone()))
                    ),
                ];
                #[cfg(feature = "browser-preview")]
                let controls = if bundle.supports_browser() {
                    controls.push(control(
                        ty,
                        "Open browser",
                        (!self.busy && self.browser.is_none())
                            .then(|| Action::OpenBrowser(machine.into(), source.clone())),
                    ))
                } else {
                    controls
                };
                let plugin = column![
                    ty.data(source.name(), theme::TEXT)
                        .wrapping(iced_core::text::Wrapping::WordOrGlyph),
                    ty.body(
                        format!("Loaded · {}", bundle.kind()),
                        12.,
                        theme::TEXT_MUTED
                    ),
                    controls.spacing(theme::SPACE_2).wrap(),
                ];
                #[cfg(not(feature = "browser-preview"))]
                let plugin = if bundle.supports_browser() {
                    plugin.push(
                        ty.body(
                            "Browser preview is disabled in this build.",
                            12.,
                            theme::TEXT_MUTED,
                        )
                        .wrapping(iced_core::text::Wrapping::WordOrGlyph),
                    )
                } else {
                    plugin
                };
                entries = entries.push(plugin.spacing(theme::SPACE_1));
            }
        }
        let demo = Source::AnimatedCanvas;
        let available = std::iter::once(&demo)
            .chain(self.library.keys())
            .filter(|source| attached.is_none_or(|plugins| !plugins.contains_key(*source)));
        for source in available {
            entries = entries.push(
                row![
                    ty.data(source.name(), theme::TEXT)
                        .wrapping(iced_core::text::Wrapping::WordOrGlyph),
                    control(
                        ty,
                        "Load",
                        (!self.busy).then(|| Action::Load(machine.into(), source.clone()))
                    ),
                ]
                .align_y(alignment::Vertical::Center)
                .spacing(theme::SPACE_2)
                .wrap(),
            );
        }
        entries = entries
            .push(
                button(ty.control("Plugin library"))
                    .height(ty.control_height())
                    .padding([0, 10])
                    .style(widgets::button_style(widgets::Button::Ghost))
                    .on_press(Message::Show(crate::Page::Plugins)),
            )
            .push(self.status(ty, Some(machine)));
        widgets::scroll(entries)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn status(&self, ty: Typography, machine: Option<&str>) -> UiElement<'_> {
        if self.status_machine.as_deref() != machine {
            return iced_widget::space::vertical().height(Length::Shrink).into();
        }
        if self.busy {
            return ty.body("Opening plugin…", 13., theme::TEXT_MUTED).into();
        }
        ty.data(&self.status, theme::TEXT_MUTED)
            .wrapping(iced_core::text::Wrapping::WordOrGlyph)
            .into()
    }
}

pub(super) fn control<'a>(
    ty: Typography,
    label: &'static str,
    action: Option<Action>,
) -> UiElement<'a> {
    button(ty.control(label))
        .height(ty.control_height())
        .padding([0, 10])
        .style(widgets::button_style(widgets::Button::Secondary))
        .on_press_maybe(action.map(Message::Plugin))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reload_preserves_failed_snapshots_and_affects_only_the_selected_machine() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("tool.wasm");
        let original = b"\0asm\x01\0\0\0";
        std::fs::write(&path, original).unwrap();
        let source = Source::Local(path.clone());
        let mut state = State::default();
        state.reply(Reply::Imported(Ok(Some((
            source.clone(),
            Bundle::open(&path).unwrap(),
        )))));
        state.load("first".into(), source.clone());
        state.load("second".into(), source.clone());
        assert_eq!(state.attached["first"][&source].bytes(), original);
        assert_eq!(state.attached["second"][&source].bytes(), original);

        std::fs::write(&path, b"not a Wasm file").unwrap();
        state.reply(Reply::Reloaded(
            "first".into(),
            source.clone(),
            Bundle::open(&path),
        ));
        assert_eq!(state.attached["first"][&source].bytes(), original);

        std::fs::write(&path, DEMO_WASM).unwrap();
        state.reply(Reply::Reloaded(
            "first".into(),
            source.clone(),
            Bundle::open(&path),
        ));
        assert_eq!(state.attached["first"][&source].kind(), "Wasm component");
        assert_eq!(state.attached["second"][&source].bytes(), original);
        state.unload("first", &source);
        assert!(!state.attached.contains_key("first"));
        assert!(state.attached["second"].contains_key(&source));
        assert!(state.library.contains_key(&source));
    }
}
