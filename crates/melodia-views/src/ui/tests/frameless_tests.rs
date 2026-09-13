//! Source pins for the native title bar dropping its frame under the miniplayer.
//!
//! Neither half can go wrong where CI looks. A shell binding reading the setting instead of
//! `frameless` is right under the custom titlebar and wrong only for the native miniplayer, and
//! the exit edge and the latch's timing matter only where dropping a frame grows the client area,
//! Win32 and macOS. The Rust half of the frame reading is pinned beside `window_chrome::geometry`
//! and its `Resized` arm.

use melodia_testkit::{binding_value, normalize_ws, strip_line_comments};

const APP_WINDOW: &str = include_str!("../../../../melodia-ui/ui/app-window.slint");
const MINI_SWITCH: &str =
    include_str!("../../../../melodia-ui/ui/components/mini-player-switch.slint");
const THEME: &str = include_str!("../../../../melodia-ui/ui/theme.slint");

/// Comments stripped and whitespace collapsed, so a pin reads tokens rather than one layout.
fn tokens(src: &str) -> String {
    normalize_ws(&strip_line_comments(src))
}

/// Reading the setting at any of these leaves the native miniplayer with an OS frame over it, a
/// transparent window with square corners, or no resize edge to grow it back out through.
#[test]
fn every_frame_question_in_the_shell_reads_frameless() {
    const BINDINGS: [&str; 5] = [
        "no-frame: root.frameless;",
        "resize-border-width: root.frameless ? Theme.resize-border : 0px;",
        "background: (!root.frameless || WindowChrome.is-maximized) ? Theme.mantle : Colors.transparent;",
        "border-radius: (!root.frameless || WindowChrome.is-maximized) ? 0px : Theme.window-radius;",
        "if root.frameless && !WindowChrome.is-maximized: ResizeRing {",
    ];
    let shell = tokens(APP_WINDOW);

    let missing: Vec<&str> = BINDINGS.into_iter().filter(|b| !shell.contains(b)).collect();

    assert!(missing.is_empty(), "these frame bindings no longer read `frameless`:\n{missing:#?}");
}

/// The native miniplayer paints its own outline once the OS frame goes, and on the custom
/// titlebar's radius that outline changes shape mid-swap wherever the user's pick differs from the
/// host's: the frame's rounding one tick, Melodia's the next.
#[test]
fn the_native_miniplayer_rounds_like_the_frame_it_replaced() {
    let theme = tokens(THEME);

    let radius = binding_value(&theme, "out property <length> window-radius:").trim();

    assert_eq!(
        radius, "use-native-titlebar ? native-content-radius : shell-radius",
        "`Theme.window-radius` no longer takes the host's radius under the native titlebar"
    );
}

/// On `active` the frame would drop while the full UI is still fading out, leaving it with neither
/// the OS frame nor the custom titlebar, which doesn't mount under the native setting.
#[test]
fn the_frame_follows_the_switch_latch_not_the_threshold() {
    let shell = tokens(APP_WINDOW);

    let frameless = binding_value(&shell, "property <bool> frameless:").trim();

    assert_eq!(
        frameless, "!Theme.use-native-titlebar || mini-switch.frame-dropped",
        "`frameless` no longer follows the switch's `frame-dropped` latch"
    );
}

/// The frame drops once the miniplayer is mounted and returns at the decision to leave, ahead of
/// the full UI's mount. Cleared only at the swap, it comes back one pump after that mount and
/// resizes the page it just built. Copied before `render-active` rather than after, it takes the
/// branch being left, and a completed leave drops the frame over the full UI.
#[test]
fn the_frame_latch_returns_at_the_exit_decision_and_follows_each_swap() {
    const EXIT_ARM: &str = "if (!root.watched-active) { root.frame-dropped = false; }";
    const FOLLOW: &str =
        "root.render-active = root.active; root.frame-dropped = root.render-active;";
    let switch = tokens(MINI_SWITCH);

    assert!(
        switch.contains(EXIT_ARM),
        "the exit decision no longer brings the frame back ahead of the full UI's mount"
    );
    assert_eq!(
        switch.matches(FOLLOW).count(),
        2,
        "the seed and swap timers no longer both set the frame from the branch they mounted"
    );
}

/// Dropping the frame grows the client area by the frame, so an exit edge without the allowance
/// sits inside the size the miniplayer has just grown to.
#[test]
fn the_exit_edge_widens_by_the_frame_allowance() {
    const TERMS: [&str; 3] =
        ["root.frame-dropped ?", "root.exit-allowance-w", "root.exit-allowance-h"];
    let switch = tokens(MINI_SWITCH);
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
    let shell = tokens(APP_WINDOW);

    let missing: Vec<&str> = BINDINGS.into_iter().filter(|b| !shell.contains(b)).collect();

    assert!(missing.is_empty(), "the switch no longer receives the frame reading:\n{missing:#?}");
}
