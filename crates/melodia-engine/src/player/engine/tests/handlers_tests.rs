//! Tests for the playback monitor's per-tick decision.
//!
//! `evaluate_playing_tick` is the whole of the `Playing` branch: it decides
//! whether this tick starts a crossfade, stages a gapless preload, or does
//! neither. The two are mutually exclusive for a given transition — a crossfade
//! runs the next track on the *other* deck, while a gapless source would sit on
//! this one, behind the outgoing track, and inherit its fade cell.

use std::sync::Arc;

use super::*;
use crate::player::engine::state::PlayerViewModelLight;
use melodia_core::entities::track::TrackSummary;
use melodia_playback::player::playback::crossfade::CrossfadeSettings;

fn track(id: i64, album: Option<&str>) -> Arc<TrackSummary> {
    Arc::new(TrackSummary {
        id,
        file_path: format!("/music/{id}.mp3"),
        file_name: format!("{id}.mp3"),
        title: format!("Track {id}"),
        artist: None,
        album: album.map(str::to_owned),
        duration_ms: 180_000,
        artwork_path: None,
        track_number: None,
        disc_number: None,
        last_position: 0,
        is_favorite: false,
        rating: 0,
        replaygain_track_gain: None,
        replaygain_track_peak: None,
        replaygain_album_gain: None,
        replaygain_album_peak: None,
    })
}

/// Two queued tracks on different albums, playing the first, gapless on.
fn playing_state() -> PlayerState {
    let mut state = PlayerState::default();
    state.queue.add_tracks(vec![track(1, Some("A")), track(2, Some("B"))]);
    state.queue.current_index = Some(0);
    state.status = PlaybackStatus::Playing;
    state.source = Some(PlaybackSource::Track(track(1, Some("A"))));
    state.duration_ms = 180_000;
    state.gapless_enabled = true;
    state
}

fn crossfade_off() -> CrossfadeSettings {
    CrossfadeSettings {
        enabled: false,
        duration_ms: 2_000,
        manual: false,
        skip_same_album: true,
        fade_on_pause: false,
    }
}

fn crossfade_on(duration_ms: u32) -> CrossfadeSettings {
    CrossfadeSettings {
        enabled: true,
        duration_ms,
        ..crossfade_off()
    }
}

fn backend(position_ms: u64, xf: CrossfadeSettings) -> BackendSnapshot {
    BackendSnapshot {
        position_ms,
        already_preloaded: false,
        crossfading: false,
        xf,
    }
}

/// The position at which `remaining_ms` of the 180 s fixture track are left.
fn at_remaining(remaining_ms: u64) -> u64 {
    180_000 - remaining_ms
}

/// Run one tick, asserting it isn't void. `None` (playback moved off `Playing`)
/// is a separate case with its own test.
fn tick(state: &mut PlayerState, backend: BackendSnapshot) -> Option<PlayingTick> {
    let out = evaluate_playing_tick(state, backend);
    assert!(out.is_some(), "a tick on a playing state must not be void");
    out
}

/// The path of the track this tick staged behind the current one, if any.
fn staged(t: Option<&PlayingTick>) -> Option<&str> {
    t.and_then(|t| t.late_preload.as_ref()).map(|(path, _rg)| path.as_str())
}

/// The fade length this tick decided on, if it decided to crossfade.
fn fade_ms(t: Option<&PlayingTick>) -> Option<u64> {
    t.and_then(|t| t.crossfade.as_ref()).map(|d| d.fade_ms)
}

// --- the tick itself -------------------------------------------------------

#[test]
fn a_tick_off_playing_is_void() {
    let mut state = playing_state();
    state.status = PlaybackStatus::Paused;

    assert!(evaluate_playing_tick(&mut state, backend(1_000, crossfade_off())).is_none());
}

