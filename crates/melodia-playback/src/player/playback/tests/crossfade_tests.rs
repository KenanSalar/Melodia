//! Tests for the crossfade ramp cell, its settings cell, and the two pure
//! predicates that decide when a transition becomes a crossfade.

use super::{
    ABORT_RAMP_MS, CrossfadeSettings, CrossfadeShared, DEFAULT_CROSSFADE_MS, FadeShared,
    MAX_CROSSFADE_MS, MIN_CROSSFADE_MS, MIN_FADE_MS, clamp_crossfade_ms, crossfade_eligible,
    crossfade_ms_to_secs, is_unity_target, manual_fade_ms, ramp_gain, same_album,
    secs_to_crossfade_ms, should_crossfade,
};
use melodia_core::entities::track::TrackSummary;

// --- helpers ---------------------------------------------------------------

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-6
}

fn settings() -> CrossfadeSettings {
    CrossfadeSettings {
        enabled: true,
        duration_ms: 2_000,
        manual: false,
        skip_same_album: true,
        fade_on_pause: false,
    }
}

fn track(album: Option<&str>, artist: Option<&str>) -> TrackSummary {
    TrackSummary {
        id: 1,
        file_path: "/music/a.flac".into(),
        file_name: "a.flac".into(),
        title: "A".into(),
        artist: artist.map(Into::into),
        album: album.map(Into::into),
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
    }
}

// --- ramp_gain -------------------------------------------------------------

#[test]
fn ramp_gain_hits_its_endpoints() {
    assert!(approx(ramp_gain(1.0, 0.0, 0, 100), 1.0));
    assert!(approx(ramp_gain(1.0, 0.0, 100, 100), 0.0));
    assert!(approx(ramp_gain(0.0, 1.0, 0, 100), 0.0));
    assert!(approx(ramp_gain(0.0, 1.0, 100, 100), 1.0));
    assert!(approx(ramp_gain(1.0, 0.0, 50, 100), 0.5));
}

#[test]
fn ramp_gain_saturates_past_the_end_and_on_zero_length() {
    // Past the end holds the target rather than overshooting.
    assert!(approx(ramp_gain(1.0, 0.0, 500, 100), 0.0));
    // A zero-length ramp lands immediately, which is how `resume` snaps a deck
    // back to unity when fade-on-pause is off.
    assert!(approx(ramp_gain(0.0, 1.0, 0, 0), 1.0));
}

#[test]
fn complementary_ramps_always_sum_to_unity() {
    // This is the property that lets two overlapping decks feed the unclamped
    // mixer without ever summing past full scale. Equal-power curves would peak
    // at sqrt(2) here.
    const TOTAL: u64 = 4_096;
    for pos in (0..=TOTAL).step_by(37) {
        let out = ramp_gain(1.0, 0.0, pos, TOTAL);
        let inc = ramp_gain(0.0, 1.0, pos, TOTAL);
        assert!(approx(out + inc, 1.0), "gains at pos {pos} summed to {}", out + inc);
    }
}

#[test]
fn a_partially_faded_deck_still_sums_within_unity() {
    // Manual next during an auto-crossfade: the incoming deck is fading out from
    // a partial gain, not from unity. The sum must still stay bounded.
    const TOTAL: u64 = 1_000;
    let start = 0.4_f32;
    for pos in (0..=TOTAL).step_by(11) {
        let out = ramp_gain(start, 0.0, pos, TOTAL);
        let inc = ramp_gain(0.0, 1.0, pos, TOTAL);
        assert!(out + inc <= 1.0 + 1e-6, "sum {} exceeded unity at pos {pos}", out + inc);
    }
}

#[test]
fn unity_target_detection() {
    assert!(is_unity_target(1.0));
    assert!(!is_unity_target(0.0));
    assert!(!is_unity_target(0.5));
}

// --- clamp -----------------------------------------------------------------

#[test]
fn crossfade_duration_clamps_to_range() {
    assert_eq!(clamp_crossfade_ms(0), MIN_CROSSFADE_MS);
    assert_eq!(clamp_crossfade_ms(u32::MAX), MAX_CROSSFADE_MS);
    assert_eq!(clamp_crossfade_ms(DEFAULT_CROSSFADE_MS), DEFAULT_CROSSFADE_MS);
}

