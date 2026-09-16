//! Source pins for the native title bar dropping its frame under the miniplayer, and for the
//! outline and resize grabs a frameless window draws in the frame's place.
//!
//! Neither half can go wrong where CI looks. A shell binding reading the setting instead of
//! `frameless` is right under the custom titlebar and wrong only for the native miniplayer, and
//! the exit edge and the frame's timing matter only where dropping a frame grows the client area,
//! Win32 and macOS. The kept margins matter on Win32 alone, the one frame with invisible edges. The
//! Rust half of the frame reading is pinned beside `window_chrome::geometry` and its `Resized` arm.

use melodia_testkit::{binding_value, blocks_named, code_tokens};

const APP_WINDOW: &str = include_str!("../../../../melodia-ui/ui/app-window.slint");
const MINI_SWITCH: &str =
    include_str!("../../../../melodia-ui/ui/components/mini-player-switch.slint");
const THEME: &str = include_str!("../../../../melodia-ui/ui/theme.slint");
const RESIZE_RING: &str = include_str!("../../../../melodia-ui/ui/layout/resize-ring.slint");
const EDGE_OUTLINE: &str = include_str!("../../../../melodia-ui/ui/components/edge-outline.slint");

/// The rect the shell takes inside the client, as tokens.
const CONTENT_RECT: &str =
    "x: root.margin-left; y: 0px; width: root.content-width; height: root.content-height;";

/// Reading the setting at any of these leaves the native miniplayer with an OS frame over it, a
/// transparent window with square corners, or no resize edge to grow it back out through.
#[test]
fn every_frame_question_in_the_shell_reads_frameless() {
    const BINDINGS: [&str; 5] = [
        "no-frame: root.frameless;",
        "property <bool> rounded-shell: root.frameless && !WindowChrome.is-maximized;",
        "background: root.rounded-shell ? Colors.transparent : Theme.mantle;",
        "border-radius: root.rounded-shell ? Theme.window-radius : 0px;",
        "active: root.rounded-shell;",
    ];
    let shell = code_tokens(APP_WINDOW);

    let missing: Vec<&str> = BINDINGS.into_iter().filter(|b| !shell.contains(b)).collect();

    assert!(missing.is_empty(), "these frame bindings no longer read `frameless`:\n{missing:#?}");
}

/// Win32 hands a dropped frame's invisible borders to the client, so only a frameless window that
/// had a frame keeps them clear. Under the custom titlebar there was never a frame to line up with,
/// and a maximized window fills the work area edge to edge.
#[test]
fn the_frame_margins_are_kept_only_by_the_native_miniplayer() {
    let shell = code_tokens(APP_WINDOW);

    let keeps = binding_value(&shell, "property <bool> keeps-frame-margins:").trim();

    assert_eq!(
        keeps, "Theme.use-native-titlebar && root.rounded-shell",
        "the shell keeps the frame's margins for a window that never dropped one"
    );
}

/// Each side reads its own measurement, and the content rect gives each one up. A side reading its
/// neighbour is invisible on Windows 11, where all three are the same width, and wrong wherever a
/// frame isn't.
#[test]
fn the_content_rect_gives_up_each_kept_margin_on_its_own_side() {
    const BINDINGS: [&str; 5] = [
        "property <length> margin-left: root.keeps-frame-margins ? WindowChrome.frame-margin-left : 0px;",
        "property <length> margin-right: root.keeps-frame-margins ? WindowChrome.frame-margin-right : 0px;",
        "property <length> margin-bottom: root.keeps-frame-margins ? WindowChrome.frame-margin-bottom : 0px;",
        "property <length> content-width: root.width - root.margin-left - root.margin-right;",
        "property <length> content-height: root.height - root.margin-bottom;",
    ];
    let shell = code_tokens(APP_WINDOW);

    let missing: Vec<&str> = BINDINGS.into_iter().filter(|b| !shell.contains(b)).collect();

    assert!(missing.is_empty(), "these margin bindings no longer hold:\n{missing:#?}");
}

