//! What the MPRIS interfaces answer a client, and what a client's writes reach the player as.
//!
//! Every getter here is read by a desktop panel at a moment of its own choosing, so a value that is
//! merely stale fails quietly: the panel draws it and nothing errors.

use melodia_engine::player::engine::event_sink::MediaControlsSync;

use super::*;
use crate::services::integrations::media_controls::MediaControlsHandle;
use crate::services::integrations::media_controls::published::PublishedMetadata;

fn player() -> (Player, mpsc::Receiver<PlayerEvent>) {
    let (events, received) = mpsc::channel(4);
    (Player { published: Arc::new(Mutex::new(Published::default())), events }, received)
}

fn player_with(published: Published) -> (Player, mpsc::Receiver<PlayerEvent>) {
    let (events, received) = mpsc::channel(4);
    (Player { published: Arc::new(Mutex::new(published)), events }, received)
}

// --- Position ---------------------------------------------------------------

/// The defect this backend replaced: `Position` answered with the value from the last status
/// change, so a client polling it saw one number for a whole track. The handle is the one the
/// monitor feeds every poll, and nothing between it and the read may lag behind.
#[test]
fn a_polled_position_is_what_the_next_read_returns() {
    let (player, _received) =
        player_with(Published { status: Some(PlaybackStatus::Playing), ..Published::default() });
    let handle = MediaControlsHandle { published: Arc::clone(&player.published), emits: None };

    handle.update_position(61_500);

    assert_eq!(player.position(), 61_500_000);
}

#[test]
fn a_paused_track_reads_where_it_stopped() {
    let (player, _received) = player_with(Published {
        status: Some(PlaybackStatus::Paused),
        position_ms: 61_500,
        ..Published::default()
    });

    assert_eq!(player.position(), 61_500_000);
}

/// What a client reads for a player at `status` that still holds a position from earlier playback.
fn position_read_at(status: Option<PlaybackStatus>) -> i64 {
    let (player, _received) =
        player_with(Published { status, position_ms: 61_500, ..Published::default() });
    player.position()
}

/// A recorded position outlives the playback it came from, and a stopped player that still
/// reported it would draw a bar part way through a track that is not playing.
#[test]
fn nothing_playing_reads_the_start() {
    assert_eq!(position_read_at(Some(PlaybackStatus::Stopped)), 0, "stopped");
    assert_eq!(position_read_at(Some(PlaybackStatus::Loading)), 0, "a station still connecting");
    assert_eq!(position_read_at(None), 0, "before the first sync");
}

/// MPRIS counts microseconds in a signed field and the player milliseconds in an unsigned one, so
/// the top of the millisecond range does not fit. The rows are the widest value that does, the
/// step past it and the top: a wrapping multiply hands a client a negative position.
#[test]
fn a_millisecond_position_too_wide_for_microseconds_saturates() {
    assert_eq!(micros(0), 0);
    assert_eq!(micros(61_500), 61_500_000);
    assert_eq!(micros(9_223_372_036_854_775), 9_223_372_036_854_775_000, "the widest that fits");
    assert_eq!(micros(9_223_372_036_854_776), i64::MAX, "one past it");
    assert_eq!(micros(u64::MAX), i64::MAX, "the top of the range");
}

// --- SetPosition ------------------------------------------------------------

/// The spec's two refusals, at their edges. The fourth row is the one integer division makes: a
/// position inside the final millisecond floors onto the length and still lands.
#[test]
fn a_position_inside_the_track_lands_and_either_side_of_it_is_refused() {
    const LENGTH_MS: u64 = 214_000;

    assert_eq!(seek_target(-1, Some(LENGTH_MS)), None, "one microsecond before the start");
    assert_eq!(seek_target(0, Some(LENGTH_MS)), Some(0), "the start");
    assert_eq!(seek_target(214_000_000, Some(LENGTH_MS)), Some(214_000), "the end");
    assert_eq!(seek_target(214_000_999, Some(LENGTH_MS)), Some(214_000), "the last millisecond");
    assert_eq!(seek_target(214_001_000, Some(LENGTH_MS)), None, "a millisecond past the end");
}

/// A live source publishes no length, so the only position left to refuse is a negative one.
#[test]
fn a_source_with_no_length_refuses_only_a_negative_position() {
    assert_eq!(seek_target(i64::MAX, None), Some(9_223_372_036_854_775));
    assert_eq!(seek_target(-1, None), None);
}

#[test]
fn a_position_on_the_published_track_seeks_the_player() {
    let (player, mut received) = player();

    player.set_position(TRACK_ID, 90_500_000);

    assert!(matches!(received.try_recv(), Ok(PlayerEvent::SeekTo(90_500))));
}

/// The spec calls a `SetPosition` naming any other track stale, and this player never publishes
/// another id, so whatever sent one is not looking at this player's panel.
#[test]
fn a_position_naming_another_track_is_ignored() {
    let (player, mut received) = player();
    let no_track =
        ObjectPath::from_static_str_unchecked("/org/mpris/MediaPlayer2/TrackList/NoTrack");

    player.set_position(no_track, 90_500_000);

    assert!(received.try_recv().is_err());
}

