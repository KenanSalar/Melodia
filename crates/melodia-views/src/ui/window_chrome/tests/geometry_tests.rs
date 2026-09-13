use super::*;
use melodia_testkit::{block_body, strip_line_comments};

/// Sub-pixel slack for a length divided back out of physical pixels.
const TOLERANCE: f32 = 0.001;

fn assert_allowance(got: WinitLogicalSize<f32>, width: f32, height: f32) {
    assert!(
        (got.width - width).abs() < TOLERANCE && (got.height - height).abs() < TOLERANCE,
        "expected {width}×{height}, got {}×{}",
        got.width,
        got.height
    );
}

// The Win32 case the miniplayer's exit edge exists for: undecorated, the client area takes the
// whole window rect, so it grows by exactly this much.
#[test]
fn a_frame_measures_what_it_adds_around_the_client() {
    let frame =
        allowance_between(WinitPhysicalSize::new(816, 639), WinitPhysicalSize::new(800, 600), 1.0);
    assert_allowance(frame, 16.0, 39.0);
}

// `MiniPlayerSwitch` compares its edges in logical pixels, so the reading has to be in them too.
#[test]
fn a_scale_factor_divides_the_frame_back_into_logical_pixels() {
    let frame = allowance_between(
        WinitPhysicalSize::new(1224, 960),
        WinitPhysicalSize::new(1200, 900),
        1.5,
    );
    assert_allowance(frame, 16.0, 40.0);
}

// Wayland with server-side decorations reports the client size for both.
#[test]
fn a_frame_outside_the_reported_sizes_measures_nothing() {
    let frame =
        allowance_between(WinitPhysicalSize::new(800, 600), WinitPhysicalSize::new(800, 600), 1.0);
    assert_allowance(frame, 0.0, 0.0);
}

// Saturating, because a wrapped `u32` is an allowance past any window size, and the miniplayer
// would never let go.
#[test]
fn an_inner_size_past_the_outer_never_wraps() {
    let frame =
        allowance_between(WinitPhysicalSize::new(800, 600), WinitPhysicalSize::new(801, 602), 1.0);
    assert_allowance(frame, 0.0, 0.0);
}

/// Once the miniplayer drops the frame, outer equals inner, so a reading taken there is zero and
/// would overwrite the one the exit edge needs. The client area has already grown by the frame,
/// so the window lands past the shrunken edge and bounces back out. A source walk because
/// answering needs a live, decorated window.
#[test]
fn an_undecorated_window_gives_no_reading() {
    const FN: &str = "pub fn frame_allowance";
    let code = strip_line_comments(include_str!("../geometry.rs"));
    let body = code
        .find(FN)
        .and_then(|at| code[at..].find('{').map(|rel| at + rel))
        .and_then(|open| block_body(&code, open))
        .unwrap_or_default();

    assert!(!body.is_empty(), "no `{FN}` found: the walk is broken, not the code");
    assert!(
        body.contains("is_decorated()"),
        "`frame_allowance` measures an undecorated window, whose zero reading replaces the frame \
         the miniplayer's exit edge allows for:\n{body}"
    );
}
