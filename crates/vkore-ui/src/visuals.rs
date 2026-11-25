//! Small native vector illustrations. No icon font or host font fallback.
//!
//! An icon is a table of strokes on a 24-unit grid, drawn with round caps and
//! joins in one theme tone. Keeping the set as data, not as drawing code, is
//! what lets it grow: a new glyph is a few lines of coordinates, and the tests
//! below prove every glyph stays on the grid and draws through the geometry
//! lane without a refusal.
use crate::{AxUi, Message, UiElement, theme};
use iced_core::{Length, Point, Rectangle, Size, Theme, mouse};
use iced_widget::canvas::{self, Frame, Geometry, LineCap, LineJoin, Path, Stroke, path};

/// The grid every glyph is drawn on; a glyph scales to its requested size.
const GRID: f32 = 24.0;
/// Stroke width on that grid.
const INK: f32 = 1.6;
/// Radius of a filled dot on that grid.
const DOT: f32 = 1.25;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Machine,
    Volume,
    Settings,
    Terminal,
    Plugin,
    Account,
    Collapse,
    Expand,
    Window,
    Play,
    Stop,
    Plus,
    Minus,
    Close,
    Copy,
    Text,
    Back,
    Forward,
    Globe,
    ChevronDown,
    ChevronUp,
    ChevronRight,
    Trash,
    Refresh,
    Maximize,
    Restore,
    Cpu,
    Memory,
    Folder,
    File,
    Hash,
    Lock,
    Check,
    Alert,
    Keyboard,
    Download,
    Network,
    Info,
    Activity,
    Dashboard,
    Grip,
}

