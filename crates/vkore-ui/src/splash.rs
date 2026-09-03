//! The mark, and the launch splash that powers it on.
//!
//! Both draw through the geometry lane. The mark is the published SVG's paths;
//! the splash is the same paths with the screen bars and plate driven by the
//! published timings, then the wordmark.
//!
//! This is the one branded motion moment in the system. The timings are the
//! design's own and are not re-eased.

use crate::{AxUi, Message, logo, theme};
use iced_core::{Length, Point, Rectangle, Size, Theme, alignment, mouse};
use iced_widget::canvas::{self, Geometry, Path};
use iced_widget::{column, container, stack};
use std::time::Duration;

/// Published splash timings.
const TV_ON: Duration = Duration::from_millis(1500);
const WORDMARK_AT: Duration = Duration::from_millis(900);
const WORDMARK_OVER: Duration = Duration::from_millis(1100);
const FADE_AT: Duration = Duration::from_millis(2000);
const DONE: Duration = Duration::from_millis(2220);

/// The mark's drawn size on the splash.
const MARK: f32 = 216.0;

/// Opacity of the screen bars over the power-on, as published.
///
/// The bars flicker to one, dip, and settle: 0 → 1 → .06 → 1 → .14 → 1 → .5
/// → 1 across the fifteen hundred milliseconds. Linear between stops, because
/// a CRT does not ease.
fn bar_opacity(progress: f32) -> f32 {
    const STOPS: [f32; 8] = [0.0, 1.0, 0.06, 1.0, 0.14, 1.0, 0.5, 1.0];
    stops(progress, &STOPS)
}

/// Opacity of the off-white screen plate: 0 → .85 → 0 → .4 → 0.
fn plate_opacity(progress: f32) -> f32 {
    const STOPS: [f32; 5] = [0.0, 0.85, 0.0, 0.4, 0.0];
    stops(progress, &STOPS)
}

/// Linear interpolation across evenly spaced stops.
fn stops(progress: f32, stops: &[f32]) -> f32 {
    let progress = progress.clamp(0.0, 1.0);
    let last = stops.len() - 1;
    let scaled = progress * last as f32;
    let index = (scaled.floor() as usize).min(last);
    if index == last {
        return stops[last];
    }
    let fraction = scaled - index as f32;
    stops[index] + (stops[index + 1] - stops[index]) * fraction
}

/// How far through the splash `elapsed` is, and whether it is over.
pub fn progress(elapsed: Duration) -> Option<Phase> {
    if elapsed >= DONE {
        return None;
    }
    Some(Phase {
        power: elapsed.min(TV_ON).as_secs_f32() / TV_ON.as_secs_f32(),
        wordmark: if elapsed <= WORDMARK_AT {
            0.0
        } else {
            ((elapsed - WORDMARK_AT).as_secs_f32() / (WORDMARK_OVER - WORDMARK_AT).as_secs_f32())
                .min(1.0)
        },
        fade: if elapsed <= FADE_AT {
            0.0
        } else {
            ((elapsed - FADE_AT).as_secs_f32() / (DONE - FADE_AT).as_secs_f32()).min(1.0)
        },
    })
}

/// Where the splash is, as three independent progressions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Phase {
    /// Power-on, zero to one over the first 1500ms.
    pub power: f32,
    /// Wordmark fade-in.
    pub wordmark: f32,
    /// Whole-overlay fade-out.
    pub fade: f32,
}

/// Draws the mark's paths at a given size and opacity.
pub struct Mark {
    /// Drawn edge length. The mark is square.
    pub size: f32,
    /// Multiplied into every shape's alpha.
    pub opacity: f32,
    /// Power-on progress, or `None` for the mark at rest.
    pub power: Option<f32>,
}

/// The rasterized mark, kept until something it depends on moves.
#[derive(Default)]
pub struct MarkState {
    cache: canvas::Cache<AxUi>,
    signature: std::cell::Cell<u64>,
}

impl Mark {
    /// Everything the drawn mark depends on, folded into one value.
    ///
    /// Eighteen paths is real work. A mark at rest never changes, so it must
    /// not be rasterized again -- and a display list that differs every frame
    /// would defeat the retained backend for the whole window, not just here.
    fn signature(&self, bounds: Rectangle) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        let mut mix = |value: u64| {
            hash = (hash ^ value).wrapping_mul(0x0000_0100_0000_01b3);
        };
        mix(self.size.to_bits().into());
        mix(self.opacity.to_bits().into());
        mix(self.power.unwrap_or(-1.0).to_bits().into());
        mix(bounds.width.to_bits().into());
        mix(bounds.height.to_bits().into());
        hash
    }
}