#[test]
fn a_normal_tick_publishes_the_position_and_does_nothing_else() {
    let mut state = playing_state();

    let t = tick(&mut state, backend(30_000, crossfade_off()));

    assert_eq!(t.as_ref().map(|t| t.tick.position_ms), Some(30_000));
    assert_eq!(t.as_ref().map(|t| t.tick.duration_ms), Some(180_000));
    assert_eq!(state.position_ms, 30_000);
    assert_eq!(fade_ms(t.as_ref()), None);
    assert_eq!(staged(t.as_ref()), None, "far from the end, nothing to stage");
}

// --- gapless preload -------------------------------------------------------

#[test]
fn the_gapless_preload_stages_the_next_track_near_the_end() {
    let mut state = playing_state();

    let t = tick(&mut state, backend(at_remaining(1_000), crossfade_off()));

    assert_eq!(
        staged(t.as_ref()),
        Some("/music/2.mp3"),
        "within PRELOAD_LEAD_MS of the end, the next track must be staged"
    );
    assert_eq!(fade_ms(t.as_ref()), None, "crossfade is off");
}

#[test]
fn pause_at_track_end_suppresses_the_gapless_preload() {
    let mut state = playing_state();
    // The sleep timer's "End of current track": the track must drain to
    // `EndOfStream`, not `GaplessTransition` (which would already be playing the
    // next one) — that's the only boundary the pause gate can catch.
    state.pause_after_current_track = true;

    let t = tick(&mut state, backend(at_remaining(1_000), crossfade_off()));

    assert_eq!(staged(t.as_ref()), None);
    assert_eq!(fade_ms(t.as_ref()), None);
}

#[test]
fn a_zero_position_read_never_stages_a_preload() {
    let mut state = playing_state();

    // A deck reads 0 until it has been pulled for its new source, which would
    // otherwise read as a whole track's worth of remaining time.
    let t = tick(&mut state, backend(0, crossfade_off()));

    assert_eq!(staged(t.as_ref()), None);
}

// --- crossfade vs gapless: the mutual exclusion -----------------------------

#[test]
fn an_eligible_crossfade_suppresses_the_gapless_preload() {
    let mut state = playing_state();

    // Inside the preload window, but this transition belongs to the crossfade
    // path — so the preload must not fire, whatever the timing says.
    let t = tick(&mut state, backend(at_remaining(1_400), crossfade_on(2_000)));

    assert_eq!(
        staged(t.as_ref()),
        None,
        "a gapless source would sit on the outgoing deck and inherit its fade cell"
    );
}

#[test]
fn a_crossfade_shorter_than_the_preload_lead_still_fires() {
    let mut state = playing_state();
    let xf = crossfade_on(crossfade::MIN_CROSSFADE_MS);

    // The regression this guards: the preload window (1.5 s) is *wider* than a
    // 1 s crossfade. Gating the preload on the timing-dependent predicate would
    // let it fire first here, set `gapless_pending`, and then permanently block
    // the crossfade via its own `!gapless_pending` gate.
    let early = tick(&mut state, backend(at_remaining(1_400), xf));
    assert_eq!(staged(early.as_ref()), None, "the preload must stay suppressed");
    assert_eq!(fade_ms(early.as_ref()), None, "too early for a 1 s fade");

    let t = tick(&mut state, backend(at_remaining(900), xf));

    // The ramp lands on the real track end, not on the configured duration.
    assert_eq!(fade_ms(t.as_ref()), Some(900));
    assert_eq!(
        t.as_ref().and_then(|t| t.crossfade.as_ref()?.track_id),
        Some(1),
        "the decision carries the OUTGOING track's id"
    );
    assert_eq!(
        t.as_ref().and_then(|t| t.crossfade.as_ref()).map(|d| d.position_ms),
        Some(at_remaining(900))
    );
}

