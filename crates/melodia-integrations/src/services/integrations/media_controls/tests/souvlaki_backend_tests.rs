//! What the OS panel sends in.
//!
//! A mistranslated key does something the user did not press.

use souvlaki::SeekDirection;

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

fn seek_target(position: Duration) -> Option<u64> {
    match translate_event(MediaControlEvent::SetPosition(MediaPosition(position))) {
        Some(PlayerEvent::SeekTo(ms)) => Some(ms),
        _ => None,
    }
}

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
    assert_eq!(tag(MediaControlEvent::SetVolume(0.5)), Some("set-volume"));
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
