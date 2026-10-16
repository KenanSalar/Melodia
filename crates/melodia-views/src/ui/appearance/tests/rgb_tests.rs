use super::{brush_to_rgb, brush_with_alpha, color, color_to_rgb};

/// Every palette table and solved tier is held packed, so a value comes back from Slint as it went
/// in. The last row has an alpha byte on top, which is no part of the colour.
#[test]
fn a_packed_colour_round_trips_through_slint() {
    let packed = [0x0000_0000, 0x00ff_ffff, 0x0012_3456, 0xff12_3456];

    let read = packed.map(|rgb| color_to_rgb(color(rgb)));

    assert_eq!(read, [0x0000_0000, 0x00ff_ffff, 0x0012_3456, 0x0012_3456]);
}

/// A surface reads the accent's hue back off a brush that may carry a scrim's weight, and that
/// weight is no part of the hue.
#[test]
fn reading_a_brush_back_drops_its_alpha() {
    let alphas = [0, 128, 255];

    let read = alphas.map(|alpha| brush_to_rgb(&brush_with_alpha(0x0012_3456, alpha)));

    assert_eq!(read, [0x0012_3456; 3]);
}