#[test]
fn seconds_and_milliseconds_round_trip() {
    assert_eq!(secs_to_crossfade_ms(2.0), 2_000);
    assert!(approx(crossfade_ms_to_secs(2_000), 2.0));
    assert!(approx(crossfade_ms_to_secs(DEFAULT_CROSSFADE_MS), 2.0));
    // The slider's own endpoints round-trip exactly, so seeding it from the
    // constants and reading a value back can't drift.
    let min_secs = crossfade_ms_to_secs(MIN_CROSSFADE_MS);
    let max_secs = crossfade_ms_to_secs(MAX_CROSSFADE_MS);
    assert_eq!(secs_to_crossfade_ms(min_secs), MIN_CROSSFADE_MS);
    assert_eq!(secs_to_crossfade_ms(max_secs), MAX_CROSSFADE_MS);
}

#[test]
fn seconds_clamp_before_the_narrowing_cast() {
    // The slider can only produce in-range values, but a hand-edited
    // `settings.json` or a future caller must not be able to truncate.
    assert_eq!(secs_to_crossfade_ms(-100.0), MIN_CROSSFADE_MS);
    assert_eq!(secs_to_crossfade_ms(1e9), MAX_CROSSFADE_MS);
    assert_eq!(secs_to_crossfade_ms(0.0), MIN_CROSSFADE_MS);
    // A sub-minimum request (an old `settings.json` written when the floor was
    // lower) clamps up rather than truncating to a window the monitor can miss.
    assert_eq!(secs_to_crossfade_ms(0.5), MIN_CROSSFADE_MS);
    assert!(approx(crossfade_ms_to_secs(0), crossfade_ms_to_secs(MIN_CROSSFADE_MS)));
    assert!(approx(crossfade_ms_to_secs(u32::MAX), 12.0));
}

// --- FadeShared ------------------------------------------------------------

#[test]
fn a_fresh_fade_cell_is_idle() {
    let fade = FadeShared::idle();
    assert!(fade.snapshot().is_none());
}

#[test]
fn arm_publishes_the_command_and_bumps_the_generation() {
    let fade = FadeShared::idle();
    let before = fade.generation();

    fade.arm(Some(0.0), 1.0, 2_000, false);
    assert_ne!(fade.generation(), before, "arming must bump the generation");

    let cmd = fade.snapshot();
    assert!(cmd.is_some(), "armed cell must yield a command");
    if let Some(cmd) = cmd {
        assert_eq!(cmd.start, Some(0.0));
        assert!(approx(cmd.target, 1.0));
        assert_eq!(cmd.ramp_ms, 2_000);
        assert!(!cmd.end_on_complete);
    }
}

#[test]
fn a_none_start_survives_the_nan_sentinel_roundtrip() {
    let fade = FadeShared::idle();
    fade.arm(None, 0.0, 500, true);
    let cmd = fade.snapshot();
    assert!(cmd.is_some(), "armed cell must yield a command");
    if let Some(cmd) = cmd {
        assert_eq!(cmd.start, None, "None must not decode as Some(NaN)");
        assert!(cmd.end_on_complete);
    }
}

#[test]
fn reset_returns_the_cell_to_idle_and_bumps() {
    let fade = FadeShared::idle();
    fade.arm(None, 0.0, 500, true);
    let armed_gen = fade.generation();

    fade.reset();
    assert!(fade.snapshot().is_none());
    assert_ne!(fade.generation(), armed_gen, "reset must bump so sources re-poll");
}

// --- CrossfadeShared -------------------------------------------------------

#[test]
fn crossfade_settings_default_to_off_with_same_album_protection() {
    let xf = CrossfadeShared::new();
    let snap = xf.snapshot();
    assert!(!snap.enabled);
    assert!(!snap.manual);
    assert!(!snap.fade_on_pause);
    assert!(snap.skip_same_album, "same-album transitions stay gapless by default");
    assert_eq!(snap.duration_ms, DEFAULT_CROSSFADE_MS);
}

#[test]
fn crossfade_shared_clamps_duration_on_write() {
    let xf = CrossfadeShared::new();
    xf.set_duration_ms(1);
    assert_eq!(xf.snapshot().duration_ms, MIN_CROSSFADE_MS);
    xf.set_duration_ms(u32::MAX);
    assert_eq!(xf.snapshot().duration_ms, MAX_CROSSFADE_MS);
}

// --- same_album ------------------------------------------------------------

#[test]
fn same_album_needs_a_present_album_tag() {
    let a = track(Some("Kid A"), Some("Radiohead"));
    let b = track(Some("Kid A"), Some("Radiohead"));
    assert!(same_album(&a, &b));

    // Two untagged tracks must NOT read as same-album, or an untagged library
    // would never crossfade (`None == None` is true).
    let untagged_a = track(None, None);
    let untagged_b = track(None, None);
    assert!(!same_album(&untagged_a, &untagged_b));
}

