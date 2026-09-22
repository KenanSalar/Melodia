use super::*;
use melodia_testkit::{block_after, reading_env, strip_line_comments};

/// Sub-pixel slack for a length divided back out of physical pixels.
const TOLERANCE: f32 = 0.001;

fn is_allowance(got: WinitLogicalSize<f32>, width: f32, height: f32) -> bool {
    (got.width - width).abs() < TOLERANCE && (got.height - height).abs() < TOLERANCE
}

fn assert_allowance(got: WinitLogicalSize<f32>, width: f32, height: f32) {
    assert!(
        is_allowance(got, width, height),
        "expected {width}×{height}, got {}×{}",
        got.width,
        got.height
    );
}

/// `geometry.rs`, comments stripped, for the walks below.
fn geometry_source() -> String {
    strip_line_comments(include_str!("../geometry.rs"))
}

// The case the miniplayer's exit edge exists for: the frame goes, the client takes the whole
// window rect, and it grows by exactly this much.
#[test]
fn a_frame_measures_what_its_going_gave_the_client() {
    let frame =
        step_between(WinitPhysicalSize::new(800, 600), WinitPhysicalSize::new(816, 639), 1.0);
    assert_allowance(frame, 16.0, 39.0);
}

// `MiniPlayerSwitch` compares its edges in logical pixels, so the reading has to be in them too.
#[test]
fn a_scale_factor_divides_the_frame_back_into_logical_pixels() {
    let frame =
        step_between(WinitPhysicalSize::new(1200, 900), WinitPhysicalSize::new(1224, 960), 1.5);
    assert_allowance(frame, 16.0, 40.0);
}

// Where the window manager takes a dropped frame out of the window rather than handing it to the
// client, there is no step and the exit edge is the entry edge.
#[test]
fn a_frame_the_client_never_gained_measures_nothing() {
    let frame =
        step_between(WinitPhysicalSize::new(800, 600), WinitPhysicalSize::new(800, 600), 1.0);
    assert_allowance(frame, 0.0, 0.0);
}

