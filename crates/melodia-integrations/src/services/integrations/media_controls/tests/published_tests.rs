//! What the OS panel is told back.
//!
//! A dedupe that reads two different songs as one leaves the panel showing the previous one for
//! as long as the station keeps playing.

use melodia_engine::player::engine::now_playing::SourceId;

use super::*;

/// One track's worth of panel. Spelled out rather than defaulted: `SourceSummary` has no
/// `Default`, every field on it being something a surface actually draws.
fn summary() -> SourceSummary<'static> {
    SourceSummary {
        id: SourceId::Track(1),
        title: "Sunset Drive",
        secondary: Some("The Coastliners"),
        album: Some("Long Way Round"),
        artwork_path: Some("artwork/ab/abcdef.jpg"),
        duration_ms: Some(214_000),
    }
}

/// An empty panel over an empty deck is the one pairing with nothing to write: there is no
/// metadata to clear and no round trip worth spending to clear it.
#[test]
fn nothing_held_and_nothing_playing_needs_no_write() {
    assert!(PublishedMetadata::still_current(None, None));
}

#[test]
fn a_source_arriving_or_leaving_is_always_a_write() {
    let source = summary();
    let held = PublishedMetadata::from(&source);

    assert!(
        !PublishedMetadata::still_current(None, Some(&source)),
        "the panel is empty and the deck is not",
    );
    assert!(
        !PublishedMetadata::still_current(Some(&held), None),
        "and the deck emptied under a panel still showing a song",
    );
}

#[test]
fn a_source_that_would_paint_the_same_panel_needs_no_write() {
    let source = summary();
    let held = PublishedMetadata::from(&source);

    assert!(PublishedMetadata::still_current(Some(&held), Some(&source)));
}

/// Every field the panel draws, moved one at a time. Two of these are the defect the key was taken
/// off the identity for: `secondary` moving is a station announcing its next song, and `title` or
/// `album` moving is a track re-tagged in place. Both keep the id they had, so a comparison that
/// drops one of these fields is invisible except as a panel that stops keeping up.
#[test]
fn each_field_the_panel_draws_is_a_write_of_its_own() {
    let reference = summary();
    let held = PublishedMetadata::from(&reference);
    let differs =
        |source: &SourceSummary<'_>| !PublishedMetadata::still_current(Some(&held), Some(source));

    let retitled = SourceSummary { title: "Sunset Drive (Remastered)", ..reference };
    assert!(differs(&retitled), "a track re-tagged in place");

    let announced = SourceSummary { secondary: Some("Night Bus"), ..reference };
    assert!(differs(&announced), "a station announcing its next song");

    let recompiled = SourceSummary { album: Some("Singles"), ..reference };
    assert!(differs(&recompiled), "the album alone");

    let recovered = SourceSummary { artwork_path: Some("artwork/cd/cdef01.jpg"), ..reference };
    assert!(differs(&recovered), "the cover alone");

    let remastered = SourceSummary { duration_ms: Some(215_000), ..reference };
    assert!(differs(&remastered), "the length alone");
}
