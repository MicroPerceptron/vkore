//! Which pixels changed between two frames.
//!
//! The raster takes exact damage rectangles, and a frame that changed by a
//! spinner's few pixels should not repaint the window. Axiom does not diff
//! frames, so this does, and it is built to be wrong only in the safe
//! direction: a rectangle may cover more than changed, never less.
//!
//! A display list is painter-ordered, so the pixels that differ are the ones
//! some *unmatched* drawing touches. The two lists are aligned (a longest common
//! subsequence of drawings, each compared by what it draws and the clip,
//! transform and opacity it draws under); every drawing left unmatched, in
//! either frame, damages its own bounds. A pixel no unmatched drawing touches
//! is drawn by the same drawings in the same order in both frames, so it is the
//! same colour. Anything this cannot reason about (a backdrop blur, whose
//! pixels depend on the pixels beneath it; a transform it does not know; a
//! change too large to be worth a diff) is a full repaint.

use axgfx::{Extent2d, PixelRect};
use axui_core::{Command, Frame, Glyph, GlyphRun, Image, Quad, Rect, Transform};
use axui_iced::PublishedImage;
use std::alloc::System;

/// What a new frame changed against the one on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Change {
    /// Nothing visible differs.
    Unchanged,
    /// Repaint everything.
    Full,
    /// Repaint exactly these disjoint rectangles.
    Rects(Vec<PixelRect>),
}

/// A frame and the raster resources it names.
pub(crate) struct Snapshot<'a> {
    pub(crate) frame: &'a Frame<System>,
    pub(crate) images: &'a [PublishedImage],
}

/// The most edits a diff searches for before it gives up and repaints. A page
/// that changed this much is not a small change, and the search is quadratic.
const MAX_EDITS: usize = 192;
/// The most rectangles handed to the raster; more are merged into their bounds.
const MAX_RECTS: usize = 24;
/// Rectangles closer than this many pixels are one rectangle: the raster pays
/// per region, and a gap this small is cheaper painted than skipped.
const MERGE_GAP: i64 = 8;
/// Past this share of the target, a full repaint is the cheaper way.
const FULL_SHARE: f64 = 0.6;
/// Pixels around a drawing that its antialiased edges may touch.
const EDGE: f32 = 2.0;
/// Pixels around a shadow that the raster's falloff may touch, as it bounds them.
const SHADOW_REACH: f32 = 16.0;

pub(crate) fn diff(old: Snapshot<'_>, new: Snapshot<'_>, extent: Extent2d) -> Change {
    let (o, n) = (old.frame, new.frame);
    if o.bounds() != n.bounds()
        || o.scale_factor() != n.scale_factor()
        || o.antialiasing() != n.antialiasing()
    {
        return Change::Full;
    }
    let mut contexts = Contexts::default();
    let (Some(a), Some(b)) = (
        walk(o, extent, &mut contexts),
        walk(n, extent, &mut contexts),
    ) else {
        return Change::Full;
    };
    let same = |i: usize, j: usize| same_drawing(&old, &a.items[i], &new, &b.items[j]);

    // Most frames differ in a few places: trim what both ends share first.
    let (len_a, len_b) = (a.items.len(), b.items.len());
    let mut start = 0;
    while start < len_a && start < len_b && same(start, start) {
        start += 1;
    }
    let (mut end_a, mut end_b) = (len_a, len_b);
    while end_a > start && end_b > start && same(end_a - 1, end_b - 1) {
        end_a -= 1;
        end_b -= 1;
    }
    let (span_a, span_b) = (end_a - start, end_b - start);
    if span_a == 0 && span_b == 0 {
        return Change::Unchanged;
    }
    let Some((deleted, inserted)) = edits(span_a, span_b, &|i, j| same(start + i, start + j))
    else {
        return Change::Full;
    };
    let mut damage: Vec<PixelRect> = Vec::new();
    damage.extend(
        deleted
            .iter()
            .enumerate()
            .filter(|(_, gone)| **gone)
            .filter_map(|(i, _)| a.items[start + i].bounds),
    );
    damage.extend(
        inserted
            .iter()
            .enumerate()
            .filter(|(_, added)| **added)
            .filter_map(|(j, _)| b.items[start + j].bounds),
    );
    shape(damage, extent)
}

/// One drawing, with where it lands.
struct Item {
    draw: Draw,
    /// The clips, transforms and opacity groups it is drawn under, interned
    /// across both frames so that equal contexts are equal numbers.
    context: usize,
    /// Everything it can touch, in physical pixels; `None` if it touches none.
    bounds: Option<PixelRect>,
}

