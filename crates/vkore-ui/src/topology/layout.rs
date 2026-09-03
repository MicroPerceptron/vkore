//! Tidy-tree placement for the agent spawn lineage.
//!
//! Leaves consume horizontal cursor space in order; a parent centres over the
//! span of its children. That is the whole rule, and it is enough for a spawn
//! tree, which is shallow and never re-parents.
//!
//! Placement is pure arithmetic over indices. Keeping it here rather than in
//! the drawing code means the shape of a graph can be asserted without a
//! renderer, a target, or a frame.

/// A node's placed rectangle, in graph coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Node width.
    pub width: f32,
    /// Node height.
    pub height: f32,
}

impl Placement {
    /// Horizontal centre, where an edge to a child departs.
    pub fn center_x(&self) -> f32 {
        self.x + self.width / 2.0
    }

    /// Vertical centre.
    pub fn center_y(&self) -> f32 {
        self.y + self.height / 2.0
    }
}

/// The measurements a tidy tree is laid out against.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    /// Node width.
    pub width: f32,
    /// Node height.
    pub height: f32,
    /// Gap between siblings.
    pub gap_x: f32,
    /// Gap between depths.
    pub gap_y: f32,
}

impl Metrics {
    /// The design's agent node: 262 x 124, 30 across, 86 down.
    pub const AGENTS: Self = Self {
        width: 262.0,
        height: 124.0,
        gap_x: 30.0,
        gap_y: 86.0,
    };
}

/// Places a forest given each node's parent, in the order supplied.
///
/// `parents[i]` is the index of `i`'s parent, or `None` for a root. Children
/// keep the order they appear in. A node whose parent index is not smaller
/// than its own is treated as a root, so a malformed or cyclic parent link
/// degrades to a separate tree rather than looping.
pub fn tidy(parents: &[Option<usize>], metrics: Metrics) -> Vec<Placement> {
    let count = parents.len();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); count];
    let mut roots = Vec::new();
    for (index, parent) in parents.iter().enumerate() {
        match parent {
            Some(parent) if *parent < index => children[*parent].push(index),
            _ => roots.push(index),
        }
    }

    let mut depth = vec![0usize; count];
    for index in 0..count {
        if let Some(parent) = parents.get(index).copied().flatten()
            && parent < index
        {
            depth[index] = depth[parent] + 1;
        }
    }

    let mut placement = vec![
        Placement {
            x: 0.0,
            y: 0.0,
            width: metrics.width,
            height: metrics.height,
        };
        count
    ];
    let mut cursor = 0.0f32;
    for root in roots {
        cursor = place(root, cursor, &children, &depth, metrics, &mut placement);
    }
    placement
}

/// Places `node` and its subtree, returning the next free cursor position.
fn place(
    node: usize,
    cursor: f32,
    children: &[Vec<usize>],
    depth: &[usize],
    metrics: Metrics,
    placement: &mut [Placement],
) -> f32 {
    placement[node].y = depth[node] as f32 * (metrics.height + metrics.gap_y);
    if children[node].is_empty() {
        placement[node].x = cursor;
        return cursor + metrics.width + metrics.gap_x;
    }
    let mut next = cursor;
    for child in &children[node] {
        next = place(*child, next, children, depth, metrics, placement);
    }
    let first = placement[children[node][0]].x;
    let last = placement[*children[node].last().expect("a child")].x;
    // Centre the parent over the span its children occupy.
    placement[node].x = (first + last) / 2.0;
    next
}

/// The bounding box of every placement, or `None` for an empty graph.
pub fn bounds(placements: &[Placement]) -> Option<(f32, f32, f32, f32)> {
    let first = placements.first()?;
    let mut left = first.x;
    let mut top = first.y;
    let mut right = first.x + first.width;
    let mut bottom = first.y + first.height;
    for placement in placements.iter().skip(1) {
        left = left.min(placement.x);
        top = top.min(placement.y);
        right = right.max(placement.x + placement.width);
        bottom = bottom.max(placement.y + placement.height);
    }
    Some((left, top, right, bottom))
}

