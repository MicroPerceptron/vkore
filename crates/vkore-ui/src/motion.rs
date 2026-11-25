//! Motion: small marks that move with the clock.
//!
//! The desktop draws on demand, not on a timer: a frame happens when something
//! asks for the next one. Everything here asks only while it is actually
//! moving, and stops asking the frame it settles. That matters twice over. An
//! idle window should cost nothing, and a display list that changes every
//! frame defeats the retained backend for the whole window, so a moving mark
//! is kept small and a settled one is exactly the same list as before.
//!
//! The drawing is separate from the clock. Each mark paints from a time in
//! seconds, so the tests pin a moment instead of racing one.

use crate::{AxUi, Message, UiElement, theme};
use iced_core::{Color, Point, Rectangle, Size, Theme, mouse};
use iced_widget::canvas::{self, Action, Frame, Geometry, LineCap, Path, Stroke, path};
use std::{
    cell::Cell,
    f32::consts::TAU,
    sync::OnceLock,
    time::{Duration, Instant},
};

/// Seconds since the first mark was drawn. One epoch for the whole process, so
/// every pulse on screen beats together instead of each starting from its own
/// first frame.
pub fn seconds() -> f32 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_secs_f32()
}

/// Cubic ease-out: fast at first, settling. For things that arrive.
pub fn ease_out(progress: f32) -> f32 {
    let remaining = 1.0 - progress.clamp(0.0, 1.0);
    1.0 - remaining * remaining * remaining
}

/// A smooth 0..1..0 swell, `period` seconds long.
fn breath(t: f32, period: f32) -> f32 {
    0.5 - 0.5 * (t / period * TAU).cos()
}

fn with_alpha(color: Color, alpha: f32) -> Color {
    Color {
        a: color.a * alpha.clamp(0.0, 1.0),
        ..color
    }
}

/// What a status mark is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Beat {
    /// Nothing is happening: an outline.
    Hollow,
    /// A settled state: a filled dot.
    Solid,
    /// Alive: a dot with a ring that spreads and fades.
    Pulse,
    /// Working toward something: a turning arc.
    Spin,
}

impl Beat {
    fn moves(self) -> bool {
        matches!(self, Self::Pulse | Self::Spin)
    }
}

/// Paints a status mark into a square of side `size` at time `t`.
fn paint_beacon(frame: &mut Frame<AxUi>, size: f32, beat: Beat, color: Color, t: f32) {
    let centre = Point::new(size / 2.0, size / 2.0);
    let dot = size * 0.24;
    match beat {
        Beat::Hollow => frame.stroke(
            &Path::circle(centre, dot),
            Stroke::default()
                .with_color(color)
                .with_width((size * 0.1).max(1.2)),
        ),
        Beat::Solid => frame.fill(&Path::circle(centre, dot), color),
        Beat::Pulse => {
            frame.fill(&Path::circle(centre, dot), color);
            let phase = (t % 1.8) / 1.8;
            let reach = dot + phase * (size / 2.0 - dot - 0.5);
            frame.stroke(
                &Path::circle(centre, reach),
                Stroke::default()
                    .with_color(with_alpha(color, (1.0 - phase) * 0.6))
                    .with_width(1.4),
            );
        }
        Beat::Spin => {
            let radius = size * 0.36;
            frame.stroke(
                &Path::circle(centre, radius),
                Stroke::default()
                    .with_color(with_alpha(color, 0.22))
                    .with_width(1.6),
            );
            let start = (t / 0.9).fract() * TAU;
            frame.stroke(
                &Path::new(|p| {
                    p.arc(path::Arc {
                        center: centre,
                        radius,
                        start_angle: start.into(),
                        end_angle: (start + TAU * 0.28).into(),
                    });
                }),
                Stroke::default()
                    .with_color(color)
                    .with_width(1.9)
                    .with_line_cap(LineCap::Round),
            );
        }
    }
}

struct Beacon {
    beat: Beat,
    tone: theme::Tone,
}

impl canvas::Program<Message, Theme, AxUi> for Beacon {
    type State = ();