#[derive(Clone, Copy)]
enum Draw {
    Quad(usize),
    Glyphs(usize),
    Image(usize),
}

struct Walk {
    items: Vec<Item>,
}

/// The set of distinct contexts seen in either frame.
#[derive(Default)]
struct Contexts(Vec<Vec<Command>>);

impl Contexts {
    fn id(&mut self, stack: &[Command]) -> usize {
        if let Some(found) = self.0.iter().position(|known| known == stack) {
            return found;
        }
        self.0.push(stack.to_vec());
        self.0.len() - 1
    }
}

/// A 2D affine map, composed exactly as the raster composes it.
#[derive(Clone, Copy)]
struct Affine {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    tx: f32,
    ty: f32,
}

impl Affine {
    fn base(bounds: Rect, scale: f32) -> Self {
        Self {
            a: scale,
            b: 0.0,
            c: 0.0,
            d: scale,
            tx: -bounds.x * scale,
            ty: -bounds.y * scale,
        }
    }

    /// `None` for anything but a plain 2D transform.
    fn from_transform(transform: Transform) -> Option<Self> {
        let m = transform.0;
        let epsilon = 0.000_01;
        let flat = [m[2], m[3], m[6], m[7], m[8], m[9], m[11], m[14]];
        if flat.iter().any(|v| v.abs() > epsilon)
            || (m[10] - 1.0).abs() > epsilon
            || (m[15] - 1.0).abs() > epsilon
        {
            return None;
        }
        Some(Self {
            a: m[0],
            b: m[1],
            c: m[4],
            d: m[5],
            tx: m[12],
            ty: m[13],
        })
    }

    /// `next` applied first, then `self`.
    fn then(self, next: Self) -> Self {
        Self {
            a: self.a * next.a + self.c * next.b,
            b: self.b * next.a + self.d * next.b,
            c: self.a * next.c + self.c * next.d,
            d: self.b * next.c + self.d * next.d,
            tx: self.a * next.tx + self.c * next.ty + self.tx,
            ty: self.b * next.tx + self.d * next.ty + self.ty,
        }
    }

    fn point(self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.tx,
            self.b * x + self.d * y + self.ty,
        )
    }

    /// How far a logical distance can stretch, at most.
    fn reach(self) -> f32 {
        (self.a.hypot(self.b)).max(self.c.hypot(self.d))
    }
}

/// The union of two logical rectangles.
fn union(a: Rect, b: Rect) -> Rect {
    let (x0, y0) = (a.x.min(b.x), a.y.min(b.y));
    let (x1, y1) = (
        (a.x + a.width).max(b.x + b.width),
        (a.y + a.height).max(b.y + b.height),
    );
    Rect {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    }
}

/// `rect` under `affine`, grown by `margin` physical pixels, as the pixels it
/// can touch inside the target. A rectangle that is not finite is the whole
/// target: wrong in the safe direction.
fn physical(rect: Rect, affine: Affine, margin: f32, extent: Extent2d) -> Option<PixelRect> {
    let whole = PixelRect::full(extent);
    let corners = [
        affine.point(rect.x, rect.y),
        affine.point(rect.x + rect.width, rect.y),
        affine.point(rect.x, rect.y + rect.height),
        affine.point(rect.x + rect.width, rect.y + rect.height),
    ];
    if corners
        .iter()
        .any(|(x, y)| !x.is_finite() || !y.is_finite())
        || !margin.is_finite()
    {
        return Some(whole);
    }
    let min_x = corners.iter().map(|c| c.0).fold(f32::INFINITY, f32::min) - margin;
    let min_y = corners.iter().map(|c| c.1).fold(f32::INFINITY, f32::min) - margin;
    let max_x = corners
        .iter()
        .map(|c| c.0)
        .fold(f32::NEG_INFINITY, f32::max)
        + margin;
    let max_y = corners
        .iter()
        .map(|c| c.1)
        .fold(f32::NEG_INFINITY, f32::max)
        + margin;
    let (width, height) = (extent.width() as f32, extent.height() as f32);
    let (x0, y0) = (
        min_x.floor().clamp(0.0, width),
        min_y.floor().clamp(0.0, height),
    );
    let (x1, y1) = (
        max_x.ceil().clamp(0.0, width),
        max_y.ceil().clamp(0.0, height),
    );
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    PixelRect::new(x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32).ok()
}