/// The first-run card, the dialog overlay, the toast stack and the window outline take the content
/// rect by mounting inside the shell. One of them left full-client paints into the transparent
/// margins: a dialog's backdrop as a dark strip around the miniplayer, a toast hanging off its edge,
/// an outline round nothing.
#[test]
fn the_shell_and_every_overlay_over_it_sit_inside_the_kept_margins()
-> Result<(), Box<dyn std::error::Error>> {
    const MOUNTS: [&str; 4] = [
        "if Onboarding.mounted: OnboardingOverlay {",
        "DialogOverlay {",
        "NotificationStack {",
        "if root.rounded-shell && Settings.window-border-shown: EdgeOutline {",
    ];
    let shell = code_tokens(APP_WINDOW);
    let shell_body = blocks_named(&shell, "Rectangle")
        .into_iter()
        .find(|body| body.trim_start().starts_with(CONTENT_RECT))
        .ok_or("no `Rectangle` takes the content rect: the walk is broken, not the shell")?;

    let outside: Vec<&str> =
        MOUNTS.into_iter().filter(|mount| !shell_body.contains(mount)).collect();

    assert!(
        outside.is_empty(),
        "these no longer mount inside the shell's content rect: {outside:?}"
    );
    Ok(())
}

/// Gated on the setting instead, the outline draws inside the full window's OS frame under the
/// native titlebar, and along the screen edge of a maximized window.
#[test]
fn the_outline_mounts_only_where_no_frame_draws_one() {
    let shell = code_tokens(APP_WINDOW);

    let gated = shell
        .matches("if root.rounded-shell && Settings.window-border-shown: EdgeOutline {")
        .count();

    assert_eq!(gated, 1, "the window outline no longer mounts under the frameless gate");
}

/// A logical pixel is two physical ones at 200 % and a blurred one and a half at 150 %, where the
/// OS draws exactly one at every scale. The outline asks for one, and `EdgeOutline` strokes it
/// pushed out by a physical pixel the clip cuts away, so a stroke not pushed out shows both.
#[test]
fn the_outline_is_one_physical_pixel() {
    const OUTLINE_BINDINGS: [&str; 3] = [
        "private property <length> bleed: 1phx;",
        "x: -root.bleed; y: -root.bleed; width: parent.width + 2 * root.bleed; height: parent.height + 2 * root.bleed;",
        "border-width: root.stroke-width + root.bleed;",
    ];
    let shell = code_tokens(APP_WINDOW);
    let outline = code_tokens(EDGE_OUTLINE);

    let asked: Vec<bool> = blocks_named(&shell, "EdgeOutline")
        .iter()
        .map(|mount| mount.contains("stroke-width: 1phx;"))
        .collect();
    let missing: Vec<&str> =
        OUTLINE_BINDINGS.into_iter().filter(|b| !outline.contains(b)).collect();

    assert_eq!(asked, [true], "the window outline no longer asks for one physical pixel");
    assert!(
        missing.is_empty(),
        "`EdgeOutline` no longer leaves exactly the stroke it is asked for:\n{missing:#?}"
    );
}

/// The outline is pushed out by one physical pixel, so its radius takes that pixel on top of the
/// shell's. Any other leaves its corners cutting across the shell's or standing off it inside.
#[test]
fn the_outline_rounds_with_the_shell() {
    let shell = code_tokens(APP_WINDOW);
    let outline = code_tokens(EDGE_OUTLINE);

    let handed: Vec<bool> = blocks_named(&shell, "EdgeOutline")
        .iter()
        .map(|mount| mount.contains("radius: Theme.window-radius;"))
        .collect();
    let radius = binding_value(&outline, "border-radius:").trim();

    assert_eq!(handed, [true], "the window outline's mount no longer hands it the shell's radius");
    assert_eq!(
        radius, "root.radius + root.bleed",
        "`EdgeOutline`'s corners no longer take the pixel it is pushed out by"
    );
}

