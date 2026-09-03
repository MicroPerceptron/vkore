//! The MicroPerceptron mark.
//!
//! The published asset is an SVG of eighteen filled shapes. The display list
//! carries no images and no SVG, but it does carry the geometry lane, and
//! zeno parses SVG path data directly -- so the mark is drawn as paths rather
//! than rasterized elsewhere and blitted. It stays sharp at any size for the
//! same reason the topology edges do.
//!
//! Shapes are listed in paint order; the two magenta ones are the screen bars
//! the splash animates.

/// One filled shape of the mark.
pub struct Shape {
    /// The asset's own id, kept so the animated parts can be found by name.
    pub id: &'static str,
    /// Fill colour as authored.
    pub fill: u32,
    /// SVG path data, which zeno parses without a conversion step.
    pub data: &'static str,
}

/// The mark's design grid. Every path is expressed in this space.
pub const VIEWBOX: f32 = 900.0;

/// The shapes the splash animates as a screen powering on.
pub const SCREEN_BARS: [&str; 2] = ["bar_top", "content_orange"];

/// The mark, in paint order.
pub const SHAPES: &[Shape] = &[
    Shape {
        id: "body",
        fill: 0x10101A,
        data: "M614.5,176.5L648.5,178.0L725.5,214.0L735.0,228.5L735.0,262.5L818.0,359.5L819.5,461.5L818.0,563.5L811.5,572.0L729.5,628.5L728.0,688.5C726.0,699.5 719.5,706.5 708.5,709.0L639.5,722.0L194.5,722.0L135.5,714.0L99.5,713.0C87.0,708.5 80.5,699.5 79.5,686.0L87.0,242.5C90.0,230.5 97.5,223.0 109.5,220.0L614.5,176.5Z",
    },
    Shape {
        id: "panel_front",
        fill: 0xD0CCD8,
        data: "M616.5,197.5L630.5,198.5L632.0,200.5L620.0,669.5L617.5,673.0L596.5,675.0L142.5,692.0L103.5,692.0L102.0,690.5L100.5,664.5L107.0,246.5L109.5,242.0L128.5,239.0L616.5,197.5Z",
    },
    Shape {
        id: "panel_side",
        fill: 0xC0BCC8,
        data: "M652.5,203.5L714.0,232.5L707.0,686.5L697.5,689.5L638.5,678.0L641.0,671.5L652.5,203.5Z",
    },
    Shape {
        id: "edge_hi",
        fill: 0x6A6878,
        data: "M735.5,294.5L733.5,310.5L734.5,293.5L735.5,294.5Z",
    },
    Shape {
        id: "panel_detail",
        fill: 0xD0CCD8,
        data: "M735.5,294.5L798.0,366.5L798.0,555.5L730.5,602.5L729.5,580.5L733.5,310.5L735.5,294.5Z",
    },
    Shape {
        id: "side_edge",
        fill: 0x20202A,
        data: "M729.5,580.5L730.5,602.5L729.5,603.5L729.5,580.5Z",
    },
    Shape {
        id: "screen",
        fill: 0x10101A,
        data: "M566.5,241.5L584.5,244.0L589.0,249.5L587.0,390.5L581.0,596.5L571.5,607.0L145.5,625.0L137.0,616.5L144.0,287.5L152.5,279.0L566.5,241.5Z",
    },
    Shape {
        id: "bar_top",
        fill: 0xFF2D78,
        data: "M547.5,264.5L564.5,265.0L573.0,273.5L571.5,350.5L168.5,365.5L170.0,306.5L178.5,298.0L547.5,264.5Z",
    },
    Shape {
        id: "content_orange",
        fill: 0xFF2D78,
        data: "M537.5,400.5L570.5,400.5L566.0,582.5L556.5,592.0L171.5,607.0L163.0,597.5L166.5,438.5L177.5,439.0L179.0,440.5L180.0,457.5L181.5,459.0L198.5,460.5L200.0,483.5L201.5,485.0L297.5,485.0L299.0,483.5L300.5,455.5L302.5,454.0L322.5,453.0L324.0,451.5L325.0,415.5L326.5,414.0L354.5,414.0L356.0,415.5L357.0,429.5L358.5,431.0L379.5,432.0L381.0,433.5L382.0,448.5L383.5,450.0L398.5,451.0L401.0,452.5L402.0,474.5L403.5,476.0L489.5,476.0L492.0,473.5L493.5,448.5L511.5,447.0L514.0,444.5L515.0,433.5L516.5,431.0L533.5,430.0L535.0,428.5L536.0,402.5L537.5,400.5Z",
    },
    Shape {
        id: "edge_shd",
        fill: 0x5A5868,
        data: "M373.5,409.5L394.5,409.5L394.5,430.5L393.5,410.5L373.5,409.5Z",
    },
    Shape {
        id: "shd_detail",
        fill: 0x7A7888,
        data: "M398.5,434.5L415.5,434.5L415.5,455.5L414.5,435.5L398.5,434.5Z",
    },
    Shape {
        id: "glasses_R",
        fill: 0xD0CCD8,
        data: "M423.5,410.5L423.5,430.5L439.5,430.5L444.0,433.5L443.5,457.5L439.5,460.0L419.5,460.0L415.5,455.5L415.5,434.5L398.5,434.5L394.5,430.5L394.5,409.5L373.5,409.5L370.0,406.5L370.0,383.5L372.5,381.0L395.5,381.0L399.0,384.5L399.5,405.5L419.5,406.0L423.5,410.5Z",
    },
    Shape {
        id: "glasses_L",
        fill: 0xD0CCD8,
        data: "M199.5,396.5L199.5,416.5L219.5,416.5L224.0,420.5L224.5,441.5L241.5,442.0L244.0,443.5L245.0,465.5L242.5,470.0L219.5,471.0L216.0,466.5L215.5,446.5L198.5,446.0L196.0,443.5L194.5,421.5L174.5,421.0L171.0,418.5L170.0,395.5L173.5,392.0L196.5,392.0L199.5,396.5Z",
    },
    Shape {
        id: "glasses_R_shd",
        fill: 0x2E2A38,
        data: "M423.5,410.5L424.5,429.5L439.5,430.5L423.5,430.5L423.5,410.5Z",
    },
    Shape {
        id: "glasses_L_shd",
        fill: 0x1E1A28,
        data: "M199.5,396.5L200.5,415.5L219.5,416.5L199.5,416.5L199.5,396.5Z",
    },
    Shape {
        id: "",
        fill: 0x10101A,
        data: "M284.5,656.0C284.5,648.5441558777 290.5441558777,642.5 298.0,642.5C305.4558441223,642.5 311.5,648.5441558777 311.5,656.0C311.5,663.4558441223 305.4558441223,669.5 298.0,669.5C290.5441558777,669.5 284.5,663.4558441223 284.5,656.0Z",
    },
    Shape {
        id: "",
        fill: 0x10101A,
        data: "M331.5,652.5C331.5,645.0441558777 337.5441558777,639.0 345.0,639.0C352.4558441223,639.0 358.5,645.0441558777 358.5,652.5C358.5,659.9558441223 352.4558441223,666.0 345.0,666.0C337.5441558777,666.0 331.5,659.9558441223 331.5,652.5Z",
    },
    Shape {
        id: "",
        fill: 0x10101A,
        data: "M377.5,649.5C377.5,642.0441558777 383.5441558777,636.0 391.0,636.0C398.4558441223,636.0 404.5,642.0441558777 404.5,649.5C404.5,656.9558441223 398.4558441223,663.0 391.0,663.0C383.5441558777,663.0 377.5,656.9558441223 377.5,649.5Z",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shape_carries_parseable_path_data() {
        assert_eq!(SHAPES.len(), 18);
        for shape in SHAPES {
            assert!(
                zeno::validate_svg(shape.data).is_ok(),
                "shape {} does not parse",
                shape.id
            );
        }
    }

    #[test]
    fn the_animated_bars_are_present_and_are_the_accent() {
        for id in SCREEN_BARS {
            let shape = SHAPES
                .iter()
                .find(|shape| shape.id == id)
                .unwrap_or_else(|| panic!("{id} is missing"));
            assert_eq!(shape.fill, 0xFF2D78, "{id} is not a screen bar");
        }
    }

    #[test]
    fn shapes_stay_inside_the_design_grid() {
        // A coordinate outside the viewbox would place the mark wrong at
        // every size, and is the kind of thing an export silently does.
        for shape in SHAPES {
            for number in shape
                .data
                .split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
                .filter(|part| !part.is_empty())
            {
                if let Ok(value) = number.parse::<f32>() {
                    assert!(
                        (-1.0..=VIEWBOX + 1.0).contains(&value),
                        "{} leaves the grid at {value}",
                        shape.id
                    );
                }
            }
        }
    }
}