    fn update(
        &self,
        _: &mut (),
        event: &canvas::Event,
        _: Rectangle,
        _: mouse::Cursor,
    ) -> Option<Action<Message>> {
        (self.beat.moves() && is_frame(event)).then(Action::request_redraw)
    }

    fn draw(
        &self,
        _: &(),
        renderer: &AxUi,
        appearance: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<Geometry<AxUi>> {
        let mut frame = Frame::new(renderer, bounds.size());
        let t = if self.beat.moves() { seconds() } else { 0.0 };
        paint_beacon(
            &mut frame,
            bounds.width.min(bounds.height),
            self.beat,
            theme::color(appearance, self.tone),
            t,
        );
        vec![frame.into_geometry()]
    }
}

fn is_frame(event: &canvas::Event) -> bool {
    matches!(
        event,
        canvas::Event::Window(iced_core::window::Event::RedrawRequested(_))
    )
}

/// A status mark: a dot that is hollow, solid, pulsing or working.
pub fn beacon<'a>(beat: Beat, tone: theme::Tone, size: f32) -> UiElement<'a> {
    iced_widget::canvas(Beacon { beat, tone })
        .width(size)
        .height(size)
        .into()
}

/// How far a boot has come, as a row of segments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Steps {
    /// Segments in all.
    pub total: u8,
    /// Segments that are done.
    pub done: u8,
    /// Whether the next segment is being worked on.
    pub working: bool,
    pub tone: theme::Tone,
}

const SEGMENT_GAP: f32 = 2.0;

/// Paints the segments of `steps` across `bounds` at time `t`.
fn paint_steps(
    frame: &mut Frame<AxUi>,
    bounds: Size,
    steps: Steps,
    tone: Color,
    rest: Color,
    t: f32,
) {
    let total = f32::from(steps.total.max(1));
    let width = (bounds.width - SEGMENT_GAP * (total - 1.0)) / total;
    let radius = bounds.height / 2.0;
    for index in 0..steps.total {
        let color = if index < steps.done {
            tone
        } else if index == steps.done && steps.working {
            // The segment in progress breathes between its resting and full colour.
            let level = breath(t, 1.3);
            Color {
                a: rest.a + (tone.a - rest.a) * (0.25 + 0.75 * level),
                ..tone
            }
        } else {
            rest
        };
        let x = f32::from(index) * (width + SEGMENT_GAP);
        frame.fill(
            &Path::rounded_rectangle(
                Point::new(x, 0.0),
                Size::new(width, bounds.height),
                radius.into(),
            ),
            color,
        );
    }
}

struct StepsProgram(Steps);

impl canvas::Program<Message, Theme, AxUi> for StepsProgram {
    type State = ();

    fn update(
        &self,
        _: &mut (),
        event: &canvas::Event,
        _: Rectangle,
        _: mouse::Cursor,
    ) -> Option<Action<Message>> {
        (self.0.working && is_frame(event)).then(Action::request_redraw)
    }

