//! What the handle announces on the bus, and what it keeps to itself.
//!
//! Both halves fail without an error. A signal that never goes out leaves a panel stale, and one
//! that goes out on every sync costs every listener a wake-up, KDE Connect a packet to each paired
//! phone among them.

use melodia_engine::player::engine::fixtures::{test_track, test_view_model};
use melodia_engine::player::engine::types::RepeatMode;

use super::*;

/// The signal as a token. `Emit` carries no `PartialEq`, being a private message nothing else
/// compares.
fn tag(emit: Emit) -> &'static str {
    match emit {
        Emit::Metadata => "metadata",
        Emit::PlaybackStatus => "playback-status",
        Emit::Volume => "volume",
        Emit::LoopStatus => "loop-status",
        Emit::Shuffle => "shuffle",
        Emit::Seeked(_) => "seeked",
    }
}

fn handle() -> (MediaControlsHandle, std_mpsc::Receiver<Emit>) {
    let (emits, announced) = std_mpsc::channel();
    let handle = MediaControlsHandle {
        published: Arc::new(Mutex::new(Published::default())),
        emits: Some(emits),
    };
    (handle, announced)
}

fn playing() -> PlayerViewModelLight {
    test_view_model(
        Some(test_track("Sunset Drive", Some("The Coastliners"), Some("Long Way Round"))),
        None,
        214_000,
    )
}

/// The signals a sync of `moved` sends over a handle that has already announced `playing()`.
fn announced_after(moved: &PlayerViewModelLight, status: PlaybackStatus) -> Vec<&'static str> {
    let (handle, announced) = handle();
    handle.sync(&playing(), PlaybackStatus::Playing);
    announced.try_iter().for_each(drop);

    handle.sync(moved, status);

    announced.try_iter().map(tag).collect()
}

/// The defect this backend replaced: every position handed to souvlaki re-announced
/// `PlaybackStatus`. `Position` is read on request, and a playing track costs the bus nothing
/// between the changes a listener would notice.
#[test]
fn a_position_that_only_advanced_announces_nothing() {
    let advanced = PlayerViewModelLight { position_ms: 61_000, ..playing() };

    let signals = announced_after(&advanced, PlaybackStatus::Playing);

    assert!(signals.is_empty(), "announced {signals:?}");
}

#[test]
fn a_polled_position_announces_nothing() {
    let (handle, announced) = handle();

    handle.update_position(61_500);

    assert!(announced.try_recv().is_err());
}

/// Each part a panel draws, moved alone, goes out under its own signal and no other: a panel only
/// re-reads what a signal names, so the wrong name leaves the moved part stale on screen.
#[test]
fn each_part_a_panel_draws_is_announced_under_its_own_signal() {
    let reference = playing();

    assert_eq!(announced_after(&reference, PlaybackStatus::Paused), ["playback-status"], "a pause");
    assert_eq!(
        announced_after(
            &PlayerViewModelLight { volume: 40, ..reference.clone() },
            PlaybackStatus::Playing
        ),
        ["volume"],
        "the volume"
    );
    assert_eq!(
        announced_after(
            &PlayerViewModelLight { repeat_mode: RepeatMode::One, ..reference.clone() },
            PlaybackStatus::Playing
        ),
        ["loop-status"],
        "repeat"
    );
    assert_eq!(
        announced_after(
            &PlayerViewModelLight { shuffle_enabled: true, ..reference.clone() },
            PlaybackStatus::Playing
        ),
        ["shuffle"],
        "shuffle"
    );
    assert_eq!(
        announced_after(
            &PlayerViewModelLight {
                current_track: Some(test_track("Night Bus", Some("The Coastliners"), None)),
                ..reference
            },
            PlaybackStatus::Playing
        ),
        ["metadata"],
        "the next track"
    );
}

/// `Seeked` carries the position itself, because a client extrapolating from it takes the argument
/// rather than reading `Position` back.
#[test]
fn a_seek_is_announced_with_where_it_landed() {
    let (handle, announced) = handle();

    handle.seeked(90_500);

    assert!(matches!(announced.try_recv(), Ok(Emit::Seeked(90_500))));
}
