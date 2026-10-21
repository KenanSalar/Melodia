//! A volume step, key or wheel, is read off `Player`, and two of its halves fail silently.
//!
//! A step that skips `bump-volume` still moves the level, so it looks right until a restart puts
//! the old one back. A wheel handler that rejects still steps, and only the hover it drops says so.

use melodia_testkit::{
    MIN_SLINT_SOURCES, UI_DIR, block_after, normalize_ws, strip_line_comments, stripped_sources,
};

const PLAYER: &str = include_str!("../../melodia-ui/ui/globals/player.slint");

/// **Every volume step goes through `Player.bump-volume`.** Its tick is what `ShortcutScope`'s
/// debounce commits on, so a step that calls `step-volume` directly is never written to
/// `settings.json`.
#[test]
fn no_volume_step_skips_the_step_tick() {
    let direct: Vec<String> = stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES)
        .into_iter()
        .filter(|(_, src)| src.contains("Player.step-volume("))
        .map(|(path, _)| path)
        .collect();
    assert!(direct.is_empty(), "these step the volume around the tick: {direct:?}");

    let player = strip_line_comments(PLAYER);
    assert_eq!(
        player.matches("root.step-volume(").count(),
        1,
        "`globals/player.slint` steps the volume somewhere other than `bump-volume`"
    );
    assert!(
        normalize_ws(block_after(&player, "public function bump-volume("))
            .contains("root.step-volume(delta); root.volume-step-tick += 1;"),
        "`bump-volume` no longer steps and ticks together"
    );
}

/// **`Player.wheel-volume` accepts every event, an empty one included.** A `TouchArea` whose
/// `scroll-event` rejects drops `has-hover` until the pointer moves (`slint-pitfalls.md`), and the
/// empty-delta return reads like one to hand on to whatever scrolls underneath.
#[test]
fn the_volume_wheel_never_rejects_an_event() {
    let player = strip_line_comments(PLAYER);
    let wheel = block_after(&player, "public function wheel-volume(");
    assert!(wheel.contains("return accept;"), "the walk no longer finds `wheel-volume`'s body");

    assert!(!wheel.contains("reject"), "`wheel-volume` hands an event back, and the hover with it");
}