    fn draw(
        &self,
        _: &(),
        renderer: &AxUi,
        appearance: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<Geometry<AxUi>> {
        let mut frame = Frame::new(renderer, bounds.size());
        paint_steps(
            &mut frame,
            bounds.size(),
            self.0,
            theme::color(appearance, self.0.tone),
            theme::rgba(appearance, theme::TEXT_MUTED, 0.28),
            if self.0.working { seconds() } else { 0.0 },
        );
        vec![frame.into_geometry()]
    }
}

/// A row of progress segments, `width` by `height` logical pixels.
pub fn steps<'a>(steps: Steps, width: f32, height: f32) -> UiElement<'a> {
    iced_widget::canvas(StepsProgram(steps))
        .width(width)
        .height(height)
        .into()
}

/// How long a gauge takes to sweep to its value when it appears.
const SWEEP: Duration = Duration::from_millis(650);

/// A ring that fills to a fraction.
struct Ring {
    fraction: f32,
    tone: theme::Tone,
}

/// When the ring first appeared. A ring that is rebuilt (its tile was shown
/// again, say) starts from nothing and sweeps once more; one that stays does
/// not.
#[derive(Default)]
struct Sweep {
    born: Cell<Option<Instant>>,
}

impl Sweep {
    fn progress(&self) -> f32 {
        let born = self.born.get().unwrap_or_else(|| {
            let now = Instant::now();
            self.born.set(Some(now));
            now
        });
        (born.elapsed().as_secs_f32() / SWEEP.as_secs_f32()).min(1.0)
    }
}

fn paint_ring(
    frame: &mut Frame<AxUi>,
    size: f32,
    fraction: f32,
    progress: f32,
    tone: Color,
    track: Color,
) {
    let width = (size * 0.1).max(2.5);
    let radius = (size - width) / 2.0;
    let centre = Point::new(size / 2.0, size / 2.0);
    frame.stroke(
        &Path::circle(centre, radius),
        Stroke::default().with_color(track).with_width(width),
    );
    // A gauge fed nonsense shows empty, never a broken arc.
    let fraction = if fraction.is_finite() {
        fraction.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let sweep = fraction * ease_out(progress);
    if sweep <= 0.0 {
        return;
    }
    let start = -TAU / 4.0;
    // A full ring is a circle: an arc that ends where it began draws nothing.
    let path = if sweep >= 0.999 {
        Path::circle(centre, radius)
    } else {
        Path::new(|p| {
            p.arc(path::Arc {
                center: centre,
                radius,
                start_angle: start.into(),
                end_angle: (start + TAU * sweep).into(),
            });
        })
    };
    frame.stroke(
        &path,
        Stroke::default()
            .with_color(tone)
            .with_width(width)
            .with_line_cap(LineCap::Round),
    );
}

impl canvas::Program<Message, Theme, AxUi> for Ring {
    type State = Sweep;

    fn update(
        &self,
        state: &mut Sweep,
        event: &canvas::Event,
        _: Rectangle,
        _: mouse::Cursor,
    ) -> Option<Action<Message>> {
        (is_frame(event) && state.progress() < 1.0).then(Action::request_redraw)
    }

    fn draw(
        &self,
        state: &Sweep,
        renderer: &AxUi,
        appearance: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<Geometry<AxUi>> {
        let mut frame = Frame::new(renderer, bounds.size());
        paint_ring(
            &mut frame,
            bounds.width.min(bounds.height),
            self.fraction,
            state.progress(),
            theme::color(appearance, self.tone),
            theme::rgba(appearance, theme::TEXT_MUTED, 0.28),
        );
        vec![frame.into_geometry()]
    }
}

/// A ring gauge, `size` pixels square, that sweeps to `fraction` when it appears.
pub fn ring<'a>(fraction: f32, tone: theme::Tone, size: f32) -> UiElement<'a> {
    iced_widget::canvas(Ring { fraction, tone })
        .width(size)
        .height(size)
        .into()
}

/// How long a hover takes to arrive or leave.
const GLIDE: Duration = Duration::from_millis(140);

/// A value that eases toward zero or one, remembering where it was when it
/// was told to turn round so a quick in-and-out never jumps.
#[derive(Default)]
struct Glide {
    from: Cell<f32>,
    to: Cell<f32>,
    since: Cell<Option<Instant>>,
}

impl Glide {
    fn level(&self, now: Instant) -> f32 {
        let Some(since) = self.since.get() else {
            return self.to.get();
        };
        let progress = now.saturating_duration_since(since).as_secs_f32() / GLIDE.as_secs_f32();
        self.from.get() + (self.to.get() - self.from.get()) * ease_out(progress)
    }

    /// Whether it has arrived.
    fn settled(&self, now: Instant) -> bool {
        self.since
            .get()
            .is_none_or(|since| now.saturating_duration_since(since) >= GLIDE)
    }

    /// Head for one (`on`) or zero.
    fn aim(&self, on: bool, now: Instant) {
        let target = f32::from(u8::from(on));
        if (self.to.get() - target).abs() > f32::EPSILON {
            self.from.set(self.level(now));
            self.to.set(target);
            self.since.set(Some(now));
        }
    }
}

/// A hover that eases in and out.
///
/// Wraps a control and, after it draws, lays a faint wash of one tone over it
/// that fades in as the pointer arrives and out as it leaves: the control
/// lifts instead of snapping. The wash goes on top, so it works on any fill
/// (a surface, an accent, nothing at all) and the control's own style carries
/// only what does not animate. Asks for frames only while it is moving.
pub struct Fade<'a> {
    content: UiElement<'a>,
    tone: theme::Tone,
    strength: f32,
    radius: f32,
    enabled: bool,
}

/// Wraps `content` so a wash of `tone`, up to `strength` opaque, eases over it
/// while the pointer is on it. A disabled control never washes.
pub fn fade<'a>(
    content: impl Into<UiElement<'a>>,
    tone: theme::Tone,
    strength: f32,
    radius: f32,
    enabled: bool,
) -> UiElement<'a> {
    UiElement::new(Fade {
        content: content.into(),
        tone,
        strength,
        radius,
        enabled,
    })
}

