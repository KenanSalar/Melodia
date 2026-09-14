use super::*;
use melodia_testkit::{block_after, strip_line_comments};

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

/// Every term alone has to withhold the client: a frame measured off an undecorated, a maximized
/// or a minimized window is one the miniplayer's exit edge then widens by for no frame at all.
#[test]
fn only_a_decorated_restored_window_has_a_frame_to_measure_around() {
    const LIVE: WinitPhysicalSize<u32> = WinitPhysicalSize::new(800, 600);
    const MINIMIZED: WinitPhysicalSize<u32> = WinitPhysicalSize::new(0, 0);
    // (decorated, maximized, client, measurable)
    let table = [
        (true, false, LIVE, Some(LIVE)),
        (false, false, LIVE, None),
        (true, true, LIVE, None),
        (true, false, MINIMIZED, None),
        (false, true, LIVE, None),
        (false, false, MINIMIZED, None),
        (true, true, MINIMIZED, None),
        (false, true, MINIMIZED, None),
    ];
    for (decorated, maximized, client, measurable) in table {
        let reading = WindowReading { client, scale: 1.0, maximized, decorated };
        assert_eq!(
            measurable_client(reading),
            measurable,
            "decorated {decorated}, maximized {maximized}, client {client:?}"
        );
    }
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
    const FN: &str = "fn measurable_client";
    let code = geometry_source();
    let body = block_after(&code, FN);

    assert!(!body.is_empty(), "no `{FN}` found: the walk is broken, not the code");
    assert!(
        body.contains("reading.decorated"),
        "`measurable_client` passes an undecorated window, whose zero reading replaces the frame \
         the miniplayer's exit edge allows for:\n{body}"
    );
}

/// Both frame readings answer the same question about when there is a frame to read, and a reader
/// that asks its own version is how one of them starts measuring an undecorated or minimized
/// window again.
#[test]
fn every_frame_reading_goes_through_the_one_gate() {
    const READERS: [&str; 2] = ["pub fn frame_allowance", "pub fn frame_margins"];
    let code = geometry_source();

    let ungated: Vec<&str> = READERS
        .into_iter()
        .filter(|reader| !block_after(&code, reader).contains("measurable_client(reading)?"))
        .collect();

    assert!(ungated.is_empty(), "these frame readings skip `measurable_client`: {ungated:?}");
}

#[cfg(target_os = "windows")]
fn rect(x: i32, y: i32, width: u32, height: u32) -> ScreenRect {
    ScreenRect { at: WinitPhysicalPosition::new(x, y), size: WinitPhysicalSize::new(width, height) }
}

#[cfg(target_os = "windows")]
fn assert_margins(got: WinitLogicalInsets<f32>, left: f32, right: f32, bottom: f32) {
    assert!(
        (got.left - left).abs() < TOLERANCE
            && (got.right - right).abs() < TOLERANCE
            && (got.bottom - bottom).abs() < TOLERANCE,
        "expected left {left}, right {right}, bottom {bottom}; got {}, {}, {}",
        got.left,
        got.right,
        got.bottom
    );
}

// A Windows 11 frame at 100 %: an 8 px border either side and below, a 31 px caption above.
#[cfg(target_os = "windows")]
#[test]
fn the_margins_are_the_frame_beside_and_below_the_client() {
    let margins = margins_between(rect(100, 50, 816, 639), rect(108, 81, 800, 600), 1.0);

    assert_margins(margins, 8.0, 8.0, 8.0);
}

// Windows 11 draws all three sides alike, which is exactly why a side read off its neighbour would
// pass every symmetric fixture; this frame is uneven on purpose.
#[cfg(target_os = "windows")]
#[test]
fn each_margin_is_measured_on_its_own_side() {
    let margins = margins_between(rect(100, 50, 822, 641), rect(106, 81, 800, 600), 1.0);

    assert_margins(margins, 6.0, 16.0, 10.0);
}

/// The caption is the frame the user sees. Counted as a margin, the miniplayer would drop its top
/// edge by the caption's height and the window would visibly move on every swap.
#[cfg(target_os = "windows")]
#[test]
fn the_caption_above_the_client_is_no_margin() {
    let margins = margins_between(rect(100, 50, 816, 639), rect(108, 81, 800, 600), 1.0);

    assert!(margins.top.abs() < TOLERANCE, "the caption was read as {} px of margin", margins.top);
}

// The shell insets in logical pixels, so the reading has to be in them too.
#[cfg(target_os = "windows")]
#[test]
fn a_scale_factor_divides_the_margins_back_into_logical_pixels() {
    let margins = margins_between(rect(100, 50, 1224, 960), rect(112, 98, 1200, 900), 1.5);

    assert_margins(margins, 8.0, 8.0, 8.0);
}

// Every position is negative on a monitor left of or above the primary; only the distances count.
#[cfg(target_os = "windows")]
#[test]
fn a_window_on_a_monitor_left_of_the_primary_measures_the_same_margins() {
    let margins = margins_between(rect(-1920, -40, 816, 639), rect(-1912, -9, 800, 600), 1.0);

    assert_margins(margins, 8.0, 8.0, 8.0);
}

// Floored rather than wrapped: a wrapped `u32` is a margin wider than the window.
#[cfg(target_os = "windows")]
#[test]
fn a_client_past_the_frame_edge_never_wraps() {
    let margins = margins_between(rect(100, 50, 800, 600), rect(99, 50, 802, 601), 1.0);

    assert_margins(margins, 0.0, 0.0, 0.0);
}
