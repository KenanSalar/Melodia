use super::*;
use melodia_testkit::{block_after, reading_env, strip_line_comments};

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

fn sized(width: f64, height: f64) -> PersistedGeometry {
    PersistedGeometry { width, height, x: 0.0, y: 0.0, maximized: false }
}

/// A restored window where winit reported one, at a fixed position.
fn placed(width: f64, height: f64) -> Placement {
    Placement {
        geom: PersistedGeometry { width, height, x: 40.0, y: 30.0, maximized: false },
        position_known: true,
    }
}

/// A restored window with no position to report, as every reading on Wayland is.
fn unplaced(width: f64, height: f64) -> Placement {
    Placement { geom: sized(width, height), position_known: false }
}

fn maximized(width: f64, height: f64) -> Placement {
    Placement {
        geom: PersistedGeometry { maximized: true, ..sized(width, height) },
        position_known: true,
    }
}

/// The mirror after a window's first reading, the way [`record`] takes it.
fn recorded(seen: Placement, at: Instant) -> LiveGeometry {
    let mut mirror = LiveGeometry::first(seen, at);
    mirror.observe(seen, at);
    mirror
}

fn assert_logical_size(got: LogicalSize, width: f32, height: f32) {
    assert!(
        (got.width - width).abs() < TOLERANCE && (got.height - height).abs() < TOLERANCE,
        "expected {width}×{height}, got {}×{}",
        got.width,
        got.height
    );
}

fn assert_placement_size(got: Placement, width: f64, height: f64) {
    let tolerance = f64::from(TOLERANCE);
    assert!(
        (got.geom.width - width).abs() < tolerance && (got.geom.height - height).abs() < tolerance,
        "expected {width}×{height}, got {}×{}",
        got.geom.width,
        got.geom.height
    );
}

// Each axis alone, or a hand-edited hold of normal width and no height restores unusable.
#[test]
fn a_width_below_the_floor_is_raised_alone() {
    let (size, _) = logical_placement(sized(100.0, 900.0), MIN_FULL_PLAYER);

    assert_logical_size(size, MIN_FULL_PLAYER.width, 900.0);
}

#[test]
fn a_height_below_the_floor_is_raised_alone() {
    let (size, _) = logical_placement(sized(900.0, 100.0), MIN_FULL_PLAYER);

    assert_logical_size(size, 900.0, MIN_FULL_PLAYER.height);
}

#[test]
fn a_size_on_the_floor_is_kept() {
    let floor = sized(f64::from(MIN_FULL_PLAYER.width), f64::from(MIN_FULL_PLAYER.height));

    let (size, _) = logical_placement(floor, MIN_FULL_PLAYER);

    assert_logical_size(size, MIN_FULL_PLAYER.width, MIN_FULL_PLAYER.height);
}

#[test]
fn a_size_just_past_the_floor_is_kept() {
    let past =
        sized(f64::from(MIN_FULL_PLAYER.width) + 1.0, f64::from(MIN_FULL_PLAYER.height) + 1.0);

    let (size, _) = logical_placement(past, MIN_FULL_PLAYER);

    assert_logical_size(size, MIN_FULL_PLAYER.width + 1.0, MIN_FULL_PLAYER.height + 1.0);
}

// A window on a monitor left of or above the primary sits at negative coordinates.
#[test]
fn a_position_is_never_floored() {
    let off_primary = PersistedGeometry { x: -1920.0, y: -40.0, ..sized(1200.0, 800.0) };

    let (_, position) = logical_placement(off_primary, MIN_FULL_PLAYER);

    assert!(
        (position.x + 1920.0).abs() < TOLERANCE && (position.y + 40.0).abs() < TOLERANCE,
        "expected (-1920, -40), got ({}, {})",
        position.x,
        position.y
    );
}

#[test]
fn a_hold_without_a_position_is_never_placed() {
    let saved = FullPlayerGeometry { width: 1200.0, height: 800.0, position: None };

    let hold = Placement::from_persisted(saved);

    assert!(!hold.position_known, "a hold with no position would place the window at 0, 0");
}