#[test]
fn a_same_album_transition_stays_gapless() {
    let mut state = playing_state();
    // Both tracks on album "A": the default `skip_same_album` keeps
    // continuous-mix albums seamless.
    state.queue.clear();
    state.queue.add_tracks(vec![track(1, Some("A")), track(2, Some("A"))]);
    state.queue.current_index = Some(0);

    let t = tick(&mut state, backend(at_remaining(1_000), crossfade_on(2_000)));

    assert_eq!(fade_ms(t.as_ref()), None, "same album, so no crossfade");
    assert_eq!(
        staged(t.as_ref()),
        Some("/music/2.mp3"),
        "and the gapless preload takes over instead"
    );
}

#[test]
fn a_crossfade_in_flight_blocks_both_paths() {
    let mut state = playing_state();
    let mut snap = backend(at_remaining(1_000), crossfade_on(2_000));
    snap.crossfading = true;

    let t = tick(&mut state, snap);

    assert_eq!(fade_ms(t.as_ref()), None, "one overlap at a time");
    assert_eq!(staged(t.as_ref()), None);
}

#[test]
fn a_staged_gapless_source_blocks_the_crossfade() {
    let mut state = playing_state();
    let mut snap = backend(at_remaining(1_500), crossfade_on(2_000));
    snap.already_preloaded = true;

    let t = tick(&mut state, snap);

    // The staged source shares the deck's fade cell, so it would inherit the
    // outgoing ramp. `begin_crossfade` debug-asserts this can't happen.
    assert_eq!(fade_ms(t.as_ref()), None);
    assert_eq!(staged(t.as_ref()), None, "already staged");
}

#[test]
fn the_last_track_neither_crossfades_nor_preloads() {
    let mut state = playing_state();
    state.queue.clear();
    state.queue.add_tracks(vec![track(1, Some("A"))]);
    state.queue.current_index = Some(0);

    let t = tick(&mut state, backend(at_remaining(1_000), crossfade_on(2_000)));

    assert_eq!(fade_ms(t.as_ref()), None);
    assert_eq!(staged(t.as_ref()), None);
}

// --- Live sources ----------------------------------------------------------

/// A station playing over the same two-track queue, which stays untouched underneath it. Both
/// decisions below are per-*track* transitions, so a queue with somewhere to go is exactly what
/// makes the assertions mean something.
fn playing_a_station() -> PlayerState {
    let mut state = playing_state();
    let (generation, _actions) = state.build_station_connecting_actions(
        crate::player::engine::fixtures::test_station("Example FM"),
    );
    let _started = state.build_station_connected_actions(generation);
    state
}

/// A crossfade ramps between two tracks and a gapless preload stages the next one. A live source
/// has neither a next nor an end, so both gates have to be answered before they are asked — the
/// position would otherwise sit deep inside the trigger window of a duration that means nothing.
#[test]
fn a_live_source_neither_crossfades_nor_preloads() {
    let mut state = playing_a_station();
    let mut xf = crossfade_on(4_000);
    xf.manual = true;

    let out = tick(&mut state, backend(at_remaining(1_000), xf));

    assert!(
        out.as_ref().is_some_and(|t| t.crossfade.is_none()),
        "a station has nothing to fade into"
    );
    assert_eq!(staged(out.as_ref()), None, "a station has nothing to stage behind it");
}

#[test]
fn a_live_source_still_publishes_its_elapsed_time() {
    let mut state = playing_a_station();

    let out = tick(&mut state, backend(12_345, crossfade_off()));

    assert_eq!(out.as_ref().map(|t| t.tick.position_ms), Some(12_345));
    assert_eq!(state.position_ms, 12_345);
    // Elapsed listening time, not a fraction of anything: a live stream has no length.
    assert_eq!(out.as_ref().map(|t| t.tick.duration_ms), Some(0));
}

// --- The live-stream reconcile ---------------------------------------------

/// A handle seated on a state, plus a receiver on the view-model sink, which is the only way to
/// see whether a tick published anything.
fn seated(
    state: PlayerState,
) -> (PlayerStateHandle, PlayerSinks, watch::Receiver<Option<PlayerViewModelLight>>) {
    let handle = PlayerStateHandle::default();
    *lock_state(&handle) = state;
    let (view_model, published) = watch::channel(None);
    let (queue, _) = watch::channel(None);
    let sinks = PlayerSinks {
        view_model,
        queue,
        media_controls: None,
    };
    (handle, sinks, published)
}