/// A frame's drawings in order, or `None` if it holds something this cannot
/// reason about.
fn walk(frame: &Frame<System>, extent: Extent2d, contexts: &mut Contexts) -> Option<Walk> {
    let mut items = Vec::new();
    let mut stack: Vec<Command> = Vec::new();
    let mut saved: Vec<Affine> = Vec::new();
    let mut affine = Affine::base(frame.bounds(), frame.scale_factor());
    let mut context = contexts.id(&stack);
    for command in frame.commands() {
        match command {
            Command::PushClip(_) | Command::PushOpacity { .. } => {
                stack.push(*command);
                context = contexts.id(&stack);
            }
            Command::PushTransform(transform) => {
                saved.push(affine);
                affine = affine.then(Affine::from_transform(*transform)?);
                stack.push(*command);
                context = contexts.id(&stack);
            }
            Command::PopClip | Command::PopOpacity => {
                stack.pop()?;
                context = contexts.id(&stack);
            }
            Command::PopTransform => {
                stack.pop()?;
                affine = saved.pop()?;
                context = contexts.id(&stack);
            }
            // Its pixels depend on the pixels under it, so damage under it
            // would have to reach beyond the damage.
            Command::BackdropBlur { .. } | Command::DrawField(_) => return None,
            Command::DrawQuad(index) => {
                let quad = frame.quads().get(*index)?;
                let mut rect = quad.bounds;
                let mut margin = EDGE;
                if quad.shadow.color.a > 0.0 {
                    let mut shifted = rect;
                    shifted.x += quad.shadow.offset.x;
                    shifted.y += quad.shadow.offset.y;
                    rect = union(rect, shifted);
                    margin +=
                        SHADOW_REACH + (quad.shadow.blur_radius.max(0.0) * affine.reach()).ceil();
                }
                items.push(Item {
                    draw: Draw::Quad(*index),
                    context,
                    bounds: physical(rect, affine, margin, extent),
                });
            }
            Command::DrawGlyphRun(index) => {
                let run = frame.glyph_runs().get(*index)?;
                let glyphs = run_glyphs(frame, run)?;
                let rect = glyphs
                    .iter()
                    .fold(run.bounds, |rect, glyph| union(rect, glyph.bounds));
                items.push(Item {
                    draw: Draw::Glyphs(*index),
                    context,
                    bounds: physical(rect, affine, EDGE + 1.0, extent),
                });
            }
            Command::DrawImage(index) => {
                let image = frame.images().get(*index)?;
                items.push(Item {
                    draw: Draw::Image(*index),
                    context,
                    bounds: physical(image.bounds, affine, EDGE, extent),
                });
            }
        }
    }
    (stack.is_empty() && saved.is_empty()).then_some(Walk { items })
}

fn run_glyphs<'a>(frame: &'a Frame<System>, run: &GlyphRun) -> Option<&'a [Glyph]> {
    let start = run.glyph_start as usize;
    frame
        .glyphs()
        .get(start..start.checked_add(run.glyph_len as usize)?)
}

/// Whether two drawings, one from each frame, draw the same pixels.
fn same_drawing(old: &Snapshot<'_>, a: &Item, new: &Snapshot<'_>, b: &Item) -> bool {
    if a.context != b.context {
        return false;
    }
    match (a.draw, b.draw) {
        (Draw::Quad(i), Draw::Quad(j)) => {
            let (x, y): (&Quad, &Quad) = (&old.frame.quads()[i], &new.frame.quads()[j]);
            x == y
        }
        (Draw::Glyphs(i), Draw::Glyphs(j)) => {
            let (x, y) = (&old.frame.glyph_runs()[i], &new.frame.glyph_runs()[j]);
            // The run names its glyphs by position in a table that is rebuilt
            // each frame: compare the glyphs, not the position.
            x.bounds == y.bounds
                && x.font == y.font
                && x.face_index == y.face_index
                && x.size == y.size
                && x.color == y.color
                && x.glyph_len == y.glyph_len
                && run_glyphs(old.frame, x) == run_glyphs(new.frame, y)
        }
        (Draw::Image(i), Draw::Image(j)) => {
            let (x, y) = (&old.frame.images()[i], &new.frame.images()[j]);
            same_image(old, x, new, y)
        }
        _ => false,
    }
}