#[test]
fn same_album_distinguishes_albums_and_artists() {
    let a = track(Some("Kid A"), Some("Radiohead"));
    assert!(!same_album(&a, &track(Some("Amnesiac"), Some("Radiohead"))));
    // A shared album title across different artists (compilations, "Greatest
    // Hits") is not the same album.
    assert!(!same_album(&a, &track(Some("Kid A"), Some("Someone Else"))));
    assert!(!same_album(&a, &track(None, Some("Radiohead"))));
}

// --- crossfade_eligible ----------------------------------------------------

#[test]
fn eligible_in_the_ordinary_case() {
    assert!(crossfade_eligible(settings(), false, true, false));
}

#[test]
fn ineligible_when_disabled_or_zero_length_or_last_track() {
    let mut off = settings();
    off.enabled = false;
    assert!(!crossfade_eligible(off, false, true, false));

    let mut zero = settings();
    zero.duration_ms = 0;
    assert!(!crossfade_eligible(zero, false, true, false));

    assert!(!crossfade_eligible(settings(), false, false, false), "no next track");
}

#[test]
fn ineligible_when_the_sleep_timer_will_pause_at_track_end() {
    // The track has to drain to `EndOfStream` — the only boundary that gate sees.
    assert!(!crossfade_eligible(settings(), true, true, false));
}

#[test]
fn same_album_is_skipped_only_when_the_option_is_on() {
    assert!(!crossfade_eligible(settings(), false, true, true));

    let mut allow = settings();
    allow.skip_same_album = false;
    assert!(crossfade_eligible(allow, false, true, true));
}

// --- should_crossfade ------------------------------------------------------

#[test]
fn fires_inside_the_window_and_shortens_to_the_real_remaining() {
    // 500 ms poll granularity means we land somewhere inside the window; the
    // fade shortens to the actual remaining so it lands on the track end.
    let fade = should_crossfade(true, false, false, 178_200, 180_000, 2_000);
    assert_eq!(fade, Some(1_800));
}

#[test]
fn does_not_fire_before_the_window() {
    assert_eq!(should_crossfade(true, false, false, 100_000, 180_000, 2_000), None);
}

#[test]
fn does_not_fire_inside_the_final_sliver() {
    // Below MIN_FADE_MS a crossfade would have to be clamped *up* past the real
    // remaining audio, cutting the outgoing track at a non-zero gain.
    let pos = 180_000 - (MIN_FADE_MS - 1);
    assert_eq!(should_crossfade(true, false, false, pos, 180_000, 2_000), None);
    // Exactly at the threshold it still fires.
    let pos = 180_000 - MIN_FADE_MS;
    assert_eq!(should_crossfade(true, false, false, pos, 180_000, 2_000), Some(MIN_FADE_MS));
}

#[test]
fn ineligible_transitions_never_fire() {
    assert_eq!(should_crossfade(false, false, false, 179_000, 180_000, 2_000), None);
}

#[test]
fn a_staged_gapless_preload_blocks_the_crossfade() {
    // A gapless source sits on the *active* deck and shares its fade cell, so it
    // would fade out along with the track it was staged behind.
    assert_eq!(should_crossfade(true, true, false, 179_000, 180_000, 2_000), None);
}

#[test]
fn a_crossfade_already_in_flight_blocks_a_second_one() {
    // Otherwise a track shorter than the fade would chain-crossfade and the new
    // incoming deck would clear the still-fading outgoing one.
    assert_eq!(should_crossfade(true, false, true, 179_000, 180_000, 2_000), None);
}

#[test]
fn a_short_track_crossfades_once_and_only_once() {
    // 3 s track, 2 s crossfade. At t=1 s the window opens.
    assert_eq!(should_crossfade(true, false, false, 1_000, 3_000, 2_000), Some(2_000));
    // The transition advances state and arms the flag, so the next tick is
    // blocked by `is_crossfading` rather than chaining.
    assert_eq!(should_crossfade(true, false, true, 1_500, 3_000, 2_000), None);
}

#[test]
fn never_fires_at_position_zero() {
    // A track shorter than the crossfade would otherwise trigger on its first
    // tick, before a single sample had played.
    assert_eq!(should_crossfade(true, false, false, 0, 1_000, 2_000), None);
}