/// The transform that fits `placements` into a viewport of `width` x `height`.
///
/// Returns a translation and a scale clamped to the topology's interactive
/// zoom range. A graph is never magnified past its drawn size. It may shrink
/// below the mock's 70% preferred floor when that is the only way to keep
/// complete cards inside a smaller viewport; clipping a node's identity or
/// authority is worse than rendering it slightly smaller.
pub fn fit(placements: &[Placement], width: f32, height: f32, padding: f32) -> (f32, f32, f32) {
    let Some((left, top, right, bottom)) = bounds(placements) else {
        return (0.0, 0.0, 1.0);
    };
    let scene_width = (right - left).max(1.0);
    let scene_height = (bottom - top).max(1.0);
    let available_width = (width - padding * 2.0).max(1.0);
    let available_height = (height - padding * 2.0).max(1.0);
    let scale = (available_width / scene_width)
        .min(available_height / scene_height)
        .clamp(0.32, 1.0);

    if scene_height * scale > available_height {
        let root = &placements[0];
        return (
            width / 2.0 - root.center_x() * scale,
            padding - top * scale,
            scale,
        );
    }
    (
        (width - scene_width * scale) / 2.0 - left * scale,
        (height - scene_height * scale) / 2.0 - top * scale,
        scale,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const M: Metrics = Metrics::AGENTS;

    #[test]
    fn a_lone_node_sits_at_the_origin() {
        let placed = tidy(&[None], M);
        assert_eq!(placed.len(), 1);
        assert_eq!(placed[0].x, 0.0);
        assert_eq!(placed[0].y, 0.0);
    }

    #[test]
    fn siblings_are_spaced_by_one_gap_and_never_overlap() {
        let placed = tidy(&[None, Some(0), Some(0), Some(0)], M);
        let leaves = [&placed[1], &placed[2], &placed[3]];
        for pair in leaves.windows(2) {
            assert_eq!(pair[1].x - pair[0].x, M.width + M.gap_x);
            assert!(pair[0].x + pair[0].width <= pair[1].x, "siblings overlap");
        }
    }

    #[test]
    fn a_parent_centres_over_the_span_of_its_children() {
        let placed = tidy(&[None, Some(0), Some(0), Some(0)], M);
        let expected = (placed[1].x + placed[3].x) / 2.0;
        assert_eq!(placed[0].x, expected);
        // Its centre also lands between the outer children's centres.
        assert!(
            (placed[0].center_x() - (placed[1].center_x() + placed[3].center_x()) / 2.0).abs()
                < 0.001
        );
    }

    #[test]
    fn depth_sets_the_row_and_nothing_else_does() {
        //      0
        //    1   2
        //       3 4
        let placed = tidy(&[None, Some(0), Some(0), Some(2), Some(2)], M);
        let row = M.height + M.gap_y;
        assert_eq!(placed[0].y, 0.0);
        assert_eq!(placed[1].y, row);
        assert_eq!(placed[2].y, row);
        assert_eq!(placed[3].y, row * 2.0);
        assert_eq!(placed[4].y, row * 2.0);
    }

    #[test]
    fn a_deeper_subtree_still_leaves_its_sibling_clear() {
        let placed = tidy(&[None, Some(0), Some(0), Some(2), Some(2)], M);
        // The two grandchildren consume the cursor, so the shallow sibling
        // cannot sit on top of them.
        assert!(placed[1].x + placed[1].width <= placed[3].x);
        assert!(placed[3].x + placed[3].width <= placed[4].x);
    }

    #[test]
    fn a_parent_pointing_forward_is_treated_as_another_root() {
        // A malformed link must not recurse forever.
        let placed = tidy(&[Some(1), None], M);
        assert_eq!(placed.len(), 2);
        assert_eq!(placed[0].y, 0.0);
        assert_eq!(placed[1].y, 0.0);
        assert_ne!(placed[0].x, placed[1].x);
    }

    #[test]
    fn fit_never_magnifies_and_never_shrinks_past_the_interaction_floor() {
        let wide = tidy(&[None, Some(0), Some(0), Some(0), Some(0), Some(0)], M);
        let (_, _, small) = fit(&wide, 400.0, 400.0, 36.0);
        assert_eq!(small, 0.32, "a large scene clamps at the floor");

        let tiny = tidy(&[None], M);
        let (_, _, large) = fit(&tiny, 4000.0, 4000.0, 36.0);
        assert_eq!(large, 1.0, "a small scene is never magnified");
    }

    #[test]
    fn fit_centres_a_scene_that_comfortably_fits() {
        let placed = tidy(&[None], M);
        let (tx, ty, scale) = fit(&placed, 1000.0, 600.0, 36.0);
        let centre_x = tx + placed[0].center_x() * scale;
        let centre_y = ty + placed[0].center_y() * scale;
        assert!((centre_x - 500.0).abs() < 0.001);
        assert!((centre_y - 300.0).abs() < 0.001);
    }

    #[test]
    fn fit_keeps_a_wide_scene_inside_the_viewport_when_the_zoom_range_allows_it() {
        let wide = tidy(&[None, Some(0), Some(0), Some(0), Some(0)], M);
        let width = 760.0;
        let padding = 36.0;
        let (tx, _, scale) = fit(&wide, width, 560.0, padding);
        let (left, _, right, _) = bounds(&wide).expect("a non-empty scene");
        assert!(tx + left * scale >= padding - 0.001);
        assert!(tx + right * scale <= width - padding + 0.001);
    }

    #[test]
    fn fit_pins_an_overflowing_scene_to_the_top_instead_of_centring_it() {
        let deep = tidy(&[None, Some(0), Some(1), Some(2), Some(3)], M);
        let (tx, ty, scale) = fit(&deep, 800.0, 300.0, 36.0);
        assert!((tx + deep[0].center_x() * scale - 400.0).abs() < 0.001);
        assert!((ty + deep[0].y * scale - 36.0).abs() < 0.001);
    }

    #[test]
    fn an_empty_graph_has_no_bounds_and_an_identity_fit() {
        assert_eq!(bounds(&[]), None);
        assert_eq!(fit(&[], 800.0, 600.0, 36.0), (0.0, 0.0, 1.0));
    }
}