/// Two image draws are the same when everything about the draw is, and the
/// pixels they name are. A resource's identity alone is not enough: a canvas
/// that is not cached publishes fresh coverage, under a fresh identity, every
/// frame, so identical icons carry different ids; and an identity could be
/// kept while its bytes changed. Compare the bytes.
fn same_image(old: &Snapshot<'_>, x: &Image, new: &Snapshot<'_>, y: &Image) -> bool {
    if x.source != y.source || x.bounds != y.bounds || x.sampling != y.sampling || x.tint != y.tint
    {
        return false;
    }
    match (published(old, x), published(new, y)) {
        (Some(p), Some(q)) => {
            let (d, e) = (&p.descriptor, &q.descriptor);
            d.kind == e.kind
                && d.format == e.format
                && d.width == e.width
                && d.height == e.height
                && d.row_stride == e.row_stride
                && d.byte_len == e.byte_len
                && (std::ptr::eq(p.bytes.as_ptr(), q.bytes.as_ptr())
                    && p.bytes.len() == q.bytes.len()
                    || p.bytes.as_ref() == q.bytes.as_ref())
        }
        // A resource the frame did not publish (an installed one): its
        // identity is its content.
        (None, None) => {
            x.resource == y.resource
                && old.frame.resources().iter().find(|r| r.id == x.resource)
                    == new.frame.resources().iter().find(|r| r.id == y.resource)
        }
        _ => false,
    }
}

/// The raster resource a snapshot published for an image draw, if it did.
fn published<'a>(snapshot: &'a Snapshot<'_>, image: &Image) -> Option<&'a PublishedImage> {
    snapshot
        .images
        .iter()
        .find(|published| published.descriptor.id == image.resource)
}

/// Which items of `a` (length `n`) and `b` (length `m`) a shortest edit script
/// deletes and inserts, by Myers' algorithm; `None` past [`MAX_EDITS`].
fn edits(
    n: usize,
    m: usize,
    same: &dyn Fn(usize, usize) -> bool,
) -> Option<(Vec<bool>, Vec<bool>)> {
    let max_d = MAX_EDITS.min(n + m);
    let offset = max_d + 1;
    let mut v = vec![0_isize; 2 * max_d + 3];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut found = None;
    'search: for d in 0..=max_d as isize {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            let at = (k + offset as isize) as usize;
            let mut x = if k == -d || (k != d && v[at - 1] < v[at + 1]) {
                v[at + 1]
            } else {
                v[at - 1] + 1
            };
            let mut y = x - k;
            while (x as usize) < n && (y as usize) < m && same(x as usize, y as usize) {
                x += 1;
                y += 1;
            }
            v[at] = x;
            if x as usize >= n && y as usize >= m {
                found = Some(d);
                break 'search;
            }
            k += 2;
        }
    }
    let last = found?;
    let mut deleted = vec![false; n];
    let mut inserted = vec![false; m];
    let (mut x, mut y) = (n as isize, m as isize);
    for d in (0..=last).rev() {
        let v = &trace[d as usize];
        let k = x - y;
        let at = |k: isize| (k + offset as isize) as usize;
        let prev_k = if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = v[at(prev_k)];
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            if x == prev_x {
                inserted[prev_y as usize] = true;
            } else {
                deleted[prev_x as usize] = true;
            }
        }
        x = prev_x;
        y = prev_y;
    }
    Some((deleted, inserted))
}

/// Turns the damaged rectangles of a diff into what is handed to the raster:
/// disjoint, few, and a full repaint when most of the target is damaged.
fn shape(mut rects: Vec<PixelRect>, extent: Extent2d) -> Change {
    if rects.is_empty() {
        return Change::Unchanged;
    }
    // Merge any two that overlap or nearly touch, until none do.
    loop {
        let mut merged = false;
        'pairs: for i in 0..rects.len() {
            for j in i + 1..rects.len() {
                if near(rects[i], rects[j]) {
                    rects[i] = bound(rects[i], rects[j]);
                    rects.swap_remove(j);
                    merged = true;
                    break 'pairs;
                }
            }
        }
        if !merged {
            break;
        }
    }
    if rects.len() > MAX_RECTS {
        let all = rects.iter().copied().reduce(bound).expect("nonempty");
        rects = vec![all];
    }
    let area: u64 = rects
        .iter()
        .map(|r| u64::from(r.width()) * u64::from(r.height()))
        .sum();
    let target = u64::from(extent.width()) * u64::from(extent.height());
    if area as f64 > target as f64 * FULL_SHARE {
        return Change::Full;
    }
    rects.sort_by_key(|r| (r.y(), r.x()));
    Change::Rects(rects)
}