#[test]
fn a_stale_high_position_read_is_rejected() {
    // The monitor reads a position and acts on it a moment later, so the value
    // reaching here can be past the track's end. Too high → `remaining`
    // saturates to zero, below MIN_FADE_MS.
    assert_eq!(should_crossfade(true, false, false, 400_000, 180_000, 2_000), None);
}

#[test]
fn a_stale_low_position_read_is_rejected() {
    // Too low → `remaining` exceeds the configured cap.
    assert_eq!(should_crossfade(true, false, false, 12, 180_000, 2_000), None);
}

#[test]
fn a_crossfade_shorter_than_the_gapless_preload_lead_still_fires() {
    // The load-bearing case for splitting `crossfade_eligible` out of
    // `should_crossfade`: at the shortest crossfade (1000 ms) the preload lead
    // (1500 ms) opens first. Because the preload is gated on the
    // *timing-independent* predicate, `gapless_pending` stays false and the
    // crossfade still fires here.
    let cap = MIN_CROSSFADE_MS;
    assert_eq!(should_crossfade(true, false, false, 179_000, 180_000, cap), Some(1_000));
    // ...and a tick earlier, inside the preload lead but outside the crossfade
    // window, nothing fires.
    assert_eq!(should_crossfade(true, false, false, 178_500, 180_000, cap), None);
}

#[test]
fn every_tick_phase_catches_the_window_at_the_shortest_crossfade() {
    // The monitor samples `should_crossfade` once per poll, so the trigger
    // window `[MIN_FADE_MS, cap]` must be at least one poll wide — otherwise
    // `remaining` can step straight over it (700 ms → 200 ms) and no crossfade
    // fires at all. That is not a benign miss: `crossfade_eligible` has already
    // suppressed the gapless preload, so the transition degrades into a
    // decode-and-start hard cut — an audible gap, worse than the gapless
    // behaviour crossfade replaced. `MIN_CROSSFADE_MS` is what guarantees it.
    //
    // Walk every possible tick phase (the monitor's ticks land at an arbitrary
    // offset relative to the track end) and assert each one lands in the window
    // at least once on the way down.
    const POLL_MS: u64 = 500;
    const DURATION_MS: u64 = 180_000;
    let cap = MIN_CROSSFADE_MS;

    let ticks_per_track = DURATION_MS / POLL_MS;
    for phase in 0..POLL_MS {
        let fired = (0..ticks_per_track).map(|k| phase + k * POLL_MS).any(|position| {
            should_crossfade(true, false, false, position, DURATION_MS, cap).is_some()
        });
        assert!(
            fired,
            "tick phase {phase} ms never landed inside the crossfade window at the \
             shortest duration ({cap} ms) — the window is narrower than one {POLL_MS} ms poll"
        );
    }
}

#[test]
fn a_zero_duration_track_never_fires() {
    assert_eq!(should_crossfade(true, false, false, 1_000, 0, 2_000), None);
}

#[test]
fn abort_ramp_is_short_enough_to_be_inaudible_but_not_a_step() {
    const { assert!(ABORT_RAMP_MS > 0 && ABORT_RAMP_MS < MIN_FADE_MS) }
}

// --- manual_fade_ms --------------------------------------------------------

fn manual_settings() -> CrossfadeSettings {
    CrossfadeSettings {
        manual: true,
        ..settings()
    }
}

#[test]
fn a_manual_track_change_fades_when_something_is_playing() {
    assert_eq!(manual_fade_ms(manual_settings(), false, true, false), 2_000);
}

#[test]
fn a_manual_track_change_hard_cuts_when_the_option_is_off() {
    // Crossfade on, but only for automatic transitions.
    assert_eq!(manual_fade_ms(settings(), false, true, false), 0);

    let mut off = manual_settings();
    off.enabled = false;
    assert_eq!(manual_fade_ms(off, false, true, false), 0);
}

#[test]
fn a_manual_track_change_hard_cuts_from_silence_or_into_a_restored_position() {
    // Nothing playing to fade out of.
    assert_eq!(manual_fade_ms(manual_settings(), false, false, false), 0);
    // Resuming into the middle of a track should start clean, not fade in.
    assert_eq!(manual_fade_ms(manual_settings(), true, true, false), 0);
}

#[test]
fn a_manual_track_change_hard_cuts_while_a_gapless_source_is_staged() {
    // The staged source shares the active deck's fade cell. A self-ending
    // fade-out armed there would be inherited by it the moment the current
    // source ends, and it would play at full volume while fading out.
    assert_eq!(manual_fade_ms(manual_settings(), false, true, true), 0);
}