/// Swapped, System paints the Windows accent round every window but the focused one.
#[test]
fn the_outline_takes_the_unfocused_colour_only_on_an_unfocused_window() {
    let shell = code_tokens(APP_WINDOW);

    let color = binding_value(&shell, "stroke-color: Theme.window-focused").trim();

    assert_eq!(
        color, "? WindowChrome.border-color : WindowChrome.border-color-unfocused",
        "the outline no longer picks its colour off the window's focus"
    );
}

/// The margins were the frame's resize borders, and they stay transparent, so they have to stay a
/// resize band too. Narrower than a margin, the rest of it is a strip of window that takes the
/// click and does nothing.
#[test]
fn the_resize_band_covers_every_kept_margin() {
    let shell = code_tokens(APP_WINDOW);

    let band = binding_value(&shell, "property <length> resize-band:").trim();

    assert_eq!(
        band, "max(Theme.resize-border, root.margin-left, root.margin-right, root.margin-bottom)",
        "the resize band no longer reaches across the kept margins"
    );
}

/// The band is the one widened over the kept margins, and the ring is all that resizes from it. On
/// a band of its own, the margin strip past it takes the click and does nothing.
#[test]
fn the_resize_ring_is_handed_the_windows_band() {
    let shell = code_tokens(APP_WINDOW);

    let handed = shell.matches("band: root.resize-band;").count();

    assert_eq!(handed, 1, "the ring's mount no longer hands it the window's resize band");
}

/// The other half: an edge inside the ring reading the theme's band directly ignores the one it
/// was handed, and the strip between the two widths drifts exactly as a band of its own would.
#[test]
fn the_resize_ring_reads_no_band_but_the_one_it_is_handed() {
    let ring = code_tokens(RESIZE_RING);

    let own = ring.matches("Theme.resize-border").count();

    assert_eq!(own, 1, "a ring edge reads the theme's band past the `band` default");
}

/// Slint's handler for this binding hit-tests square corners a band wide and compares a logical
/// width with a physical cursor. Beside the ring it takes presses the ring's cursor says mean
/// something else: no diagonal anywhere on a rounded corner, and a thinner edge on a scaled display.
#[test]
fn the_shell_leaves_every_resize_press_to_the_ring() {
    let shell = code_tokens(APP_WINDOW);

    let bound = shell.matches("resize-border-width").count();

    assert_eq!(bound, 0, "the shell binds `resize-border-width` beside the ring again");
}

/// Unmounted, the ring can't move its zone back to `none`, so the last grab the pointer was over
/// stays armed in Rust and the next click anywhere, a maximized window's included, resizes.
#[test]
fn the_resize_ring_is_never_behind_an_if() {
    let shell = code_tokens(APP_WINDOW);

    let mounts = blocks_named(&shell, "ResizeRing").len();
    let conditional = shell.matches(": ResizeRing {").count();

    assert_eq!((mounts, conditional), (1, 0), "the ring is no longer mounted once and for good");
}

/// The corners grow along the curve they are handed. Left at the default, every preset gets the
/// square window's corner, whose diagonal sits in the transparent cut-out outside a rounded one.
#[test]
fn the_resize_ring_is_handed_the_windows_radius() {
    let shell = code_tokens(APP_WINDOW);
    let mounts = blocks_named(&shell, "ResizeRing");

    let handed: Vec<bool> =
        mounts.iter().map(|mount| mount.contains("radius: Theme.window-radius;")).collect();

    assert_eq!(handed, [true], "the ring's mount no longer hands it the window's corner radius");
}

/// The grabs are hover, and hover goes to whatever sits on top. Under an overlay, an open dialog
/// or the first-run card takes resizing away, which Slint's own handler never let it do.
#[test]
fn the_resize_ring_mounts_above_every_overlay() -> Result<(), Box<dyn std::error::Error>> {
    const OVERLAYS: [&str; 3] = ["OnboardingOverlay {", "DialogOverlay {", "NotificationStack {"];
    let shell = code_tokens(APP_WINDOW);
    let (under_ring, _) = shell
        .split_once("ResizeRing {")
        .ok_or("no `ResizeRing {` mount found: the walk is broken, not the shell")?;

    let not_under: Vec<&str> =
        OVERLAYS.into_iter().filter(|overlay| !under_ring.contains(overlay)).collect();

    assert!(
        not_under.is_empty(),
        "these overlays aren't declared before the resize ring, so it can't sit above them: \
         {not_under:?}"
    );
    Ok(())
}

