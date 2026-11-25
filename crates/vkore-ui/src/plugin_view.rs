//! Shared tab controls and pixel surfaces for interactive plugin tiles.

use iced_core::{ContentFit, Length, Rectangle, Size, Theme, image::Handle, mouse};
use iced_widget::{button, canvas, image, row, stack};

use crate::{AxUi, Message, UiElement, theme, typography::Typography, widgets};

pub fn tabs<'a, Id: Copy + Eq>(
    ty: Typography,
    entries: impl IntoIterator<Item = (Id, &'a str)>,
    active: Id,
    add: Option<Message>,
    select: impl Fn(Id) -> Message,
    close: impl Fn(Id) -> Option<Message>,
) -> UiElement<'a> {
    let mut strip = row![].spacing(theme::SPACE_2);
    for (id, label) in entries {
        let style = if id == active {
            widgets::Button::Choice(true)
        } else {
            widgets::Button::Secondary
        };
        strip = strip.push(
            row![
                button(
                    ty.data(label, theme::TEXT)
                        .height(Length::Fill)
                        .align_y(iced_core::alignment::Vertical::Center),
                )
                .height(ty.control_height())
                .padding([0., theme::SPACE_2])
                .style(widgets::button_style(style))
                .on_press(select(id)),
                control(ty, "×", close(id)),
            ]
            .spacing(theme::SPACE_1),
        );
    }
    strip.push(control(ty, "New tab", add)).wrap().into()
}

pub fn control<'a>(ty: Typography, label: &'static str, action: Option<Message>) -> UiElement<'a> {
    button(ty.control(label))
        .height(ty.control_height())
        .padding([0, 10])
        .style(widgets::button_style(widgets::Button::Secondary))
        .on_press_maybe(action)
        .into()
}

pub struct Frame {
    image: Handle,
    size: Size,
}

pub enum SurfaceInput {
    Click(u32, u32),
    Scroll(i32),
}

impl Frame {
    pub fn rgba(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        Self {
            image: Handle::from_rgba(width, height, pixels),
            size: Size::new(width as f32, height as f32),
        }
    }

    pub fn view<'a>(
        &self,
        on_input: impl Fn(SurfaceInput) -> Option<Message> + 'a,
    ) -> UiElement<'a> {
        stack![
            image(self.image.clone())
                .content_fit(ContentFit::Contain)
                .width(Length::Fill)
                .height(Length::Fill),
            canvas(HitTarget {
                size: self.size,
                on_input,
            })
            .width(Length::Fill)
            .height(Length::Fill),
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

struct HitTarget<F> {
    size: Size,
    on_input: F,
}

impl<F: Fn(SurfaceInput) -> Option<Message>> canvas::Program<Message, Theme, AxUi>
    for HitTarget<F>
{
    type State = ();

    fn update(
        &self,
        _: &mut (),
        event: &canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        let fitted = ContentFit::Contain.fit(self.size, bounds.size());
        let page = Rectangle {
            x: bounds.center_x() - fitted.width / 2.,
            y: bounds.center_y() - fitted.height / 2.,
            width: fitted.width,
            height: fitted.height,
        };
        let point = cursor.position_in(page)?;
        let input = match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let x = (point.x / fitted.width * self.size.width) as u32;
                let y = (point.y / fitted.height * self.size.height) as u32;
                SurfaceInput::Click(x, y)
            }
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let pixels = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => y * 40.,
                    mouse::ScrollDelta::Pixels { y, .. } => y * self.size.height / fitted.height,
                };
                let delta_y = -pixels.round() as i32;
                if delta_y == 0 {
                    return None;
                }
                SurfaceInput::Scroll(delta_y)
            }
            _ => return None,
        };
        let message = (self.on_input)(input)?;
        Some(canvas::Action::publish(message).and_capture())
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
