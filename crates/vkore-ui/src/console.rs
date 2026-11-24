//! Styled terminal output, always shaped in the registered mono face.
use iced_core::{Color, Length, text::Span};
use iced_widget::{container, rich_text};
use vkored::console::{Color as AnsiColor, Output};

use crate::{UiElement, fonts, theme, typography::Typography, widgets};

const PADDING: f32 = 14.;
const SCROLLBAR_SPACE: f32 = 12.;
const LINE_HEIGHT: f32 = 1.35;

#[derive(Clone, Copy)]
pub struct Presentation {
    pub ty: Typography,
    light: bool,
    height: Length,
}

impl Presentation {
    #[cfg(test)]
    pub fn new(ty: Typography, appearance: &iced_core::Theme, viewport_height: f32) -> Self {
        Self {
            ty,
            light: matches!(appearance, iced_core::Theme::Light),
            // The detail pane also contains launch controls and boot activity.
            // Keep the terminal proportional to the window, with a usable
            // minimum in short windows (the containing detail pane scrolls).
            height: Length::Fixed((viewport_height * 0.6).clamp(160., 1600.) - 2. * PADDING),
        }
    }

    #[cfg(test)]
    fn fixed_height(self) -> f32 {
        let Length::Fixed(content) = self.height else {
            unreachable!()
        };
        content + 2. * PADDING
    }

    pub fn tile(ty: Typography, appearance: &iced_core::Theme) -> Self {
        Self {
            ty,
            light: matches!(appearance, iced_core::Theme::Light),
            height: Length::Fill,
        }
    }

    fn background(self) -> Color {
        if self.light {
            Color::from_rgb8(250, 249, 246)
        } else {
            Color::from_rgb(0.035, 0.039, 0.05)
        }
    }

    fn foreground(self) -> Color {
        if self.light {
            Color::from_rgb8(32, 32, 38)
        } else {
            Color::from_rgb(0.83, 0.84, 0.87)
        }
    }

    fn geometry(self, bounds: iced_core::Size) -> (u16, u16) {
        // Derive the advance from the same bundled face used for rendering,
        // not a host font or an estimate that drifts with font preferences.
        static ADVANCE: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
        let advance = *ADVANCE.get_or_init(|| {
            let face = ttf_parser::Face::parse(theme::MONO_BYTES, 0).expect("bundled mono face");
            let glyph = face.glyph_index('M').expect("mono ASCII glyph");
            f32::from(face.glyph_hor_advance(glyph).expect("mono advance"))
                / f32::from(face.units_per_em())
        });
        let size = self.ty.size(theme::SIZE_MONO);
        let rows = ((bounds.height - 2. * PADDING - SCROLLBAR_SPACE) / (size * LINE_HEIGHT)).floor()
            as u16;
        let cols =
            ((bounds.width - 2. * PADDING - SCROLLBAR_SPACE) / (size * advance)).floor() as u16;
        (
            rows.clamp(1, vkored::console::MAX_ROWS),
            cols.clamp(1, vkored::console::MAX_COLS),
        )
    }
}

