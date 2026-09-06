//! What the OS panel sends in, and what it is told back.
//!
//! Both halves fail quietly. A mistranslated key does something the user did not press, and a
//! dedupe that reads two different songs as one leaves the panel showing the previous one for as
//! long as the station keeps playing.

use souvlaki::SeekDirection;

use melodia_engine::player::engine::now_playing::SourceId;

use super::*;

/// The event as a token. `PlayerEvent` carries no `PartialEq`, and its `Debug` on a failure names
/// the variant without saying which one was wanted.
fn tag(event: MediaControlEvent) -> Option<&'static str> {
    match translate_event(event)? {
        PlayerEvent::Play => Some("play"),
        PlayerEvent::Pause => Some("pause"),
        PlayerEvent::PlayPause => Some("play-pause"),
        PlayerEvent::Next => Some("next"),
        PlayerEvent::Previous => Some("previous"),
        PlayerEvent::Stop => Some("stop"),
        PlayerEvent::SeekTo(_) => Some("seek-to"),
        PlayerEvent::SetVolume(_) => Some("set-volume"),
    }
}

fn scaled_volume(vol: f64) -> Option<u32> {
    match translate_event(MediaControlEvent::SetVolume(vol)) {
        Some(PlayerEvent::SetVolume(scaled)) => Some(scaled),
        _ => None,
    }
}

fn seek_target(position: Duration) -> Option<u64> {
    match translate_event(MediaControlEvent::SetPosition(MediaPosition(position))) {
        Some(PlayerEvent::SeekTo(ms)) => Some(ms),
        _ => None,
    }
}

// --- What the panel sends in -----------------------------------------------

/// `Toggle` is the arm that is not a rename of itself: a headphone button sends one event for both
/// directions, so answering it with `Play` gives a key that starts playback and can never stop it.
#[test]
fn every_transport_key_reaches_its_own_event() {
    assert_eq!(tag(MediaControlEvent::Play), Some("play"));
    assert_eq!(tag(MediaControlEvent::Pause), Some("pause"));
    assert_eq!(tag(MediaControlEvent::Toggle), Some("play-pause"));
    assert_eq!(tag(MediaControlEvent::Next), Some("next"));
    assert_eq!(tag(MediaControlEvent::Previous), Some("previous"));
    assert_eq!(tag(MediaControlEvent::Stop), Some("stop"));
}

/// souvlaki states that a host may send a value outside `[0, 1]`, so the clamp is the whole of
/// what stands between an MPRIS client and a volume of 200. Half a percent is the other half:
/// rounding is what makes the quietest step reachable, where a truncating cast needs a full
/// percent before anything moves.
#[test]
fn the_volume_scale_clamps_its_input_and_rounds_its_output() {
    assert_eq!(scaled_volume(-0.5), Some(0), "below the floor");
    assert_eq!(scaled_volume(0.0), Some(0));
    assert_eq!(scaled_volume(0.004), Some(0), "under half a percent");
    assert_eq!(scaled_volume(0.005), Some(1), "and on it");
    assert_eq!(scaled_volume(1.0), Some(100));
    assert_eq!(scaled_volume(2.0), Some(100), "above the ceiling");
}

/// A `Duration` states its milliseconds as a `u128`, so a position the player could never hold is
/// representable at the boundary and arrives as one. The ordinary value beside it is what says the
/// saturation is not the answer to everything.
#[test]
fn a_position_too_wide_for_the_field_saturates() {
    assert_eq!(seek_target(Duration::from_millis(90_500)), Some(90_500));
    assert_eq!(seek_target(Duration::from_secs(u64::MAX)), Some(u64::MAX));
}

/// Every event this layer deliberately drops. Each is something a user can press, so an arm that
/// starts answering is a media key doing what nobody asked — `Quit` most of all, which would close
/// the window from a headphone button.
#[test]
fn the_events_this_layer_does_not_answer_reach_nothing() {
    assert_eq!(tag(MediaControlEvent::Seek(SeekDirection::Forward)), None);
    let by = MediaControlEvent::SeekBy(SeekDirection::Backward, Duration::from_secs(10));
    assert_eq!(tag(by), None);
    assert_eq!(tag(MediaControlEvent::Raise), None);
    assert_eq!(tag(MediaControlEvent::Quit), None);
    assert_eq!(tag(MediaControlEvent::OpenUri("https://example.test/x.mp3".to_owned())), None);
}

// --- What the panel is told back -------------------------------------------

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

    let retitled = SourceSummary {
        title: "Sunset Drive (Remastered)",
        ..reference
    };
    assert!(differs(&retitled), "a track re-tagged in place");

    let announced = SourceSummary {
        secondary: Some("Night Bus"),
        ..reference
    };
    assert!(differs(&announced), "a station announcing its next song");

    let recompiled = SourceSummary {
        album: Some("Singles"),
        ..reference
    };
    assert!(differs(&recompiled), "the album alone");

    let recovered = SourceSummary {
        artwork_path: Some("artwork/cd/cdef01.jpg"),
        ..reference
    };
    assert!(differs(&recovered), "the cover alone");

    let remastered = SourceSummary {
        duration_ms: Some(215_000),
        ..reference
    };
    assert!(differs(&remastered), "the length alone");
}