// Saturating, because a wrapped `u32` is a step past any window size, and the miniplayer would
// never let go.
#[test]
fn a_client_that_shrank_when_the_frame_went_never_wraps() {
    let frame =
        step_between(WinitPhysicalSize::new(801, 602), WinitPhysicalSize::new(800, 600), 1.0);
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

/// Either term alone has to withhold the client, and the decoration is deliberately not a term:
/// the step is measured across a decoration change, so a reading from either side of one is a
/// reading to keep. What a maximized or minimized one gives is a step the miniplayer's exit edge
/// then widens by for no frame at all.
#[test]
fn only_a_restored_window_is_one_to_measure() {
    const LIVE: WinitPhysicalSize<u32> = WinitPhysicalSize::new(800, 600);
    const MINIMIZED: WinitPhysicalSize<u32> = WinitPhysicalSize::new(0, 0);
    // (decorated, maximized, client, measurable)
    let table = [
        (true, false, LIVE, Some(LIVE)),
        (false, false, LIVE, Some(LIVE)),
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

/// A reading, for the step walks below.
fn framing(decorated: bool, client: WinitPhysicalSize<u32>, scale: f64) -> WindowReading {
    WindowReading { client, scale, maximized: false, decorated }
}

/// A decoration change the reading under test could plausibly be the OS answering.
const FRESH_FLIP: Option<Duration> = Some(Duration::from_millis(16));

fn assert_step(got: Option<WinitLogicalSize<f32>>, width: f32, height: f32) {
    assert!(
        matches!(got, Some(step) if is_allowance(step, width, height)),
        "expected a step of {width}×{height}, got {got:?}"
    );
}

/// The miniplayer's entry under a native titlebar, on the desktop no query can answer for: the
/// decoration goes, the window manager keeps the window's rect, and the client takes the titlebar.
/// Short by it, the exit edge sits inside the size the drop has just grown the window to and the
/// miniplayer bounces straight back out.
#[test]
fn the_frame_going_measures_what_the_client_gained() {
    let before = framing(true, WinitPhysicalSize::new(936, 227), 1.0);
    let now = framing(false, WinitPhysicalSize::new(936, 255), 1.0);

    assert_step(step_across(before, now, FRESH_FLIP), 0.0, 28.0);
}

/// The same step the other way, which is the one the Full Player caption needs: it asks for the
/// client that leaves the held size once the frame is back, so a leave measured as nothing restores
/// a window a titlebar short, and shorter again every round trip.
#[test]
fn the_frame_returning_measures_the_same_step() {
    let before = framing(false, WinitPhysicalSize::new(936, 255), 1.0);
    let now = framing(true, WinitPhysicalSize::new(936, 227), 1.0);

    assert_step(step_across(before, now, FRESH_FLIP), 0.0, 28.0);
}

/// Every reading a resize drag delivers wears the decoration the one before it did. Measured across
/// those, the step is the drag's own motion and the exit edge follows the pointer.
#[test]
fn two_readings_under_one_decoration_measure_nothing() {
    let before = framing(true, WinitPhysicalSize::new(936, 400), 1.0);
    let now = framing(true, WinitPhysicalSize::new(936, 227), 1.0);

    assert_eq!(
        step_across(before, now, FRESH_FLIP),
        None,
        "a drag between two readings is not a frame"
    );
    assert_eq!(
        step_across(
            framing(false, before.client, 1.0),
            framing(false, now.client, 1.0),
            FRESH_FLIP
        ),
        None,
        "and neither is one the miniplayer is already up for"
    );
}

/// **X11 and a Wayland compositor drawing its own decorations both answer a dropped frame with no
/// size at all**, the frame coming off the window rather than going to the client. Unbounded, the
/// next resize of any kind is then the first reading to disagree with the last about the
/// decoration, and the leave's own resize measures the whole miniplayer-to-full-player step as a
/// frame: the exit edge lands above the window the restore just asked for and the miniplayer never
/// leaves again.
#[test]
fn a_resize_no_decoration_change_explains_measures_nothing() {
    let mini = framing(true, WinitPhysicalSize::new(540, 240), 1.0);
    let restored = framing(false, WinitPhysicalSize::new(1200, 800), 1.0);
    let stale = Some(FRAME_STEP_GRACE + Duration::from_millis(1));

    assert_eq!(
        step_across(mini, restored, stale),
        None,
        "a decoration change too old to have moved this client is not what moved it"
    );
    assert_eq!(
        step_across(mini, restored, None),
        None,
        "and a client that has crossed no decoration change at all has taken no frame"
    );
}

/// The step is two physical sizes subtracted, so it means nothing across a move to a monitor that
/// reports the window at a different scale: divided by either factor the answer is wrong by their
/// ratio, and it sticks for the session.
#[test]
fn two_readings_under_different_scales_measure_nothing() {
    let before = framing(true, WinitPhysicalSize::new(936, 227), 1.0);
    let now = framing(false, WinitPhysicalSize::new(1872, 510), 2.0);

    assert_eq!(step_across(before, now, FRESH_FLIP), None);
}

/// `MiniPlayerSwitch` compares its edges in logical pixels, so the step has to reach it in them.
#[test]
fn the_step_reaches_the_exit_edge_in_logical_pixels() {
    let before = framing(true, WinitPhysicalSize::new(1404, 340), 1.5);
    let now = framing(false, WinitPhysicalSize::new(1404, 382), 1.5);

    assert_step(step_across(before, now, FRESH_FLIP), 0.0, 28.0);
}

/// The memo is the reading the next one is a step away from, so it has to move whatever this
/// reading decides. Left only where a step was measured, every later reading is compared against
/// the launch and the first drag past the threshold answers with its own travel. A source walk
/// because both the record and the answer sit behind a process-wide lock.
#[test]
fn a_reading_that_measures_nothing_is_still_the_one_to_measure_from() {
    const FN: &str = "pub fn frame_allowance";
    const RECORD: &str = "last_framing().lock().replace(reading)?";
    let code = geometry_source();
    let body = block_after(&code, FN);

    assert!(!body.is_empty(), "no `{FN}` found: the walk is broken, not the code");
    assert!(
        body.contains(RECORD),
        "`frame_allowance` no longer records every reading it is handed:\n{body}"
    );
    assert!(
        matches!((body.find("measurable_client(reading)?"), body.find(RECORD)),
            (Some(gate), Some(record)) if gate < record),
        "the gate has to run before the record, or a maximized or minimized reading becomes the \
         one the next step is measured from:\n{body}"
    );
}

/// The announcement is the whole of what tells a frame step apart from an ordinary resize, and
/// unwired it can only fail silently: every step goes stale, the allowance stays at zero, and the
/// miniplayer is back to an exit edge short by a titlebar wherever the client really does grow by
/// one. A source walk because `install` wants a window and the whole app state.
#[test]
fn the_decoration_change_reaches_the_step_that_needs_it() {
    const CALL: &str = "geometry::wire_frame_changes(app);";
    let code = strip_line_comments(include_str!("../mod.rs"));
    let body = block_after(&code, "pub fn install");

    assert!(!body.is_empty(), "no `pub fn install` found: the walk is broken, not the code");
    assert!(
        body.contains(CALL),
        "nothing takes the window's decoration changes, so no reading can be a frame:\n{body}"
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

/// Once the miniplayer drops the frame, the outer rect is the client, so the margins read zero
/// there and would replace the ones the native miniplayer insets itself by, painting into the
/// invisible borders again. A source walk because answering needs a live, decorated Win32 window,
/// and the body is `cfg`-gated to it.
#[test]
fn an_undecorated_window_gives_no_margins() {
    const FN: &str = "pub fn frame_margins";
    let code = geometry_source();
    let body = block_after(&code, FN);

    assert!(!body.is_empty(), "no `{FN}` found: the walk is broken, not the code");
    assert!(
        body.contains("reading.decorated"),
        "`frame_margins` passes an undecorated window, whose zero reading replaces the borders the \
         native miniplayer keeps transparent:\n{body}"
    );
}

/// Both frame readings answer the same question about when a reading describes a window at all,
/// and a reader that asks its own version is how one of them starts measuring a maximized or
/// minimized one again. What they don't share is the decoration: the margins need a frame
/// standing, where the allowance is the step across its going.
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
    let mut mirror = recorded(placed(800.0, 600.0), t0);
    mirror.observe(placed(1200.0, 800.0), t0 + Duration::from_millis(10));
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

// The first-launch fallback is for the restore caption, which only a miniplayer launch mounts.
#[test]
fn a_full_player_launch_with_nothing_saved_holds_nothing() {
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