// `x` and `y` still carry numbers when nothing was ever read, and the file must not take them.
#[test]
fn an_unplaced_placement_persists_no_position() {
    let never_read = Placement {
        geom: PersistedGeometry { x: 12.0, y: 34.0, ..sized(1200.0, 800.0) },
        position_known: false,
    };

    let saved = never_read.to_persisted();

    assert_eq!(saved.position, None);
}

#[test]
fn a_reading_just_inside_the_settle_window_settles_nothing() {
    let t0 = Instant::now();
    let mut mirror = recorded(placed(1200.0, 800.0), t0);
    let last = t0 + Duration::from_millis(10);
    mirror.observe(placed(1000.0, 700.0), last);

    mirror.observe(placed(900.0, 600.0), last + SETTLE.saturating_sub(Duration::from_millis(1)));

    assert_placement_size(mirror.settled, 1200.0, 800.0);
}

#[test]
fn a_reading_a_settle_window_after_the_last_settles_the_placement_it_replaces() {
    let t0 = Instant::now();
    let mut mirror = recorded(placed(1200.0, 800.0), t0);
    let last = t0 + Duration::from_millis(10);
    mirror.observe(placed(1000.0, 700.0), last);

    mirror.observe(placed(900.0, 600.0), last + SETTLE);

    assert_placement_size(mirror.settled, 1000.0, 700.0);
}

/// Shrinking into the miniplayer is a drag, and the placement it ends on is a window just past the
/// threshold, which is what the restore caption handed back before the mirror kept a settled one.
#[test]
fn a_drag_into_the_miniplayer_leaves_its_starting_placement_settled() {
    let t0 = Instant::now();
    let mut mirror = recorded(placed(1200.0, 800.0), t0);
    let drag_start = t0 + Duration::from_secs(5);
    mirror.observe(placed(900.0, 600.0), drag_start);
    mirror.observe(placed(600.0, 400.0), drag_start + Duration::from_millis(10));

    mirror.observe(placed(400.0, 200.0), drag_start + Duration::from_millis(20));

    assert_placement_size(mirror.settled, 1200.0, 800.0);
}

#[test]
fn a_maximized_reading_keeps_the_restore_size() {
    let t0 = Instant::now();
    let mut mirror = recorded(placed(1200.0, 800.0), t0);

    mirror.observe(maximized(2560.0, 1440.0), t0 + Duration::from_millis(10));

    assert_placement_size(mirror.current, 1200.0, 800.0);
}

#[test]
fn a_maximized_reading_is_recorded_as_maximized() {
    let t0 = Instant::now();
    let mut mirror = recorded(placed(1200.0, 800.0), t0);

    mirror.observe(maximized(2560.0, 1440.0), t0 + Duration::from_millis(10));

    assert!(mirror.current.geom.maximized, "a window closed maximized would reopen restored");
}

#[test]
fn a_reading_without_a_position_keeps_the_last_known_one() {
    let t0 = Instant::now();
    let mut mirror = recorded(placed(1200.0, 800.0), t0);

    mirror.observe(unplaced(1000.0, 700.0), t0 + Duration::from_millis(10));

    let current = mirror.current;
    let tolerance = f64::from(TOLERANCE);
    assert!(
        current.position_known
            && (current.geom.x - 40.0).abs() < tolerance
            && (current.geom.y - 30.0).abs() < tolerance,
        "the last known position was dropped: {current:?}"
    );
}

#[test]
fn a_window_that_never_reports_a_position_stays_unplaced() {
    let t0 = Instant::now();
    let mut mirror = recorded(unplaced(1200.0, 800.0), t0);

    mirror.observe(unplaced(1000.0, 700.0), t0 + SETTLE);

    assert!(!mirror.current.position_known, "the zeros standing in for a position would persist");
}