impl canvas::Program<Message, Theme, AxUi> for Mark {
    type State = MarkState;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &AxUi,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry<AxUi>> {
        let signature = self.signature(bounds);
        if state.signature.replace(signature) != signature {
            state.cache.clear();
        }
        vec![state.cache.draw_with_bounds(renderer, bounds, |frame| {
            let scale = self.size / logo::VIEWBOX;
            let inset_x = (bounds.width - self.size).max(0.0) / 2.0;
            let inset_y = (bounds.height - self.size).max(0.0) / 2.0;
            frame.translate(iced_core::Vector::new(inset_x, inset_y));
            frame.scale(scale);

            for shape in logo::SHAPES {
                let animated = logo::SCREEN_BARS.contains(&shape.id);
                let alpha = match (self.power, animated) {
                    (Some(power), true) => bar_opacity(power),
                    _ => 1.0,
                } * self.opacity;
                if alpha <= 0.0 {
                    continue;
                }
                // zeno parses the published path data as it stands.
                frame.fill(&Path::new(|builder| svg_into(builder, shape.data)), {
                    let mut color = theme::rgb(shape.fill);
                    color.a = alpha;
                    color
                });
            }

            // The plate flashes over the screen while it powers on.
            if let Some(power) = self.power {
                let alpha = plate_opacity(power) * self.opacity;
                if alpha > 0.0
                    && let Some(screen) = logo::SHAPES.iter().find(|shape| shape.id == "screen")
                {
                    frame.fill(&Path::new(|builder| svg_into(builder, screen.data)), {
                        let mut color = theme::rgb(theme::TEXT);
                        color.a = alpha;
                        color
                    });
                }
            }
        })]
    }
}

/// Feeds SVG path data into a path builder.
///
/// zeno owns the parsing; this only replays what it yields, so the published
/// asset is the single source of the mark's shape.
fn svg_into(builder: &mut iced_graphics::geometry::path::Builder, data: &str) {
    use zeno::{Command, PathData};

    let point = |p: zeno::Point| Point::new(p.x, p.y);
    for command in data.commands() {
        match command {
            Command::MoveTo(to) => builder.move_to(point(to)),
            Command::LineTo(to) => builder.line_to(point(to)),
            Command::QuadTo(control, to) => builder.quadratic_curve_to(point(control), point(to)),
            Command::CurveTo(first, second, to) => {
                builder.bezier_curve_to(point(first), point(second), point(to));
            }
            Command::Close => builder.close(),
        }
    }
}

/// The launch overlay: the mark powering on, then the wordmark.
pub fn overlay<'a>(phase: Phase) -> crate::UiElement<'a> {
    let opacity = 1.0 - phase.fade;
    container(
        column![
            container(
                iced_widget::canvas(Mark {
                    size: MARK,
                    opacity,
                    power: Some(phase.power),
                })
                .width(MARK)
                .height(MARK)
            )
            .width(MARK)
            .height(MARK),
            iced_widget::text("KORE AGENTS")
                .font(crate::fonts::SPLASH)
                .size(theme::SIZE_DISPLAY)
                .color({
                    let mut color = theme::rgb(theme::TEXT);
                    color.a = phase.wordmark * opacity;
                    color
                }),
        ]
        .spacing(34)
        .align_x(alignment::Horizontal::Center),
    )
    .center(Length::Fill)
    .style(move |_| {
        let mut style = theme::panel(theme::BACKGROUND, None, 0.0);
        style.background = Some(iced_core::Background::Color(theme::rgba(
            theme::BACKGROUND,
            opacity,
        )));
        style
    })
    .into()
}

/// The mark at rest, for the sidebar's brand row.
pub fn mark<'a>(size: f32) -> crate::UiElement<'a> {
    stack![
        iced_widget::canvas(Mark {
            size,
            opacity: 1.0,
            power: None,
        })
        .width(size)
        .height(size)
    ]
    .width(size)
    .height(size)
    .into()
}

/// The splash's total length, so a caller knows when to stop asking.
pub const LENGTH: Duration = DONE;

#[allow(dead_code)]
fn _sized(_: Size) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bars_start_dark_and_settle_lit() {
        assert_eq!(bar_opacity(0.0), 0.0);
        assert_eq!(bar_opacity(1.0), 1.0);
        // The published sequence dips well below one part-way through.
        let dip = (0..100)
            .map(|step| bar_opacity(step as f32 / 100.0))
            .fold(f32::MAX, f32::min);
        assert!(dip < 0.2, "the power-on should flicker, not ramp");
    }

    #[test]
    fn the_plate_flashes_and_clears() {
        assert_eq!(plate_opacity(0.0), 0.0);
        assert_eq!(plate_opacity(1.0), 0.0);
        let peak = (0..100)
            .map(|step| plate_opacity(step as f32 / 100.0))
            .fold(0.0f32, f32::max);
        assert!(peak > 0.5, "the plate should flash");
    }

    #[test]
    fn opacity_never_leaves_its_range() {
        for step in 0..200 {
            let progress = step as f32 / 100.0 - 0.5;
            assert!((0.0..=1.0).contains(&bar_opacity(progress)));
            assert!((0.0..=1.0).contains(&plate_opacity(progress)));
        }
    }

    #[test]
    fn the_splash_ends_and_stays_ended() {
        assert!(progress(Duration::ZERO).is_some());
        assert!(progress(Duration::from_millis(2219)).is_some());
        assert!(progress(DONE).is_none());
        assert!(progress(Duration::from_secs(60)).is_none());
    }

    #[test]
    fn the_wordmark_waits_then_arrives() {
        assert_eq!(progress(Duration::from_millis(500)).unwrap().wordmark, 0.0);
        assert_eq!(progress(Duration::from_millis(2100)).unwrap().wordmark, 1.0);
        let mid = progress(Duration::from_millis(1000)).unwrap().wordmark;
        assert!(mid > 0.0 && mid < 1.0);
    }

    #[test]
    fn the_overlay_only_fades_at_the_end() {
        assert_eq!(progress(Duration::from_millis(1900)).unwrap().fade, 0.0);
        assert!(progress(Duration::from_millis(2100)).unwrap().fade > 0.0);
    }
}