impl iced_core::Widget<Message, Theme, AxUi> for Fade<'_> {
    fn tag(&self) -> iced_core::widget::tree::Tag {
        iced_core::widget::tree::Tag::of::<Glide>()
    }

    fn state(&self) -> iced_core::widget::tree::State {
        iced_core::widget::tree::State::new(Glide::default())
    }

    fn diff(&mut self, tree: &mut iced_core::widget::Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.content));
    }

    fn size(&self) -> iced_core::Size<iced_core::Length> {
        self.content.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut iced_core::widget::Tree,
        renderer: &AxUi,
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

    fn operate(
        &mut self,
        tree: &mut iced_core::widget::Tree,
        layout: iced_core::Layout<'_>,
        renderer: &AxUi,
        operation: &mut dyn iced_core::widget::Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut iced_core::widget::Tree,
        event: &iced_core::Event,
        layout: iced_core::Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &AxUi,
        shell: &mut iced_core::Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        let glide = tree.state.downcast_ref::<Glide>();
        let now = Instant::now();
        glide.aim(
            self.enabled && cursor.is_over(layout.bounds()) && cursor.is_over(*viewport),
            now,
        );
        if !glide.settled(now) {
            shell.request_redraw();
        }
    }

    fn draw(
        &self,
        tree: &iced_core::widget::Tree,
        renderer: &mut AxUi,
        appearance: &Theme,
        style: &iced_core::renderer::Style,
        layout: iced_core::Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced_core::Renderer as _;
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            appearance,
            style,
            layout,
            cursor,
            viewport,
        );
        let level = tree
            .state
            .downcast_ref::<Glide>()
            .level(Instant::now())
            .clamp(0.0, 1.0);
        if level > 0.0 {
            renderer.fill_quad(
                iced_core::renderer::Quad {
                    bounds: layout.bounds(),
                    border: iced_core::Border {
                        radius: self.radius.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                theme::rgba(appearance, self.tone, self.strength * level),
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &iced_core::widget::Tree,
        layout: iced_core::Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &AxUi,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut iced_core::widget::Tree,
        layout: iced_core::Layout<'b>,
        renderer: &AxUi,
        viewport: &Rectangle,
        translation: iced_core::Vector,
    ) -> Option<iced_core::overlay::Element<'b, Message, Theme, AxUi>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced_graphics::geometry::Renderer as _;

    /// Records `paint` into a renderer and returns what the lane refused.
    fn refusals(paint: impl FnOnce(&mut Frame<AxUi>)) -> usize {
        let mut renderer = crate::native_renderer();
        let mut frame = Frame::new(&renderer, Size::new(40., 40.));
        paint(&mut frame);
        renderer.draw_geometry(frame.into_geometry());
        renderer.take_geometry_errors().len()
    }

    #[test]
    fn every_beat_paints_at_any_moment_without_a_refusal() {
        for beat in [Beat::Hollow, Beat::Solid, Beat::Pulse, Beat::Spin] {
            for t in [0.0, 0.4, 0.9, 1.79, 12.3] {
                let refused = refusals(|frame| {
                    paint_beacon(frame, 14., beat, theme::rgb(0xCE15C4), t);
                });
                assert_eq!(refused, 0, "{beat:?} at {t}");
            }
        }
    }

    /// Whether a view, given a frame, asks for another. The retained backend
    /// and an idle window both depend on this being false at rest.
    fn asks_for_the_next_frame(
        element: impl Fn() -> UiElement<'static>,
        cache: &mut Option<iced_runtime::user_interface::Cache>,
    ) -> bool {
        asks_for_the_next_frame_at(element, cache, mouse::Cursor::Unavailable)
    }

    /// As [`asks_for_the_next_frame`], with the pointer somewhere.
    fn asks_for_the_next_frame_at(
        element: impl Fn() -> UiElement<'static>,
        cache: &mut Option<iced_runtime::user_interface::Cache>,
        cursor: mouse::Cursor,
    ) -> bool {
        use iced_core::{Event, Renderer as _, shell};
        use iced_runtime::user_interface::{State, UserInterface};
        let size = Size::new(100., 100.);
        let mut renderer = crate::native_renderer();
        renderer.reset(Rectangle::with_size(size));
        let mut interface = UserInterface::build(
            element(),
            size,
            cache.take().unwrap_or_default(),
            &mut renderer,
        );
        let (state, _) = interface.update(
            &iced_core::window::Headless,
            &shell::Waker::noop(),
            &[Event::Window(iced_core::window::Event::RedrawRequested(
                Instant::now(),
            ))],
            cursor,
            &mut renderer,
            &mut shell::Bus::new(),
        );
        *cache = Some(interface.into_cache());
        matches!(
            state,
            State::Updated {
                redraw_request: iced_core::window::RedrawRequest::NextFrame,
                ..
            }
        )
    }

    #[test]
    fn a_mark_asks_for_frames_only_while_it_moves() {
        for (beat, moves) in [
            (Beat::Hollow, false),
            (Beat::Solid, false),
            (Beat::Pulse, true),
            (Beat::Spin, true),
        ] {
            assert_eq!(
                asks_for_the_next_frame(|| beacon(beat, theme::SUCCESS, 14.), &mut None),
                moves,
                "{beat:?}"
            );
        }
        for working in [false, true] {
            let progress = Steps {
                total: 5,
                done: 2,
                working,
                tone: theme::SUCCESS,
            };
            assert_eq!(
                asks_for_the_next_frame(|| steps(progress, 64., 6.), &mut None),
                working
            );
        }
    }

    #[test]
    fn a_ring_sweeps_when_it_appears_then_asks_for_nothing() {
        let ring = || ring(0.5, theme::MAGENTA, 64.);
        let mut cache = None;
        assert!(asks_for_the_next_frame(ring, &mut cache), "sweeping");
        std::thread::sleep(SWEEP + Duration::from_millis(80));
        assert!(
            !asks_for_the_next_frame(ring, &mut cache),
            "a settled ring keeps the window drawing"
        );
    }

    #[test]
    fn a_glide_eases_toward_its_target_and_reverses_without_a_jump() {
        let glide = Glide::default();
        let t0 = Instant::now();
        assert_eq!(glide.level(t0), 0.0);
        assert!(glide.settled(t0));
        glide.aim(true, t0);
        assert!(!glide.settled(t0));
        let halfway = glide.level(t0 + GLIDE / 2);
        assert!(
            halfway > 0.5 && halfway < 1.0,
            "ease-out is fast first: {halfway}"
        );
        assert!(glide.settled(t0 + GLIDE));
        assert_eq!(glide.level(t0 + GLIDE * 3), 1.0);
        // Turning round mid-flight starts from where it is, not from an end.
        let turn = t0 + GLIDE / 2;
        let at_turn = glide.level(turn);
        glide.aim(false, turn);
        assert!((glide.level(turn) - at_turn).abs() < 1e-6);
        assert_eq!(glide.level(turn + GLIDE * 3), 0.0);
        // Aiming where it already is changes nothing.
        let before = glide.level(turn + GLIDE * 3);
        glide.aim(false, turn + GLIDE * 4);
        assert_eq!(glide.level(turn + GLIDE * 4), before);
        assert!(glide.settled(turn + GLIDE * 4));
    }

    #[test]
    fn a_hover_eases_in_asks_for_frames_while_it_moves_then_goes_quiet() {
        let button = || {
            fade(
                iced_widget::container(iced_widget::space::vertical())
                    .width(60)
                    .height(30),
                theme::TEXT,
                0.1,
                8.,
                true,
            )
        };
        let mut cache = None;
        let over = mouse::Cursor::Available(Point::new(10., 10.));
        let away = mouse::Cursor::Available(Point::new(90., 90.));
        assert!(
            !asks_for_the_next_frame_at(button, &mut cache, away),
            "an untouched control is still"
        );
        assert!(
            asks_for_the_next_frame_at(button, &mut cache, over),
            "arriving"
        );
        std::thread::sleep(GLIDE + Duration::from_millis(60));
        assert!(
            !asks_for_the_next_frame_at(button, &mut cache, over),
            "a hovered control at rest keeps the window drawing"
        );
        assert!(
            asks_for_the_next_frame_at(button, &mut cache, away),
            "leaving"
        );
    }

    #[test]
    fn a_disabled_control_never_washes() {
        let button = || {
            fade(
                iced_widget::container(iced_widget::space::vertical())
                    .width(60)
                    .height(30),
                theme::TEXT,
                0.1,
                8.,
                false,
            )
        };
        let over = mouse::Cursor::Available(Point::new(10., 10.));
        assert!(!asks_for_the_next_frame_at(button, &mut None, over));
    }

    #[test]
    fn only_pulsing_and_working_marks_move() {
        assert!(Beat::Pulse.moves() && Beat::Spin.moves());
        assert!(!Beat::Hollow.moves() && !Beat::Solid.moves());
    }

    #[test]
    fn steps_paint_every_segment_whatever_the_progress() {
        for done in 0..=5 {
            for working in [false, true] {
                let steps = Steps {
                    total: 5,
                    done,
                    working,
                    tone: theme::SUCCESS,
                };
                let refused = refusals(|frame| {
                    paint_steps(
                        frame,
                        Size::new(64., 6.),
                        steps,
                        theme::rgb(0x3FCB6E),
                        theme::rgb(0x2A2A30),
                        0.7,
                    );
                });
                assert_eq!(refused, 0, "{done} done, working {working}");
            }
        }
    }

    #[test]
    fn a_ring_paints_empty_partial_and_full() {
        for fraction in [0.0, 0.01, 0.5, 0.999, 1.0, 7.0, f32::NAN] {
            for progress in [0.0, 0.5, 1.0] {
                let refused = refusals(|frame| {
                    paint_ring(
                        frame,
                        40.,
                        fraction,
                        progress,
                        theme::rgb(0xCE15C4),
                        theme::rgb(0x2A2A30),
                    );
                });
                assert_eq!(refused, 0, "{fraction} at {progress}");
            }
        }
    }

    #[test]
    fn easing_runs_from_nothing_to_everything_and_never_overshoots() {
        assert_eq!(ease_out(0.0), 0.0);
        assert_eq!(ease_out(1.0), 1.0);
        assert_eq!(ease_out(5.0), 1.0);
        let mut last = 0.0;
        for step in 1..=20 {
            let eased = ease_out(step as f32 / 20.0);
            assert!(eased >= last);
            last = eased;
        }
    }

    #[test]
    fn a_breath_swells_and_returns() {
        assert!(breath(0.0, 2.0).abs() < 1e-6);
        assert!((breath(1.0, 2.0) - 1.0).abs() < 1e-6);
        assert!(breath(2.0, 2.0).abs() < 1e-5);
    }
}
