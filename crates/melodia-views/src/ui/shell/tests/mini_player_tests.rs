//! Source walks over the miniplayer's `active-changed` wiring, whose two edges only a live event
//! loop and a real swap produce.

use melodia_testkit::{block_after, strip_line_comments};

/// The `active-changed` closure's body, comments stripped.
fn active_changed_body() -> String {
    let code = strip_line_comments(include_str!("../mini_player.rs"));
    block_after(&code, "mini.on_active_changed(").to_owned()
}

/// A launch settles through an exit that followed no entry, and `f` opens Now Playing under the
/// miniplayer ahead of its swap. Releasing on either hands back the cover and sheet a surface is
/// about to draw, with no edge left to ask for them again.
#[test]
fn an_exit_with_no_entry_before_it_releases_nothing() {
    const GUARD: &str = "if !was_visible";
    let body = active_changed_body();
    let exit = block_after(&body, "} else");
    let guard_at = exit.find(GUARD);
    let artwork_at = exit.find("release_artwork_off_thread(");
    let lyrics_at = exit.find("release_lyrics(");

    assert!(!exit.is_empty(), "no exit arm found: the walk is broken, not the code");
    assert!(
        body.contains("let was_visible = np_state.mini_visible.replace(is_active);"),
        "`was_visible` no longer reads the mirror as it stood before this edge:\n{body}"
    );
    assert_eq!(
        block_after(exit, GUARD).trim(),
        "return;",
        "an exit with no entry before it no longer returns early:\n{exit}"
    );
    assert!(
        matches!((guard_at, artwork_at, lyrics_at), (Some(guard), Some(artwork), Some(lyrics))
            if guard < artwork && guard < lyrics),
        "the guard has to run before both releases:\n{exit}"
    );
}

/// Entering force-closes Now Playing, and whether that close runs before or after this arm decides
/// whether the sheet is still there, so the ask can't wait on what the artwork gate answers.
#[test]
fn entering_asks_for_the_sheet_outside_the_artwork_guard() {
    const LYRICS_KICK: &str = "np_state.kick_lyrics();";
    let body = active_changed_body();
    let entry = block_after(&body, "if is_active");

    assert!(!entry.is_empty(), "no entry arm found: the walk is broken, not the code");
    assert!(entry.contains(LYRICS_KICK), "entering no longer asks for the sheet:\n{entry}");
    assert!(
        !block_after(entry, "if np_state.renders_artwork()").contains(LYRICS_KICK),
        "the sheet is asked for only when the artwork gate opens:\n{entry}"
    );
}