/// The whole reason the change checks exist. A station that is playing happily changes neither
/// its buffering flag nor its title, and the monitor runs twice a second: republishing the view
/// model on every one of those ticks rebuilds it for nothing.
#[test]
fn a_station_with_nothing_to_report_publishes_nothing() {
    let stream = StreamShared::new();
    let (handle, sinks, published) = seated(playing_a_station());
    let mut last_generation = stream.title_generation();

    reconcile_live_stream(&stream, &handle, &sinks, &mut last_generation);

    assert_eq!(published.has_changed().ok(), Some(false));
}

/// A new title is the one thing a station changes on its own that the user can see, and it
/// arrives on the feed thread with no way to reach the state lock from there. The generation is
/// how this tick learns of it.
#[test]
fn a_moved_title_generation_publishes_the_station_again() {
    let stream = StreamShared::new();
    let (handle, sinks, published) = seated(playing_a_station());
    // A generation the stream has never held, standing in for a title that landed since the last
    // tick read one.
    let mut last_generation = stream.title_generation().wrapping_sub(1);

    reconcile_live_stream(&stream, &handle, &sinks, &mut last_generation);

    assert_eq!(published.has_changed().ok(), Some(true));
    assert_eq!(last_generation, stream.title_generation(), "the tick has to record what it saw");
}

/// The buffering flag is the other half, and it moves in both directions: a station that has
/// recovered has to have the indicator taken off it, not just put on.
#[test]
fn a_station_that_stopped_buffering_has_the_flag_cleared() {
    let stream = StreamShared::new();
    let mut state = playing_a_station();
    if let Some(station) = state.station_mut() {
        Arc::make_mut(station).buffering = true;
    }
    let (handle, sinks, published) = seated(state);
    let mut last_generation = stream.title_generation();

    reconcile_live_stream(&stream, &handle, &sinks, &mut last_generation);

    assert_eq!(published.has_changed().ok(), Some(true));
    assert_eq!(
        lock_state(&handle).station().map(|s| s.buffering),
        Some(false),
        "the stream is not buffering, so neither is the station the UI paints",
    );
}

/// The generation is only how this tick learns that a title landed; the text is what the user
/// reads. Its sibling above pins the publish and the generation it recorded, so dropping the
/// assignment leaves every case here green while the station goes on announcing whatever it was
/// called when it started.
#[test]
fn a_new_title_reaches_the_station_the_ui_paints() {
    let stream = StreamShared::new();
    let (handle, sinks, _published) = seated(playing_a_station());
    let mut last_generation = stream.title_generation();
    stream.set_title(Some("Night Bus".to_owned()));

    reconcile_live_stream(&stream, &handle, &sinks, &mut last_generation);

    assert_eq!(
        lock_state(&handle).station().and_then(|s| s.live_title.clone()).as_deref(),
        Some("Night Bus"),
    );
}

/// A cleared title is a title change, and the feed thread reports one the same way it reports a
/// new song. The narrowing this reads as an invitation to — asking `stream.title()` for a `Some`
/// and assigning only that — leaves the last song on screen for the rest of the session, which
/// looks like a station that never changes track rather than one that stopped saying.
#[test]
fn a_station_that_stops_announcing_has_its_title_cleared() {
    let stream = StreamShared::new();
    let mut state = playing_a_station();
    if let Some(station) = state.station_mut() {
        Arc::make_mut(station).live_title = Some("Night Bus".to_owned());
    }
    let (handle, sinks, _published) = seated(state);
    let mut last_generation = stream.title_generation();
    stream.set_title(None);

    reconcile_live_stream(&stream, &handle, &sinks, &mut last_generation);

    assert_eq!(lock_state(&handle).station().and_then(|s| s.live_title.clone()), None);
}