/// Enabled while the ring is off, a grab still steals hover and shows a resize cursor, over a
/// maximized window's close button or a native frame's content, for a press nothing acts on.
#[test]
fn every_grab_in_the_ring_switches_off_with_it() {
    let ring = code_tokens(RESIZE_RING);

    let touch_areas = ring.matches("TouchArea {").count();
    let enabled = ring.matches("enabled: root.active;").count();
    let corners_handed = ring.matches("active: root.active;").count();

    assert_eq!(
        (touch_areas, enabled, corners_handed),
        (7, 7, 4),
        "a grab (four edges, three per corner, four corners) no longer follows `active`"
    );
}

/// The `!active` arm is what clears a zone when the ring switches off under the pointer, a
/// maximize by keyboard with the cursor on an edge being the usual way.
#[test]
fn the_resize_zone_is_none_whenever_the_ring_is_off() {
    let ring = code_tokens(RESIZE_RING);

    let zone = binding_value(&ring, "property <ResizeZone> zone:").trim();

    assert!(
        zone.starts_with("!root.active ? ResizeZone.none :"),
        "the zone no longer answers `none` first while the ring is off:\n{zone}"
    );
}

/// A direction missing from the binding is a grab that shows its cursor and resizes nothing.
#[test]
fn the_resize_zone_names_every_direction_a_grab_can_take() {
    const DIRECTIONS: [&str; 8] = [
        "ResizeZone.north :",
        "ResizeZone.south :",
        "ResizeZone.west :",
        "ResizeZone.east :",
        "ResizeZone.north-west :",
        "ResizeZone.north-east :",
        "ResizeZone.south-west :",
        "ResizeZone.south-east :",
    ];
    let ring = code_tokens(RESIZE_RING);
    let zone = binding_value(&ring, "property <ResizeZone> zone:");

    let missing: Vec<&str> = DIRECTIONS.into_iter().filter(|d| !zone.contains(d)).collect();

    assert!(missing.is_empty(), "the zone never answers {missing:?}:\n{zone}");
}

/// Without the callback the ring still sets every cursor, and no press ever resizes.
#[test]
fn the_resize_zone_reaches_the_window_chrome() {
    let ring = code_tokens(RESIZE_RING);

    let handed = ring.matches("changed zone => { WindowChrome.resize-zone-changed(root.zone); }");

    assert_eq!(handed.count(), 1, "the ring's zone no longer reaches `WindowChrome`");
}

/// Slint gives hover to the last-declared item under the pointer. With the corners first, every
/// arm they share with an edge resizes along that edge's one axis.
#[test]
fn the_corners_are_declared_after_the_edges() -> Result<(), Box<dyn std::error::Error>> {
    let ring = code_tokens(RESIZE_RING);
    let (_, from_first_corner) = ring
        .split_once(":= ResizeCorner {")
        .ok_or("no corner mount found: the walk is broken, not the ring")?;

    let edges_after = from_first_corner.matches("TouchArea {").count();

    assert_eq!(edges_after, 0, "an edge is declared after a corner it has to lose to");
    Ok(())
}

/// The rounded run is the diagonal. Measured from the window edge instead of the band, the native
/// miniplayer's corners end inside the transparent margin, and on the radius alone a square
/// window's corner shrinks to nothing.
#[test]
fn the_corner_arms_run_the_curve_and_a_band_past_it() {
    let ring = code_tokens(RESIZE_RING);

    let reach = binding_value(&ring, "private property <length> reach:").trim();

    assert_eq!(reach, "root.band * 2 + root.radius", "the corner arms no longer follow the radius");
}