fn color(value: AnsiColor, default: Color, bright: bool, light: bool) -> Color {
    let rgb = match value {
        AnsiColor::Default => return default,
        AnsiColor::Rgb(r, g, b) => [r, g, b],
        AnsiColor::Idx(mut index) => {
            if bright && index < 8 {
                index += 8;
            }
            match index {
                0..=15 if light => [
                    [32, 32, 38],
                    [163, 36, 36],
                    [28, 108, 55],
                    [123, 86, 0],
                    [36, 77, 170],
                    [128, 42, 146],
                    [0, 100, 119],
                    [77, 75, 85],
                    [98, 96, 106],
                    [185, 43, 43],
                    [24, 116, 58],
                    [135, 94, 0],
                    [47, 86, 183],
                    [144, 46, 157],
                    [0, 111, 127],
                    [32, 32, 38],
                ][usize::from(index)],
                0..=15 => [
                    [24, 24, 28],
                    [205, 70, 70],
                    [80, 180, 110],
                    [195, 165, 65],
                    [90, 130, 220],
                    [175, 105, 205],
                    [65, 170, 185],
                    [200, 200, 205],
                    [115, 115, 125],
                    [255, 105, 105],
                    [120, 220, 145],
                    [240, 215, 100],
                    [130, 170, 255],
                    [215, 145, 245],
                    [100, 220, 235],
                    [245, 245, 250],
                ][usize::from(index)],
                16..=231 => {
                    let n = index - 16;
                    let component = |v| if v == 0 { 0 } else { 55 + 40 * v };
                    [component(n / 36), component(n / 6 % 6), component(n % 6)]
                }
                _ => [8 + (index - 232) * 10; 3],
            }
        }
    };
    Color::from_rgb8(rgb[0], rgb[1], rgb[2])
}

pub fn view(output: &Output, presentation: Presentation) -> UiElement<'_> {
    screen(output, false, presentation)
}

fn screen(output: &Output, focused: bool, presentation: Presentation) -> UiElement<'_> {
    let ty = presentation.ty;
    let background = presentation.background();
    let foreground = presentation.foreground();
    let spans: Vec<Span<'_, ()>> = output
        .grid
        .iter()
        .map(|run| {
            let style = run.style;
            let mut fg = color(style.foreground, foreground, style.bold, presentation.light);
            let mut bg = color(style.background, background, false, presentation.light);
            if style.inverse {
                std::mem::swap(&mut fg, &mut bg);
            }
            if style.cursor && focused {
                fg = background;
                bg = foreground;
            }
            if style.dim {
                fg = Color::from_rgb((fg.r + bg.r) / 2., (fg.g + bg.g) / 2., (fg.b + bg.b) / 2.);
            }
            // Parsing removes terminal commands; this second boundary still escapes
            // unsupported glyphs, bidi controls and invisible data before shaping.
            Span::new(fonts::shown(
                fonts::MONO,
                run.text.as_str(),
                fonts::Breaks::Keep,
            ))
            .font(fonts::MONO)
            .color(fg)
            .background(bg)
            .underline(style.underline || (style.cursor && !focused))
        })
        .collect();
    container(
        widgets::scroll(
            rich_text(spans)
                .font(fonts::MONO)
                .wrapping(iced_core::text::Wrapping::None)
                .size(ty.size(theme::SIZE_MONO))
                .line_height(iced_core::text::LineHeight::Relative(LINE_HEIGHT))
                .color(foreground),
        )
        .direction(iced_widget::scrollable::Direction::Both {
            vertical: iced_widget::scrollable::Scrollbar::default(),
            horizontal: iced_widget::scrollable::Scrollbar::default(),
        })
        .height(presentation.height)
        .anchor_bottom(),
    )
    .padding(PADDING)
    .width(Length::Fill)
    .style(move |t| container::Style {
        background: Some(background.into()),
        border: iced_core::Border {
            radius: 8.into(),
            width: 1.,
            color: theme::color(
                t,
                if focused {
                    theme::MAGENTA
                } else {
                    theme::BORDER
                },
            ),
        },
        ..Default::default()
    })
    .into()
}

mod input;

pub fn interactive<'a>(
    output: &'a Output,
    name: &'a str,
    enabled: bool,
    focused: bool,
    presentation: Presentation,
) -> UiElement<'a> {
    iced_core::Element::new(Terminal {
        content: screen(output, enabled && focused, presentation),
        name,
        enabled,
        focused: enabled && focused,
        application_cursor: output.application_cursor,
        presentation,
        geometry: (output.rows, output.cols),
    })
}

struct Terminal<'a> {
    content: UiElement<'a>,
    name: &'a str,
    enabled: bool,
    focused: bool,
    application_cursor: bool,
    presentation: Presentation,
    geometry: (u16, u16),
}