/// The length is the published track's, and a refusal computed against no length at all would let
/// this position through.
#[test]
fn a_position_past_the_published_length_is_ignored() {
    let (player, mut received) = player_with(Published {
        metadata: Some(PublishedMetadata {
            title: "Sunset Drive".to_owned(),
            secondary: None,
            album: None,
            artwork_path: None,
            duration_ms: Some(214_000),
        }),
        ..Published::default()
    });

    player.set_position(TRACK_ID, 214_001_000);

    assert!(received.try_recv().is_err());
}

// --- PlaybackStatus, LoopStatus ---------------------------------------------

/// `Loading` is a station connecting, and the spec has no word for it: a panel told `Stopped`
/// shows a play button, where one told `Playing` would advance a bar over silence.
#[test]
fn a_status_goes_out_under_the_spec_name_a_panel_draws() {
    let named =
        |status| player_with(Published { status, ..Published::default() }).0.playback_status();

    assert_eq!(named(Some(PlaybackStatus::Playing)), "Playing");
    assert_eq!(named(Some(PlaybackStatus::Paused)), "Paused");
    assert_eq!(named(Some(PlaybackStatus::Stopped)), "Stopped");
    assert_eq!(named(Some(PlaybackStatus::Loading)), "Stopped", "a station connecting");
    assert_eq!(named(None), "Stopped", "before the first sync");
}

/// The names are the spec's contract rather than ours. A swap between two of them still
/// round-trips, so both directions are written out.
#[test]
fn each_repeat_mode_goes_out_under_the_spec_name() {
    assert_eq!(as_loop_status(RepeatMode::Off), "None");
    assert_eq!(as_loop_status(RepeatMode::All), "Playlist");
    assert_eq!(as_loop_status(RepeatMode::One), "Track");
}

#[test]
fn each_spec_name_comes_in_as_its_repeat_mode() {
    assert_eq!(parse_loop_status("None"), Some(RepeatMode::Off));
    assert_eq!(parse_loop_status("Playlist"), Some(RepeatMode::All));
    assert_eq!(parse_loop_status("Track"), Some(RepeatMode::One));
}

/// The names a client could plausibly send that the spec does not define.
#[test]
fn a_loop_status_outside_the_spec_names_no_mode() {
    assert_eq!(parse_loop_status("Off"), None, "Melodia's own name for it");
    assert_eq!(parse_loop_status("playlist"), None, "the spec's name in the wrong case");
    assert_eq!(parse_loop_status(""), None, "nothing");
}

// --- A client's writes ------------------------------------------------------
//
// zbus answers a set with the property straight away, before the player has applied it, so each
// setter records what it was asked for: one that recorded nothing would answer with the old value
// and jump the client's control back. Hence the pairs below, one for the answer and one for what
// the player is sent.

#[test]
fn a_repeat_mode_set_by_a_client_is_answered_back_at_once() -> zbus::fdo::Result<()> {
    let (player, _received) = player();

    player.set_loop_status("Track")?;

    assert_eq!(player.loop_status(), "Track");
    Ok(())
}

#[test]
fn a_repeat_mode_set_by_a_client_reaches_the_player() -> zbus::fdo::Result<()> {
    let (player, mut received) = player();

    player.set_loop_status("Playlist")?;

    assert!(matches!(received.try_recv(), Ok(PlayerEvent::SetRepeat(RepeatMode::All))));
    Ok(())
}

#[test]
fn a_shuffle_set_by_a_client_is_answered_back_at_once() {
    let (player, _received) = player();

    player.set_shuffle(true);

    assert!(player.shuffle());
}

#[test]
fn a_shuffle_set_by_a_client_reaches_the_player() {
    let (player, mut received) = player();

    player.set_shuffle(true);

    assert!(matches!(received.try_recv(), Ok(PlayerEvent::SetShuffle(true))));
}

/// Set over a mute, because the player unmutes on any set and an answer still reading the mute
/// would be silence.
#[test]
fn a_volume_set_over_a_mute_is_answered_back_unmuted() {
    let (player, _received) = player_with(Published { is_muted: true, ..Published::default() });

    player.set_volume(0.4);

    assert!((player.volume() - 0.4).abs() < 1e-9, "within 1e-9 of the level asked for");
}

#[test]
fn a_volume_set_by_a_client_reaches_the_player_as_a_step() {
    let (player, mut received) = player();

    player.set_volume(0.4);

    assert!(matches!(received.try_recv(), Ok(PlayerEvent::SetVolume(40))));
}

/// Refused outright rather than dropped, and nothing recorded: zbus sends its echo only after a
/// set that succeeded, so a bad value left in the snapshot would stand until the next sync.
#[test]
fn a_loop_status_outside_the_spec_is_refused_and_changes_nothing() {
    let (player, mut received) = player();

    let refused = player.set_loop_status("Shuffle");

    assert!(matches!(refused, Err(zbus::fdo::Error::InvalidArgs(_))), "the refusal");
    assert_eq!(player.loop_status(), "None", "the recorded mode");
    assert!(received.try_recv().is_err(), "the player");
}