/// An elbow of the band alone leaves the middle of a rounded corner with its grab in the cut-out
/// only. The ring argues the half radius.
#[test]
fn the_corner_elbow_follows_the_curve_inward() {
    let ring = code_tokens(RESIZE_RING);

    let elbow = binding_value(&ring, "private property <length> elbow:").trim();

    assert_eq!(
        elbow, "root.band + max(root.band, root.radius / 2)",
        "the corner elbow no longer follows the curve"
    );
}

/// The native miniplayer paints its own outline once the OS frame goes, and on the custom
/// titlebar's radius that outline changes shape mid-swap wherever the user's pick differs from the
/// host's: the frame's rounding one tick, Melodia's the next.
#[test]
fn the_native_miniplayer_rounds_like_the_frame_it_replaced() {
    let theme = code_tokens(THEME);

    let radius = binding_value(&theme, "out property <length> window-radius:").trim();

    assert_eq!(
        radius, "use-native-titlebar ? native-content-radius : shell-radius",
        "`Theme.window-radius` no longer takes the host's radius under the native titlebar"
    );
}

/// The frame changes on the tick the branches swap, the outgoing one faded out and the crossfade
/// still opaque. On `active` the full UI fades out with neither the OS frame nor the custom
/// titlebar, which doesn't mount under the native setting, and the miniplayer fades out under a
/// frame that is already back. Win32 then steps the visible window in by its invisible resize
/// borders while the miniplayer is still on screen, which is what taking it early looked like.
#[test]
fn the_frame_drops_with_the_mounted_miniplayer_not_the_threshold() {
    let shell = code_tokens(APP_WINDOW);

    let frameless = binding_value(&shell, "property <bool> frameless:").trim();

    assert_eq!(
        frameless, "!Theme.use-native-titlebar || mini-switch.render-active",
        "`frameless` no longer follows the mounted branch"
    );
}

/// Win32 takes a resize drag's minimum once, when the drag starts, as the window's minimum plus
/// whatever frame stands then. A drag from the full UI starts framed and ends frameless, so a
/// minimum that keeps the frame's share holds the miniplayer that far above its floor until the
/// button is released and a second drag asks again.
#[test]
fn the_framed_window_minimum_gives_up_the_frame_the_miniplayer_drops() {
    const BINDINGS: [&str; 2] = [
        "min-width: MiniPlayer.window-min-width - (root.frameless ? 0px : WindowChrome.frame-allowance-w);",
        "min-height: MiniPlayer.window-min-height - (root.frameless ? 0px : WindowChrome.frame-allowance-h);",
    ];
    let shell = code_tokens(APP_WINDOW);

    let missing: Vec<&str> = BINDINGS.into_iter().filter(|b| !shell.contains(b)).collect();

    assert!(
        missing.is_empty(),
        "the window minimum no longer gives up the frame while one stands:\n{missing:#?}"
    );
}

/// Dropping the frame grows the client area by the frame, so an exit edge without the allowance
/// sits inside the size the miniplayer has just grown to.
#[test]
fn the_exit_edge_widens_by_the_frame_allowance() {
    const TERMS: [&str; 3] =
        ["root.render-active ?", "root.exit-allowance-w", "root.exit-allowance-h"];
    let switch = code_tokens(MINI_SWITCH);
    let active = binding_value(&switch, "out property <bool> active:");

    let missing: Vec<&str> = TERMS.into_iter().filter(|t| !active.contains(t)).collect();

    assert!(
        missing.is_empty(),
        "`active` no longer widens its exit edge by the frame, missing {missing:?}:\n{active}"
    );
}

/// Left at its default the allowance is zero, and the exit edge sits back on the entry edge.
#[test]
fn the_shell_hands_the_switch_the_measured_frame() {
    const BINDINGS: [&str; 2] = [
        "exit-allowance-w: WindowChrome.frame-allowance-w;",
        "exit-allowance-h: WindowChrome.frame-allowance-h;",
    ];
    let shell = code_tokens(APP_WINDOW);

    let missing: Vec<&str> = BINDINGS.into_iter().filter(|b| !shell.contains(b)).collect();

    assert!(missing.is_empty(), "the switch no longer receives the frame reading:\n{missing:#?}");
}