/// One mark on the grid.
#[derive(Clone, Copy)]
enum Part {
    /// An open run of straight segments.
    Line(&'static [(f32, f32)]),
    /// A closed outline.
    Loop(&'static [(f32, f32)]),
    /// A circle outline: centre and radius.
    Circle(f32, f32, f32),
    /// A filled dot at a point.
    Dot(f32, f32),
    /// An arc: centre, radius, then start and end in degrees, clockwise from
    /// the positive x axis.
    Arc(f32, f32, f32, f32, f32),
}

use Part::{Arc, Circle, Dot, Line, Loop};

impl Icon {
    /// Every glyph, for the tests and for any gallery that wants the set.
    #[cfg(test)]
    const ALL: [Self; 41] = [
        Self::Machine,
        Self::Volume,
        Self::Settings,
        Self::Terminal,
        Self::Plugin,
        Self::Account,
        Self::Collapse,
        Self::Expand,
        Self::Window,
        Self::Play,
        Self::Stop,
        Self::Plus,
        Self::Minus,
        Self::Close,
        Self::Copy,
        Self::Text,
        Self::Back,
        Self::Forward,
        Self::Globe,
        Self::ChevronDown,
        Self::ChevronUp,
        Self::ChevronRight,
        Self::Trash,
        Self::Refresh,
        Self::Maximize,
        Self::Restore,
        Self::Cpu,
        Self::Memory,
        Self::Folder,
        Self::File,
        Self::Hash,
        Self::Lock,
        Self::Check,
        Self::Alert,
        Self::Keyboard,
        Self::Download,
        Self::Network,
        Self::Info,
        Self::Activity,
        Self::Dashboard,
        Self::Grip,
    ];

    fn parts(self) -> &'static [Part] {
        match self {
            Self::Account => &[
                Line(&[
                    (3., 21.),
                    (3., 18.),
                    (6., 15.),
                    (18., 15.),
                    (21., 18.),
                    (21., 21.),
                ]),
                Circle(12., 7., 4.),
            ],
            Self::Collapse => &[
                Loop(&[(3., 4.), (21., 4.), (21., 20.), (3., 20.)]),
                Line(&[(9., 4.), (9., 20.)]),
                Line(&[(17., 8.), (13., 12.), (17., 16.)]),
            ],
            Self::Expand => &[
                Loop(&[(3., 4.), (21., 4.), (21., 20.), (3., 20.)]),
                Line(&[(9., 4.), (9., 20.)]),
                Line(&[(13., 8.), (17., 12.), (13., 16.)]),
            ],
            Self::Machine => &[
                Loop(&[(3., 4.), (21., 4.), (21., 17.), (3., 17.)]),
                Line(&[(12., 17.), (12., 21.)]),
                Line(&[(8., 21.), (16., 21.)]),
            ],
            Self::Volume => &[
                Loop(&[
                    (5., 4.),
                    (19., 4.),
                    (21., 16.),
                    (21., 20.),
                    (3., 20.),
                    (3., 16.),
                ]),
                Line(&[(3., 15.), (21., 15.)]),
                Line(&[(16., 18.), (18., 18.)]),
            ],
            Self::Settings => &[
                Line(&[(5., 3.), (5., 6.)]),
                Line(&[(5., 10.), (5., 21.)]),
                Loop(&[(3., 6.), (7., 6.), (7., 10.), (3., 10.)]),
                Line(&[(12., 3.), (12., 14.)]),
                Line(&[(12., 18.), (12., 21.)]),
                Loop(&[(10., 14.), (14., 14.), (14., 18.), (10., 18.)]),
                Line(&[(19., 3.), (19., 8.)]),
                Line(&[(19., 12.), (19., 21.)]),
                Loop(&[(17., 8.), (21., 8.), (21., 12.), (17., 12.)]),
            ],
            Self::Terminal => &[
                Loop(&[(3., 4.), (21., 4.), (21., 20.), (3., 20.)]),
                Line(&[(7., 9.), (10., 12.), (7., 15.)]),
                Line(&[(13., 15.), (17., 15.)]),
            ],
            Self::Plugin => &[
                Loop(&[
                    (7., 8.),
                    (17., 8.),
                    (17., 12.),
                    (14., 16.),
                    (10., 16.),
                    (7., 12.),
                ]),
                Line(&[(9., 8.), (9., 3.)]),
                Line(&[(15., 8.), (15., 3.)]),
                Line(&[(12., 16.), (12., 21.)]),
            ],
            Self::Window => &[
                Loop(&[(3., 4.), (21., 4.), (21., 20.), (3., 20.)]),
                Line(&[(3., 9.), (21., 9.)]),
                Dot(6.5, 6.5),
            ],
            Self::Play => &[Loop(&[(7., 4.), (19., 12.), (7., 20.)])],
            Self::Stop => &[Loop(&[(6., 6.), (18., 6.), (18., 18.), (6., 18.)])],
            Self::Plus => &[
                Line(&[(12., 5.), (12., 19.)]),
                Line(&[(5., 12.), (19., 12.)]),
            ],
            Self::Minus => &[Line(&[(5., 12.), (19., 12.)])],
            Self::Close => &[Line(&[(6., 6.), (18., 18.)]), Line(&[(18., 6.), (6., 18.)])],
            Self::Copy => &[
                Loop(&[(9., 9.), (20., 9.), (20., 20.), (9., 20.)]),
                Line(&[(5., 15.), (4., 15.), (4., 4.), (15., 4.), (15., 5.)]),
            ],
            Self::Text => &[
                Line(&[(4., 20.), (11., 4.), (18., 20.)]),
                Line(&[(6.5, 14.), (15.5, 14.)]),
                Line(&[(19., 12.), (21., 12.)]),
            ],
            Self::Back => &[
                Line(&[(19., 12.), (5., 12.)]),
                Line(&[(11., 6.), (5., 12.), (11., 18.)]),
            ],
            Self::Forward => &[
                Line(&[(5., 12.), (19., 12.)]),
                Line(&[(13., 6.), (19., 12.), (13., 18.)]),
            ],
            Self::Globe => &[
                Circle(12., 12., 9.),
                Line(&[(3., 12.), (21., 12.)]),
                Line(&[(12., 3.), (8.5, 7.5), (7.5, 12.), (8.5, 16.5), (12., 21.)]),
                Line(&[
                    (12., 3.),
                    (15.5, 7.5),
                    (16.5, 12.),
                    (15.5, 16.5),
                    (12., 21.),
                ]),
            ],
            Self::ChevronDown => &[Line(&[(6., 9.), (12., 15.), (18., 9.)])],
            Self::ChevronRight => &[Line(&[(9., 6.), (15., 12.), (9., 18.)])],
            Self::Trash => &[
                Line(&[(4., 7.), (20., 7.)]),
                Line(&[(9., 7.), (9., 4.), (15., 4.), (15., 7.)]),
                Line(&[(6., 7.), (7., 21.), (17., 21.), (18., 7.)]),
                Line(&[(10., 11.), (10., 17.)]),
                Line(&[(14., 11.), (14., 17.)]),
            ],
            Self::ChevronUp => &[Line(&[(6., 15.), (12., 9.), (18., 15.)])],
            Self::Refresh => &[
                Arc(12., 12., 8., 40., 326.),
                Line(&[(18.6, 2.5), (18.6, 7.5), (13.6, 7.5)]),
            ],
            Self::Maximize => &[Loop(&[(4., 4.), (20., 4.), (20., 20.), (4., 20.)])],
            Self::Restore => &[
                Loop(&[(4., 8.), (16., 8.), (16., 20.), (4., 20.)]),
                Line(&[(8., 8.), (8., 4.), (20., 4.), (20., 16.), (16., 16.)]),
            ],
            Self::Cpu => &[
                Loop(&[(6., 6.), (18., 6.), (18., 18.), (6., 18.)]),
                Loop(&[(10., 10.), (14., 10.), (14., 14.), (10., 14.)]),
                Line(&[(9., 3.), (9., 6.)]),
                Line(&[(15., 3.), (15., 6.)]),
                Line(&[(9., 18.), (9., 21.)]),
                Line(&[(15., 18.), (15., 21.)]),
                Line(&[(3., 9.), (6., 9.)]),
                Line(&[(3., 15.), (6., 15.)]),
                Line(&[(18., 9.), (21., 9.)]),
                Line(&[(18., 15.), (21., 15.)]),
            ],
            Self::Memory => &[
                Loop(&[(2., 6.), (22., 6.), (22., 15.), (2., 15.)]),
                Line(&[(6., 15.), (6., 19.)]),
                Line(&[(10., 15.), (10., 19.)]),
                Line(&[(14., 15.), (14., 19.)]),
                Line(&[(18., 15.), (18., 19.)]),
                Line(&[(6., 10.), (8., 10.)]),
                Line(&[(11., 10.), (13., 10.)]),
                Line(&[(16., 10.), (18., 10.)]),
            ],
            Self::Folder => &[Loop(&[
                (3., 6.),
                (10., 6.),
                (12., 8.),
                (21., 8.),
                (21., 19.),
                (3., 19.),
            ])],
            Self::File => &[
                Loop(&[(6., 3.), (14., 3.), (19., 8.), (19., 21.), (6., 21.)]),
                Line(&[(14., 3.), (14., 8.), (19., 8.)]),
            ],
            Self::Hash => &[
                Line(&[(9., 4.), (7., 20.)]),
                Line(&[(17., 4.), (15., 20.)]),
                Line(&[(4., 9.), (20., 9.)]),
                Line(&[(3., 15.), (19., 15.)]),
            ],
            Self::Lock => &[
                Loop(&[(5., 11.), (19., 11.), (19., 21.), (5., 21.)]),
                Line(&[(8., 11.), (8., 7.)]),
                Arc(12., 7., 4., 180., 360.),
                Line(&[(16., 7.), (16., 11.)]),
            ],
            Self::Check => &[Line(&[(5., 12.), (10., 17.), (19., 7.)])],
            Self::Alert => &[
                Loop(&[(12., 3.), (22., 20.), (2., 20.)]),
                Line(&[(12., 9.), (12., 14.)]),
                Dot(12., 17.),
            ],
            Self::Keyboard => &[
                Loop(&[(2., 6.), (22., 6.), (22., 18.), (2., 18.)]),
                Line(&[(6., 10.), (7., 10.)]),
                Line(&[(10., 10.), (11., 10.)]),
                Line(&[(14., 10.), (15., 10.)]),
                Line(&[(18., 10.), (19., 10.)]),
                Line(&[(7., 14.), (17., 14.)]),
            ],
            Self::Download => &[
                Line(&[(12., 4.), (12., 15.)]),
                Line(&[(7., 10.), (12., 15.), (17., 10.)]),
                Line(&[(4., 20.), (20., 20.)]),
            ],
            Self::Network => &[
                Circle(12., 5., 2.),
                Circle(5., 19., 2.),
                Circle(19., 19., 2.),
                Line(&[(11., 6.8), (6., 17.2)]),
                Line(&[(13., 6.8), (18., 17.2)]),
                Line(&[(7., 19.), (17., 19.)]),
            ],
            Self::Info => &[
                Circle(12., 12., 9.),
                Line(&[(12., 11.), (12., 17.)]),
                Dot(12., 7.5),
            ],
            Self::Activity => &[Line(&[
                (2., 12.),
                (7., 12.),
                (10., 4.),
                (14., 20.),
                (17., 12.),
                (22., 12.),
            ])],
            Self::Dashboard => &[
                Loop(&[(3., 3.), (11., 3.), (11., 11.), (3., 11.)]),
                Loop(&[(13., 3.), (21., 3.), (21., 8.), (13., 8.)]),
                Loop(&[(13., 10.), (21., 10.), (21., 21.), (13., 21.)]),
                Loop(&[(3., 13.), (11., 13.), (11., 21.), (3., 21.)]),
            ],
            Self::Grip => &[
                Dot(9., 6.),
                Dot(15., 6.),
                Dot(9., 12.),
                Dot(15., 12.),
                Dot(9., 18.),
                Dot(15., 18.),
            ],
        }
    }
}

/// Strokes `parts` into `frame`, which is already scaled to the grid.
fn ink(frame: &mut Frame<AxUi>, parts: &[Part], color: iced_core::Color) {
    let stroke = Stroke::default()
        .with_color(color)
        .with_width(INK)
        .with_line_cap(LineCap::Round)
        .with_line_join(LineJoin::Round);
    let at = |&(x, y): &(f32, f32)| Point::new(x, y);
    for part in parts {
        match *part {
            Part::Line(points) => frame.stroke(
                &Path::new(|p| {
                    p.move_to(at(&points[0]));
                    for point in &points[1..] {
                        p.line_to(at(point));
                    }
                }),
                stroke,
            ),
            Part::Loop(points) => frame.stroke(
                &Path::new(|p| {
                    p.move_to(at(&points[0]));
                    for point in &points[1..] {
                        p.line_to(at(point));
                    }
                    p.close();
                }),
                stroke,
            ),
            Part::Circle(x, y, r) => frame.stroke(&Path::circle(Point::new(x, y), r), stroke),
            Part::Dot(x, y) => frame.fill(&Path::circle(Point::new(x, y), DOT), color),
            Part::Arc(x, y, r, from, to) => frame.stroke(
                &Path::new(|p| {
                    p.arc(path::Arc {
                        center: Point::new(x, y),
                        radius: r,
                        start_angle: from.to_radians().into(),
                        end_angle: to.to_radians().into(),
                    });
                }),
                stroke,
            ),
        }
    }
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
        frame.scale(bounds.width / GRID);
        ink(&mut frame, self.0.parts(), theme::color(appearance, self.1));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn points(part: &Part) -> Vec<(f32, f32)> {
        match *part {
            Part::Line(points) | Part::Loop(points) => points.to_vec(),
            Part::Circle(x, y, r) | Part::Arc(x, y, r, ..) => {
                vec![(x - r, y - r), (x + r, y + r)]
            }
            Part::Dot(x, y) => vec![(x - DOT, y - DOT), (x + DOT, y + DOT)],
        }
    }

    #[test]
    fn every_glyph_stays_on_the_grid_with_room_for_its_ink() {
        for icon in Icon::ALL {
            assert!(!icon.parts().is_empty(), "{icon:?} draws nothing");
            for part in icon.parts() {
                for (x, y) in points(part) {
                    let margin = INK / 2.0;
                    assert!(
                        (margin - 0.01..=GRID - margin + 0.01).contains(&x)
                            && (margin - 0.01..=GRID - margin + 0.01).contains(&y),
                        "{icon:?} leaves the grid at ({x}, {y})"
                    );
                }
            }
        }
    }

    #[test]
    fn open_runs_and_outlines_have_the_points_they_need() {
        for icon in Icon::ALL {
            for part in icon.parts() {
                match part {
                    Part::Line(points) => assert!(points.len() >= 2, "{icon:?} has a short line"),
                    Part::Loop(points) => assert!(points.len() >= 3, "{icon:?} has a short loop"),
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn every_glyph_draws_through_the_geometry_lane_without_a_refusal() {
        use iced_graphics::geometry::Renderer as _;
        let mut renderer = crate::native_renderer();
        for icon in Icon::ALL {
            let program = Glyph(icon, theme::TEXT);
            let geometry = canvas::Program::<Message, Theme, AxUi>::draw(
                &program,
                &(),
                &renderer,
                &Theme::Dark,
                Rectangle::new(Point::ORIGIN, Size::new(24., 24.)),
                mouse::Cursor::Unavailable,
            );
            assert_eq!(geometry.len(), 1, "{icon:?}");
            // The lane reports a refusal when the geometry is recorded.
            for geometry in geometry {
                renderer.draw_geometry(geometry);
            }
            assert!(
                renderer.take_geometry_errors().is_empty(),
                "{icon:?} asked the geometry lane for something it cannot draw"
            );
        }
    }
}