impl iced_core::Widget<crate::Message, iced_core::Theme, crate::AxUi> for Terminal<'_> {
    fn size(&self) -> iced_core::Size<Length> {
        self.content.as_widget().size()
    }
    fn diff(&mut self, tree: &mut iced_core::widget::Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.content));
    }
    fn layout(
        &mut self,
        tree: &mut iced_core::widget::Tree,
        renderer: &crate::AxUi,
        limits: &iced_core::layout::Limits,
    ) -> iced_core::layout::Node {
        if tree.children.is_empty() {
            tree.children
                .push(iced_core::widget::Tree::new(&self.content));
        }
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }
    fn update(
        &mut self,
        tree: &mut iced_core::widget::Tree,
        event: &iced_core::Event,
        layout: iced_core::Layout<'_>,
        cursor: iced_core::mouse::Cursor,
        renderer: &crate::AxUi,
        shell: &mut iced_core::Shell<'_, crate::Message>,
        viewport: &iced_core::Rectangle,
    ) {
        use crate::machines::MachineMessage;
        use iced_core::{Event, mouse};
        if self.enabled
            && matches!(
                event,
                Event::Window(iced_core::window::Event::RedrawRequested(_))
            )
        {
            let geometry = self.presentation.geometry(layout.bounds().size());
            if geometry != self.geometry {
                self.geometry = geometry;
                shell.publish(crate::Message::Machine(MachineMessage::ConsoleResize(
                    self.name.into(),
                    geometry.0,
                    geometry.1,
                )));
            }
        }
        if self.enabled
            && cursor.is_over(layout.bounds())
            && cursor.is_over(*viewport)
            && matches!(
                event,
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
            )
        {
            self.focused = true;
            shell.publish(crate::Message::Machine(MachineMessage::ConsoleFocus(Some(
                self.name.into(),
            ))));
        } else if self.focused
            && matches!(
                event,
                Event::Mouse(mouse::Event::ButtonPressed(_))
                    | Event::Window(iced_core::window::Event::Unfocused)
            )
        {
            self.focused = false;
            shell.publish(crate::Message::Machine(MachineMessage::ConsoleFocus(None)));
        }
        if self.focused
            && !shell.is_event_captured()
            && let Some(action) = input::key(event, self.application_cursor)
        {
            let message = match action {
                input::Action::Send(bytes) => MachineMessage::ConsoleInput(self.name.into(), bytes),
                input::Action::Release => {
                    self.focused = false;
                    MachineMessage::ConsoleFocus(None)
                }
                input::Action::Paste => MachineMessage::ConsolePaste(self.name.into()),
            };
            shell.publish(crate::Message::Machine(message));
            shell.capture_event();
            return;
        }
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
    }
    fn draw(
        &self,
        tree: &iced_core::widget::Tree,
        renderer: &mut crate::AxUi,
        theme: &iced_core::Theme,
        style: &iced_core::renderer::Style,
        layout: iced_core::Layout<'_>,
        cursor: iced_core::mouse::Cursor,
        viewport: &iced_core::Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }
    fn mouse_interaction(
        &self,
        tree: &iced_core::widget::Tree,
        layout: iced_core::Layout<'_>,
        cursor: iced_core::mouse::Cursor,
        viewport: &iced_core::Rectangle,
        renderer: &crate::AxUi,
    ) -> iced_core::mouse::Interaction {
        let interaction = self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        );
        if interaction == iced_core::mouse::Interaction::None
            && self.enabled
            && cursor.is_over(layout.bounds())
        {
            iced_core::mouse::Interaction::Text
        } else {
            interaction
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced_core::{
        keyboard::{Key, Modifiers},
        shell,
    };
    #[test]
    fn light_palette_is_readable_and_preserves_explicit_rgb() {
        let light = Presentation::new(Typography::default(), &iced_core::Theme::Light, 800.);
        let dark = Presentation::new(Typography::default(), &iced_core::Theme::Dark, 800.);
        let luminance = |color: Color| {
            let linear = |v: f32| {
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
        };
        assert!(luminance(light.background()) > 0.9);
        assert!(luminance(dark.background()) < 0.01);
        for index in 0..16 {
            let foreground = color(AnsiColor::Idx(index), light.foreground(), false, true);
            assert!(
                (luminance(light.background()) + 0.05) / (luminance(foreground) + 0.05) >= 4.5,
                "ANSI {index}"
            );
        }
        assert_eq!(
            color(AnsiColor::Default, light.foreground(), true, true),
            light.foreground()
        );
        assert_eq!(
            color(AnsiColor::Rgb(1, 2, 3), light.foreground(), false, true),
            Color::from_rgb8(1, 2, 3)
        );
    }

    #[test]
    fn layout_reports_geometry_and_converges_after_resize() {
        crate::fonts::register().unwrap();
        let mut previous = (0, 0);
        for bounds in [
            iced_core::Size::new(500., 560.),
            iced_core::Size::new(1100., 1000.),
        ] {
            let presentation = Presentation::new(
                Typography::default(),
                &iced_core::Theme::Light,
                bounds.height,
            );
            let expected = presentation.geometry(iced_core::Size::new(
                bounds.width,
                presentation.fixed_height(),
            ));
            assert!(expected.0 > previous.0 && expected.1 > previous.1);
            previous = expected;
            for current in [(24, 120), expected] {
                let output = Output {
                    rows: current.0,
                    cols: current.1,
                    ..Output::default()
                };
                let mut renderer = crate::native_renderer();
                let mut interface = iced_runtime::user_interface::UserInterface::build(
                    interactive(&output, "guest", true, false, presentation),
                    bounds,
                    iced_runtime::user_interface::Cache::new(),
                    &mut renderer,
                );
                let mut messages = shell::Bus::new();
                interface.update(
                    &iced_core::window::Headless,
                    &shell::Waker::noop(),
                    &[iced_core::Event::Window(
                        iced_core::window::Event::RedrawRequested(std::time::Instant::now()),
                    )],
                    iced_core::mouse::Cursor::Unavailable,
                    &mut renderer,
                    &mut messages,
                );
                let messages: Vec<_> = messages.into_iter().collect();
                if current == expected {
                    assert!(messages.is_empty());
                } else {
                    assert!(
                        matches!(messages.as_slice(), [crate::Message::Machine(crate::machines::MachineMessage::ConsoleResize(name, rows, cols))] if name == "guest" && (*rows, *cols) == expected)
                    );
                }
            }
        }
    }

    #[test]
    fn responsive_terminal_renders_in_both_themes_and_font_sizes() {
        use crate::typography::TextSize;
        use std::{io::Write, os::unix::net::UnixListener};
        for bounds in [
            iced_core::Size::new(640., 480.),
            iced_core::Size::new(1280., 900.),
        ] {
            let mut normal_geometry = None;
            for text_size in [TextSize::Default, TextSize::ExtraLarge] {
                for appearance in [iced_core::Theme::Light, iced_core::Theme::Dark] {
                    let ty = Typography {
                        text_size,
                        ..Typography::default()
                    };
                    let presentation = Presentation::new(ty, &appearance, bounds.height);
                    let geometry = presentation.geometry(iced_core::Size::new(
                        bounds.width - 48.,
                        presentation.fixed_height(),
                    ));
                    if text_size == TextSize::Default {
                        normal_geometry = Some(geometry);
                    } else {
                        let normal = normal_geometry.unwrap();
                        assert!(geometry.0 < normal.0 && geometry.1 < normal.1);
                    }
                    let dir = tempfile::tempdir().unwrap();
                    let path = dir.path().join("console");
                    let listener = UnixListener::bind(&path).unwrap();
                    let mut console = vkored::console::Console::connect(&path).unwrap();
                    let (mut guest, _) = listener.accept().unwrap();
                    console.resize(geometry.0, geometry.1);
                    let bytes = format!(
                        "\x1b[1;36mKore terminal\x1b[0m\r\n{}\r\n\x1b[32mReady\x1b[0m\r\nterminal:0> ",
                        "0123456789".repeat(usize::from(geometry.1) / 5)
                    );
                    guest.write_all(bytes.as_bytes()).unwrap();
                    console.poll();
                    let (runs, refused) = crate::tests::draw_at_scale(
                        widgets::page_frame(screen(console.output(), true, presentation)),
                        bounds,
                        &appearance,
                        1.,
                        Some(if text_size == TextSize::Default {
                            "terminal-adaptive"
                        } else {
                            "terminal-adaptive-large"
                        }),
                    );
                    assert!(runs > 3);
                    assert!(refused.is_empty(), "{refused:?}");
                }
            }
        }
    }

    #[test]
    fn click_and_typing_in_one_event_batch_reaches_the_guest() {
        crate::fonts::register().unwrap();
        let output = Output::default();
        let mut renderer = crate::native_renderer();
        let mut interface = iced_runtime::user_interface::UserInterface::build(
            interactive(
                &output,
                "guest",
                true,
                false,
                Presentation::new(Typography::default(), &iced_core::Theme::Dark, 600.),
            ),
            iced_core::Size::new(800., 500.),
            iced_runtime::user_interface::Cache::new(),
            &mut renderer,
        );
        let mut messages = shell::Bus::new();
        interface.update(
            &iced_core::window::Headless,
            &shell::Waker::noop(),
            &[
                iced_core::Event::Mouse(iced_core::mouse::Event::ButtonPressed(
                    iced_core::mouse::Button::Left,
                )),
                input::press(Key::Character("a".into()), Modifiers::empty(), Some("a")),
                input::press(
                    Key::Named(iced_core::keyboard::key::Named::Escape),
                    Modifiers::CTRL | Modifiers::SHIFT,
                    None,
                ),
                input::press(Key::Character("b".into()), Modifiers::empty(), Some("b")),
            ],
            iced_core::mouse::Cursor::Available(iced_core::Point::new(20., 20.)),
            &mut renderer,
            &mut messages,
        );
        let messages: Vec<_> = messages.into_iter().collect();
        use crate::{Message::Machine, machines::MachineMessage::*};
        assert!(
            matches!(messages.as_slice(), [Machine(ConsoleFocus(Some(name))), Machine(ConsoleInput(_, bytes)), Machine(ConsoleFocus(None))] if name == "guest" && bytes == b"a")
        );
    }

    #[test]
    fn only_the_focused_connected_terminal_captures_keyboard_input() {
        crate::fonts::register().unwrap();
        for (enabled, focused) in [(true, true), (true, false), (false, true)] {
            let output = Output::default();
            let mut renderer = crate::native_renderer();
            let mut interface = iced_runtime::user_interface::UserInterface::build(
                interactive(
                    &output,
                    "guest",
                    enabled,
                    focused,
                    Presentation::new(Typography::default(), &iced_core::Theme::Dark, 600.),
                ),
                iced_core::Size::new(800., 500.),
                iced_runtime::user_interface::Cache::new(),
                &mut renderer,
            );
            let mut messages = shell::Bus::new();
            let (_, statuses) = interface.update(
                &iced_core::window::Headless,
                &shell::Waker::noop(),
                &[input::press(
                    Key::Character("c".into()),
                    Modifiers::CTRL,
                    None,
                )],
                iced_core::mouse::Cursor::Unavailable,
                &mut renderer,
                &mut messages,
            );
            let messages: Vec<_> = messages.into_iter().collect();
            if enabled && focused {
                assert!(
                    matches!(messages.as_slice(), [crate::Message::Machine(crate::machines::MachineMessage::ConsoleInput(name, bytes))] if name == "guest" && bytes == b"\x03")
                );
                assert_eq!(statuses, [iced_core::event::Status::Captured]);
            } else {
                assert!(messages.is_empty());
            }
        }
    }
}