fn near(a: PixelRect, b: PixelRect) -> bool {
    let (a_x1, a_y1) = (
        i64::from(a.x()) + i64::from(a.width()),
        i64::from(a.y()) + i64::from(a.height()),
    );
    let (b_x1, b_y1) = (
        i64::from(b.x()) + i64::from(b.width()),
        i64::from(b.y()) + i64::from(b.height()),
    );
    i64::from(a.x()) - MERGE_GAP <= b_x1
        && i64::from(b.x()) - MERGE_GAP <= a_x1
        && i64::from(a.y()) - MERGE_GAP <= b_y1
        && i64::from(b.y()) - MERGE_GAP <= a_y1
}

fn bound(a: PixelRect, b: PixelRect) -> PixelRect {
    let (x0, y0) = (a.x().min(b.x()), a.y().min(b.y()));
    let x1 = (a.x() + a.width()).max(b.x() + b.width());
    let y1 = (a.y() + a.height()).max(b.y() + b.height());
    PixelRect::new(x0, y0, x1 - x0, y1 - y0).expect("a union is nonempty")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: u32, y: u32, w: u32, h: u32) -> PixelRect {
        PixelRect::new(x, y, w, h).unwrap()
    }

    fn extent() -> Extent2d {
        Extent2d::new(1000, 800).unwrap()
    }

    #[test]
    fn myers_finds_the_few_edits_between_two_sequences() {
        let a = [1, 2, 3, 4, 5, 6, 7];
        let b = [1, 2, 9, 4, 5, 7, 8];
        let (deleted, inserted) = edits(a.len(), b.len(), &|i, j| a[i] == b[j]).unwrap();
        // 3 and 6 go; 9 and 8 come. Everything else is matched.
        assert_eq!(deleted, [false, false, true, false, false, true, false]);
        assert_eq!(inserted, [false, false, true, false, false, false, true]);
    }

    #[test]
    fn myers_handles_empty_sides_identical_and_disjoint_sequences() {
        let same = |_: usize, _: usize| true;
        let never = |_: usize, _: usize| false;
        assert_eq!(edits(0, 0, &same), Some((vec![], vec![])));
        assert_eq!(edits(3, 0, &never), Some((vec![true; 3], vec![])));
        assert_eq!(edits(0, 2, &never), Some((vec![], vec![true; 2])));
        assert_eq!(edits(3, 3, &same), Some((vec![false; 3], vec![false; 3])));
        let (deleted, inserted) = edits(3, 4, &never).unwrap();
        assert!(deleted.iter().all(|d| *d) && inserted.iter().all(|i| *i));
    }

    #[test]
    fn myers_gives_up_when_the_change_is_too_large_to_be_worth_a_search() {
        let never = |_: usize, _: usize| false;
        assert_eq!(edits(MAX_EDITS, MAX_EDITS, &never), None);
    }

    #[test]
    fn near_rectangles_merge_and_distant_ones_do_not() {
        assert_eq!(
            shape(vec![rect(0, 0, 10, 10), rect(12, 0, 10, 10)], extent()),
            Change::Rects(vec![rect(0, 0, 22, 10)])
        );
        assert_eq!(
            shape(vec![rect(0, 0, 10, 10), rect(100, 100, 10, 10)], extent()),
            Change::Rects(vec![rect(0, 0, 10, 10), rect(100, 100, 10, 10)])
        );
        // Overlap chains through a third.
        let chained = shape(
            vec![rect(0, 0, 20, 20), rect(60, 0, 20, 20), rect(15, 0, 50, 5)],
            extent(),
        );
        assert_eq!(chained, Change::Rects(vec![rect(0, 0, 80, 20)]));
    }

    #[test]
    fn the_rectangles_handed_on_are_disjoint_few_and_never_most_of_the_target() {
        assert_eq!(shape(vec![], extent()), Change::Unchanged);
        let many: Vec<_> = (0..40).map(|i| rect(i * 25, (i % 3) * 200, 5, 5)).collect();
        let Change::Rects(rects) = shape(many, extent()) else {
            panic!("a few small areas are not most of the target");
        };
        assert!(rects.len() <= MAX_RECTS);
        for (i, a) in rects.iter().enumerate() {
            for b in &rects[i + 1..] {
                assert!(!near(*a, *b), "{a:?} and {b:?} touch");
            }
        }
        assert_eq!(
            shape(vec![rect(0, 0, 900, 700)], extent()),
            Change::Full,
            "most of the target is a full repaint"
        );
    }

    #[test]
    fn a_rectangle_is_clamped_to_the_target_and_a_broken_one_is_the_whole_target() {
        let identity = Affine::base(
            Rect {
                x: 0.0,
                y: 0.0,
                width: 1000.0,
                height: 800.0,
            },
            1.0,
        );
        let off = Rect {
            x: 990.0,
            y: 790.0,
            width: 100.0,
            height: 100.0,
        };
        let clamped = physical(off, identity, 0.0, extent()).unwrap();
        assert_eq!(
            (clamped.x(), clamped.y(), clamped.width(), clamped.height()),
            (990, 790, 10, 10)
        );
        let outside = Rect {
            x: -500.0,
            y: -500.0,
            width: 10.0,
            height: 10.0,
        };
        assert_eq!(physical(outside, identity, 1.0, extent()), None);
        let broken = Rect {
            x: f32::NAN,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        };
        assert_eq!(
            physical(broken, identity, 0.0, extent()),
            Some(PixelRect::full(extent()))
        );
    }

    #[test]
    fn a_child_transform_applies_before_its_parent() {
        let parent = Affine {
            a: 2.0,
            b: 0.0,
            c: 0.0,
            d: 2.0,
            tx: 10.0,
            ty: 0.0,
        };
        let child = Affine {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            tx: 5.0,
            ty: 0.0,
        };
        // (0,0) -> child -> (5,0) -> parent -> (20,0).
        assert_eq!(parent.then(child).point(0.0, 0.0), (20.0, 0.0));
    }

    // ---- the proof: incremental painting equals a full repaint -------------

    use crate::render_backend::{NativeBackend, Painted};
    use axui_core::{BlendMode, Border, Color, CornerRadii, Paint, Point, Recorder, Shadow};

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: u32) -> u32 {
            (self.next() % u64::from(n)) as u32
        }
        fn unit(&mut self) -> f32 {
            (self.next() % 10_000) as f32 / 10_000.0
        }
    }

    #[derive(Clone)]
    enum Piece {
        Quad {
            rect: Rect,
            color: [f32; 4],
            radius: f32,
            border: f32,
            shadow: bool,
        },
        Group {
            opacity: f32,
            children: Vec<Piece>,
        },
        Clip {
            rect: Rect,
            children: Vec<Piece>,
        },
        Move {
            dx: f32,
            dy: f32,
            scale: f32,
            children: Vec<Piece>,
        },
    }

    const WIDTH: f32 = 320.0;
    const HEIGHT: f32 = 240.0;

    fn random_rect(rng: &mut Rng) -> Rect {
        Rect {
            x: rng.unit() * (WIDTH - 40.0),
            y: rng.unit() * (HEIGHT - 40.0),
            width: 8.0 + rng.unit() * 120.0,
            height: 8.0 + rng.unit() * 90.0,
        }
    }

    fn random_piece(rng: &mut Rng, depth: u32) -> Piece {
        match if depth >= 2 { 0 } else { rng.below(10) } {
            0..=6 => Piece::Quad {
                rect: random_rect(rng),
                color: [rng.unit(), rng.unit(), rng.unit(), 0.4 + rng.unit() * 0.6],
                radius: rng.unit() * 12.0,
                border: if rng.below(3) == 0 { 1.0 } else { 0.0 },
                shadow: rng.below(4) == 0,
            },
            7 => Piece::Group {
                opacity: 0.3 + rng.unit() * 0.7,
                children: (0..1 + rng.below(3))
                    .map(|_| random_piece(rng, depth + 1))
                    .collect(),
            },
            8 => Piece::Clip {
                rect: random_rect(rng),
                children: (0..1 + rng.below(3))
                    .map(|_| random_piece(rng, depth + 1))
                    .collect(),
            },
            _ => Piece::Move {
                dx: rng.unit() * 60.0 - 30.0,
                dy: rng.unit() * 60.0 - 30.0,
                scale: 0.6 + rng.unit() * 0.8,
                children: (0..1 + rng.below(3))
                    .map(|_| random_piece(rng, depth + 1))
                    .collect(),
            },
        }
    }

    fn record(recorder: &mut Recorder<System>, piece: &Piece) {
        match piece {
            Piece::Quad {
                rect,
                color,
                radius,
                border,
                shadow,
            } => recorder.draw_quad(Quad {
                bounds: *rect,
                paint: Paint::Solid(Color::new(color[0], color[1], color[2], color[3])),
                border: Border {
                    color: Color::new(0.0, 0.0, 0.0, 0.8),
                    width: *border,
                    radii: CornerRadii {
                        top_left: *radius,
                        top_right: *radius,
                        bottom_right: *radius,
                        bottom_left: *radius,
                    },
                },
                shadow: if *shadow {
                    Shadow {
                        color: Color::new(0.0, 0.0, 0.0, 0.4),
                        offset: Point { x: 0.0, y: 2.0 },
                        blur_radius: 3.0,
                    }
                } else {
                    Shadow::default()
                },
                snap: false,
            }),
            Piece::Group { opacity, children } => {
                recorder.push_opacity(*opacity, BlendMode::SourceOver);
                children.iter().for_each(|c| record(recorder, c));
                recorder.pop_opacity();
            }
            Piece::Clip { rect, children } => {
                recorder.push_clip(*rect);
                children.iter().for_each(|c| record(recorder, c));
                recorder.pop_clip();
            }
            Piece::Move {
                dx,
                dy,
                scale,
                children,
            } => {
                let mut m = Transform::default();
                m.0[0] = *scale;
                m.0[5] = *scale;
                m.0[12] = *dx;
                m.0[13] = *dy;
                recorder.push_transform(m);
                children.iter().for_each(|c| record(recorder, c));
                recorder.pop_transform();
            }
        }
    }

    fn frame_of(scene: &[Piece]) -> Frame<System> {
        frame_sized(scene, WIDTH, HEIGHT)
    }

    fn frame_sized(scene: &[Piece], width: f32, height: f32) -> Frame<System> {
        let bounds = Rect {
            x: 0.0,
            y: 0.0,
            width,
            height,
        };
        let mut recorder = Recorder::new_in(bounds, 1.0, System);
        scene.iter().for_each(|piece| record(&mut recorder, piece));
        recorder.take_frame().expect("a valid frame")
    }

    fn count(pieces: &[Piece]) -> usize {
        pieces
            .iter()
            .map(|piece| match piece {
                Piece::Quad { .. } => 1,
                Piece::Group { children, .. }
                | Piece::Clip { children, .. }
                | Piece::Move { children, .. } => 1 + count(children),
            })
            .sum()
    }

    /// Calls `f` with the parent list and index of the `target`th piece, counting
    /// pieces in painting order.
    fn at(
        pieces: &mut Vec<Piece>,
        target: usize,
        seen: &mut usize,
        f: &mut dyn FnMut(&mut Vec<Piece>, usize),
    ) -> bool {
        let mut i = 0;
        while i < pieces.len() {
            if *seen == target {
                f(pieces, i);
                return true;
            }
            *seen += 1;
            if let Piece::Group { children, .. }
            | Piece::Clip { children, .. }
            | Piece::Move { children, .. } = &mut pieces[i]
                && at(children, target, seen, f)
            {
                return true;
            }
            i += 1;
        }
        false
    }

    fn mutate(rng: &mut Rng, scene: &mut Vec<Piece>) {
        let total = count(scene);
        if total == 0 {
            scene.push(random_piece(rng, 0));
            return;
        }
        let target = rng.below(total as u32) as usize;
        let kind = rng.below(7);
        let mut fresh = random_piece(rng, 1);
        let (a, b, c) = (rng.unit(), rng.unit(), rng.below(3));
        at(scene, target, &mut 0, &mut |parent, i| match kind {
            // Recolor, move, or resize a drawing; or change what a group does.
            0..=2 => match &mut parent[i] {
                Piece::Quad { color, rect, .. } => {
                    if kind == 0 {
                        color[c as usize] = a;
                    } else {
                        rect.x = (rect.x + a * 40.0).min(WIDTH - 10.0);
                        rect.y = (rect.y + b * 30.0).min(HEIGHT - 10.0);
                    }
                }
                Piece::Group { opacity, .. } => *opacity = 0.2 + a * 0.8,
                Piece::Clip { rect, .. } => rect.width = 8.0 + a * 140.0,
                Piece::Move { dx, dy, .. } => {
                    *dx += a * 20.0 - 10.0;
                    *dy += b * 20.0 - 10.0;
                }
            },
            3 => {
                parent.remove(i);
            }
            4 => parent.insert(
                i,
                std::mem::replace(
                    &mut fresh,
                    Piece::Group {
                        opacity: 1.0,
                        children: vec![],
                    },
                ),
            ),
            5 if i + 1 < parent.len() => parent.swap(i, i + 1),
            _ => parent.push(fresh.clone()),
        });
    }

    fn first_difference(a: &[u32], b: &[u32], width: usize) -> Option<(usize, usize, u32, u32)> {
        a.iter()
            .zip(b)
            .position(|(x, y)| x != y)
            .map(|at| (at % width, at / width, a[at], b[at]))
    }

    #[test]
    fn painting_only_what_changed_gives_exactly_the_pixels_of_a_full_repaint() {
        let extent = Extent2d::new(WIDTH as u32, HEIGHT as u32).unwrap();
        let mut incremental = NativeBackend::new().unwrap();
        let mut reference = NativeBackend::new().unwrap();
        let (mut rects, mut full, mut unchanged, mut area, mut frames) = (0, 0, 0, 0.0, 0);
        for seed in 1..=24_u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let mut scene: Vec<Piece> = (0..6 + rng.below(8))
                .map(|_| random_piece(&mut rng, 0))
                .collect();
            // A new scene starts from nothing on screen.
            incremental.invalidate_for_measurement();
            for step in 0..10 {
                if step > 0 {
                    mutate(&mut rng, &mut scene);
                }
                let frame = frame_of(&scene);
                let (_, painted) = incremental.render_incremental(&frame, extent).unwrap();
                incremental.mark_visible(frame.clone());
                reference.invalidate_for_measurement();
                reference.render_incremental(&frame, extent).unwrap();
                if let Some((x, y, got, want)) = first_difference(
                    incremental.canvas(),
                    reference.canvas(),
                    extent.width() as usize,
                ) {
                    panic!(
                        "seed {seed} step {step}: pixel ({x}, {y}) is {got:#010x}, a full repaint gives {want:#010x}; painted {painted:?}"
                    );
                }
                frames += 1;
                match &painted {
                    Painted::Unchanged => unchanged += 1,
                    Painted::Full => full += 1,
                    Painted::Rects(r) => {
                        rects += 1;
                        area += r
                            .iter()
                            .map(|r| f64::from(r.width()) * f64::from(r.height()))
                            .sum::<f64>()
                            / (f64::from(extent.width()) * f64::from(extent.height()));
                    }
                }
            }
        }
        // The test must exercise the incremental path, not fall back every time.
        assert!(
            rects * 3 >= frames,
            "{rects} of {frames} frames were incremental ({full} full, {unchanged} unchanged)"
        );
        assert!(
            area / f64::from(rects.max(1)) < 0.5,
            "incremental frames repainted {:.0}% of the target on average",
            100.0 * area / f64::from(rects.max(1))
        );
    }

    #[test]
    fn a_resized_target_and_a_frame_never_presented_are_repainted_in_full() {
        let mut backend = NativeBackend::new().unwrap();
        let mut rng = Rng(7);
        let scene: Vec<Piece> = (0..6).map(|_| random_piece(&mut rng, 0)).collect();
        let frame = frame_of(&scene);
        let small = Extent2d::new(WIDTH as u32, HEIGHT as u32).unwrap();
        let (_, first) = backend.render_incremental(&frame, small).unwrap();
        assert_eq!(first, Painted::Full, "nothing is on screen yet");
        // Drawn but never presented (no mark_visible): there is no frame to diff.
        let mut changed = scene.clone();
        mutate(&mut rng, &mut changed);
        let (_, after) = backend
            .render_incremental(&frame_of(&changed), small)
            .unwrap();
        assert_eq!(after, Painted::Full);
        // A different extent starts again, even for a presented frame.
        backend.mark_visible(frame.clone());
        let large = Extent2d::new(WIDTH as u32 + 16, HEIGHT as u32).unwrap();
        let wider = frame_sized(&scene, WIDTH + 16.0, HEIGHT);
        let (_, resized) = backend.render_incremental(&wider, large).unwrap();
        assert_eq!(resized, Painted::Full);
        assert_eq!(
            backend.canvas().len(),
            (large.width() * large.height()) as usize
        );
    }

    #[test]
    fn a_frame_with_a_backdrop_blur_is_always_repainted_in_full() {
        let extent = Extent2d::new(WIDTH as u32, HEIGHT as u32).unwrap();
        let bounds = Rect {
            x: 0.0,
            y: 0.0,
            width: WIDTH,
            height: HEIGHT,
        };
        let blurred = |tweak: f32| {
            let mut recorder = Recorder::new_in(bounds, 1.0, System);
            record(&mut recorder, &random_piece(&mut Rng(3), 2));
            recorder.backdrop_blur(
                Rect {
                    x: 10.0 + tweak,
                    y: 10.0,
                    width: 80.0,
                    height: 60.0,
                },
                CornerRadii::default(),
                4.0,
            );
            recorder.take_frame().unwrap()
        };
        let mut backend = NativeBackend::new().unwrap();
        let first = blurred(0.0);
        backend.render_incremental(&first, extent).unwrap();
        backend.mark_visible(first);
        let (_, painted) = backend.render_incremental(&blurred(5.0), extent).unwrap();
        assert_eq!(painted, Painted::Full);
    }
}