#[test]
fn a_close_from_the_miniplayer_persists_the_held_full_player() {
    let mut settings = reading_env(SettingsData::default);
    let miniplayer = recorded(placed(400.0, 200.0), Instant::now());

    write_snapshot(&mut settings, miniplayer, Some(placed(1200.0, 800.0)));

    let tolerance = f64::from(TOLERANCE);
    assert!(
        matches!(settings.full_player_geometry, Some(held)
            if (held.width - 1200.0).abs() < tolerance && (held.height - 800.0).abs() < tolerance),
        "the held full player persisted as {:?}",
        settings.full_player_geometry
    );
}

// The field's contract is `Some` only for a window that closed as the miniplayer.
#[test]
fn a_close_from_the_full_player_clears_an_earlier_hold() {
    let mut settings = reading_env(SettingsData::default);
    settings.full_player_geometry =
        Some(FullPlayerGeometry { width: 1200.0, height: 800.0, position: None });
    let full_player = recorded(placed(1200.0, 800.0), Instant::now());

    write_snapshot(&mut settings, full_player, None);

    assert_eq!(settings.full_player_geometry, None);
}

// Launched maximized and closed that way, the mirror only ever saw the screen's size.
#[test]
fn a_maximized_close_keeps_the_restore_geometry() {
    let mut settings = reading_env(SettingsData::default);
    settings.window_width = 1200.0;
    settings.window_height = 800.0;
    let screen = recorded(maximized(2560.0, 1440.0), Instant::now());

    write_snapshot(&mut settings, screen, None);

    let tolerance = f64::from(TOLERANCE);
    assert!(
        (settings.window_width - 1200.0).abs() < tolerance
            && (settings.window_height - 800.0).abs() < tolerance,
        "the restore geometry became {}×{}",
        settings.window_width,
        settings.window_height
    );
}

#[test]
fn a_close_with_no_known_position_leaves_the_saved_one() {
    let mut settings = reading_env(SettingsData::default);
    settings.window_x = 40.0;
    settings.window_y = 30.0;
    let unread = recorded(unplaced(1000.0, 700.0), Instant::now());

    write_snapshot(&mut settings, unread, None);

    let tolerance = f64::from(TOLERANCE);
    assert!(
        (settings.window_x - 40.0).abs() < tolerance
            && (settings.window_y - 30.0).abs() < tolerance,
        "the saved position became ({}, {})",
        settings.window_x,
        settings.window_y
    );
}

#[test]
fn a_saved_hold_is_what_a_miniplayer_launch_restores_to() {
    let saved = FullPlayerGeometry { width: 1200.0, height: 800.0, position: None };

    let hold = hold_at_launch(Some(saved), true);

    let tolerance = f64::from(TOLERANCE);
    assert!(
        matches!(hold, Some(held)
            if (held.geom.width - 1200.0).abs() < tolerance
                && (held.geom.height - 800.0).abs() < tolerance),
        "the launch held {hold:?}"
    );
}

/// A hand-edited size can open the miniplayer with nothing saved, and holding nothing would leave
/// the restore caption only the miniplayer's own placement to go back to.
#[test]
fn a_miniplayer_launch_with_nothing_saved_holds_the_first_launch_placement() {
    let first_launch = reading_env(Placement::first_launch);

    let hold = reading_env(|| hold_at_launch(None, true));

    let tolerance = f64::from(TOLERANCE);
    assert!(
        matches!(hold, Some(held)
            if (held.geom.width - first_launch.geom.width).abs() < tolerance
                && (held.geom.height - first_launch.geom.height).abs() < tolerance),
        "the launch held {hold:?}"
    );
}

/// `hold_full_player` keeps a hold already there, so one seeded here would stand in for the
/// placement the window settles at before the user shrinks it.
#[test]
fn a_full_player_launch_holds_nothing() {
    let hold = reading_env(|| hold_at_launch(None, false));

    assert!(hold.is_none(), "the launch held {hold:?}");
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
