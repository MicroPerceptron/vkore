//! Small native vector illustrations. No icon font or host font fallback.
use crate::{AxUi, Message, UiElement, theme};
use iced_core::{Length, Point, Rectangle, Size, Theme, mouse};
use iced_widget::canvas::{self, Frame, Geometry, Path, Stroke};

#[derive(Clone, Copy)]
pub enum Icon {
    Machine,
    Volume,
    Settings,
    Terminal,
}

struct Glyph(Icon, theme::Tone);
impl canvas::Program<Message, Theme, AxUi> for Glyph {
    type State = ();
    fn draw(
        &self,
        _: &(),
        renderer: &AxUi,
        appearance: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<Geometry<AxUi>> {
        let mut frame = Frame::new(renderer, bounds.size());
        frame.scale(bounds.width / 24.0);
        let ink = Stroke::default()
            .with_color(theme::color(appearance, self.1))
            .with_width(1.6);
        let mut line = |points: &[(f32, f32)]| {
            frame.stroke(
                &Path::new(|p| {
                    p.move_to(Point::new(points[0].0, points[0].1));
                    for &(x, y) in &points[1..] {
                        p.line_to(Point::new(x, y));
                    }
                }),
                ink,
            );
        };
        match self.0 {
            Icon::Machine => {
                line(&[(3., 4.), (21., 4.), (21., 17.), (3., 17.), (3., 4.)]);
                line(&[(12., 17.), (12., 21.)]);
                line(&[(8., 21.), (16., 21.)]);
            }
            Icon::Volume => {
                line(&[
                    (5., 4.),
                    (19., 4.),
                    (21., 16.),
                    (21., 20.),
                    (3., 20.),
                    (3., 16.),
                    (5., 4.),
                ]);
                line(&[(3., 15.), (21., 15.)]);
                line(&[(16., 18.), (18., 18.)]);
            }
            Icon::Settings => {
                for (x, y) in [(5., 8.), (12., 16.), (19., 10.)] {
                    line(&[(x, 3.), (x, y - 2.)]);
                    line(&[(x, y + 2.), (x, 21.)]);
                    line(&[
                        (x - 2., y - 2.),
                        (x + 2., y - 2.),
                        (x + 2., y + 2.),
                        (x - 2., y + 2.),
                        (x - 2., y - 2.),
                    ]);
                }
            }
            Icon::Terminal => {
                line(&[(3., 4.), (21., 4.), (21., 20.), (3., 20.), (3., 4.)]);
                line(&[(7., 9.), (10., 12.), (7., 15.)]);
                line(&[(13., 15.), (17., 15.)]);
            }
        }
        vec![frame.into_geometry()]
    }
}
pub fn icon<'a>(kind: Icon, tone: theme::Tone, size: f32) -> UiElement<'a> {
    iced_widget::canvas(Glyph(kind, tone))
        .width(size)
        .height(size)
        .into()
}

struct Preview(theme::Appearance);
impl canvas::Program<Message, Theme, AxUi> for Preview {
    type State = ();
    fn draw(
        &self,
        _: &(),
        renderer: &AxUi,
        _: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<Geometry<AxUi>> {
        let mut frame = Frame::new(renderer, bounds.size());
        let mut window = |x: f32, width: f32, dark: bool| {
            let palette = if dark { Theme::Dark } else { Theme::Light };
            let mut block = |x, y, w, h, tone| {
                frame.fill_rectangle(
                    Point::new(x, y),
                    Size::new(w, h),
                    theme::color(&palette, tone),
                )
            };
            block(x, 0., width, 84., theme::BACKGROUND);
            block(x + 1., 1., width * 0.24, 82., theme::PANEL);
            block(x + 6., 12., width * 0.13, 3., theme::MAGENTA);
            block(x + 6., 23., width * 0.13, 3., theme::BORDER);
            block(x + 6., 33., width * 0.13, 3., theme::BORDER);
            let content = x + width * 0.31;
            block(content, 12., width * 0.35, 4., theme::TEXT_SECONDARY);
            block(content, 25., width * 0.59, 21., theme::PANEL);
            block(content, 52., width * 0.27, 21., theme::PANEL);
            block(content + width * 0.32, 52., width * 0.27, 21., theme::PANEL);
        };
        match self.0 {
            theme::Appearance::System => {
                window(0., bounds.width / 2., false);
                window(bounds.width / 2., bounds.width / 2., true);
            }
            theme::Appearance::Light => window(0., bounds.width, false),
            theme::Appearance::Dark => window(0., bounds.width, true),
        }
        vec![frame.into_geometry()]
    }
}
pub fn appearance_preview<'a>(choice: theme::Appearance) -> UiElement<'a> {
    iced_widget::canvas(Preview(choice))
        .width(Length::Fill)
        .height(84)
        .into()
}
