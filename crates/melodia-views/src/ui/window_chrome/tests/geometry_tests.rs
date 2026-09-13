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

/// `geometry.rs`, comments stripped, for the walks below.
fn geometry_source() -> String {
    strip_line_comments(include_str!("../geometry.rs"))
}

/// The body of the first block opening after `needle`, or empty when the walk found no such
/// block, which every caller asserts apart from the code being wrong.
fn block_after<'a>(code: &'a str, needle: &str) -> &'a str {
    code.find(needle)
        .and_then(|at| code[at..].find('{').map(|rel| at + rel))
        .and_then(|open| block_body(code, open))
        .unwrap_or_default()
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

// The Win32 reading of a minimized window, which neither caller may take as geometry.
#[test]
fn an_empty_client_is_a_minimized_window() {
    assert!(
        is_minimized_client(WinitPhysicalSize::new(0, 0)),
        "a minimized window's empty client would be recorded as the size to restore"
    );
}

// Either axis on its own, so the check can't narrow to both without failing here.
#[test]
fn a_client_empty_on_one_axis_is_a_minimized_window() {
    assert!(
        is_minimized_client(WinitPhysicalSize::new(0, 600)),
        "a client with no width describes no window to restore"
    );
    assert!(
        is_minimized_client(WinitPhysicalSize::new(800, 0)),
        "a client with no height describes no window to restore"
    );
}

// The step past the edge: any client with area is a window the user can see.
#[test]
fn a_one_pixel_client_is_a_live_window() {
    assert!(
        !is_minimized_client(WinitPhysicalSize::new(1, 1)),
        "a live window read as minimized would stop recording its geometry"
    );
}

/// Win32 minimizes to an empty client at −32000, −32000 and clears the maximized flag, so a
/// window closed from the taskbar persisted all of it and relaunched un-maximized, clamped and
/// re-centred. A source walk because the reading needs a live, minimized window: the guard has to
/// return before the mirror is touched, the maximized flag being written under the same lock.
#[test]
fn a_minimized_window_records_nothing() {
    const FN: &str = "pub fn record";
    const GUARD: &str = "if is_minimized_client(";
    const MIRROR_WRITE: &str = "live().lock()";
    let code = geometry_source();
    let body = block_after(&code, FN);
    let guard_at = body.find(GUARD);
    let write_at = body.find(MIRROR_WRITE);

    assert!(!body.is_empty(), "no `{FN}` found: the walk is broken, not the code");
    assert_eq!(
        block_after(body, GUARD).trim(),
        "return;",
        "`record` no longer leaves a minimized reading out of the mirror:\n{body}"
    );
    assert!(
        matches!((guard_at, write_at), (Some(guard), Some(write)) if guard < write),
        "the minimized guard has to run before the mirror write, or the maximized flag still \
         lands:\n{body}"
    );
}

/// Once the miniplayer drops the frame, outer equals inner, so a reading taken there is zero and
/// would overwrite the one the exit edge needs. The client area has already grown by the frame,
/// so the window lands past the shrunken edge and bounces back out. A source walk because
/// answering needs a live, decorated window.
#[test]
fn an_undecorated_window_gives_no_reading() {
    const FN: &str = "pub fn frame_allowance";
    let code = geometry_source();
    let body = block_after(&code, FN);

    assert!(!body.is_empty(), "no `{FN}` found: the walk is broken, not the code");
    assert!(
        body.contains("is_decorated()"),
        "`frame_allowance` measures an undecorated window, whose zero reading replaces the frame \
         the miniplayer's exit edge allows for:\n{body}"
    );
}
