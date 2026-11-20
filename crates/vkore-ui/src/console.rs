//! Styled terminal output, always shaped in the registered mono face.
use iced_core::{Color, Length, text::Span};
use iced_widget::{container, rich_text};
use vkored::console::{Color as AnsiColor, Output};

use crate::{UiElement, fonts, theme, typography::Typography, widgets};

const BACKGROUND: Color = Color::from_rgb(0.035, 0.039, 0.05);
const FOREGROUND: Color = Color::from_rgb(0.83, 0.84, 0.87);

fn color(value: AnsiColor, default: Color, bright: bool) -> Color {
    let rgb = match value {
        AnsiColor::Default => return if bright { Color::WHITE } else { default },
        AnsiColor::Rgb(r, g, b) => [r, g, b],
        AnsiColor::Idx(mut index) => {
            if bright && index < 8 {
                index += 8;
            }
            match index {
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

pub fn view(output: &Output, ty: Typography) -> UiElement<'_> {
    screen(output, false, ty)
}

fn screen(output: &Output, focused: bool, ty: Typography) -> UiElement<'_> {
    let spans: Vec<Span<'_, ()>> = output
        .grid
        .iter()
        .map(|run| {
            let style = run.style;
            let mut fg = color(style.foreground, FOREGROUND, style.bold);
            let mut bg = color(style.background, BACKGROUND, false);
            if style.inverse {
                std::mem::swap(&mut fg, &mut bg);
            }
            if style.cursor && focused {
                fg = BACKGROUND;
                bg = FOREGROUND;
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
                .line_height(iced_core::text::LineHeight::Relative(1.35))
                .color(FOREGROUND),
        )
        .direction(iced_widget::scrollable::Direction::Both {
            vertical: iced_widget::scrollable::Scrollbar::default(),
            horizontal: iced_widget::scrollable::Scrollbar::default(),
        })
        .height(ty.size(theme::SIZE_MONO) * 1.35 * f32::from(vkored::console::ROWS) + 12.)
        .anchor_bottom(),
    )
    .padding(14)
    .width(Length::Fill)
    .style(move |t| container::Style {
        background: Some(BACKGROUND.into()),
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
    ty: Typography,
) -> UiElement<'a> {
    iced_core::Element::new(Terminal {
        content: screen(output, enabled && focused, ty),
        name,
        enabled,
        focused: enabled && focused,
        application_cursor: output.application_cursor,
    })
}

struct Terminal<'a> {
    content: UiElement<'a>,
    name: &'a str,
    enabled: bool,
    focused: bool,
    application_cursor: bool,
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
    fn click_and_typing_in_one_event_batch_reaches_the_guest() {
        crate::fonts::register().unwrap();
        let output = Output::default();
        let mut renderer = crate::native_renderer();
        let mut interface = iced_runtime::user_interface::UserInterface::build(
            interactive(&output, "guest", true, false, Typography::default()),
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
                interactive(&output, "guest", enabled, focused, Typography::default()),
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
