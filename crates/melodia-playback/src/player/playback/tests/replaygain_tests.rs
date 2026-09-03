use super::*;
use crate::player::playback::tests::helpers::assert_approx as approx;

// `db_to_linear` itself is pinned in `dsp_tests` — these exercise what
// `ReplayGain` builds on top of it.

#[test]
fn untagged_track_is_unity() {
    // No gain data in any mode → unity, regardless of preamp/prevent-clipping.
    let baked = TrackReplayGain::default();
    approx(compute_linear_gain(baked, RgMode::Album, 0.0, true), 1.0);
    approx(compute_linear_gain(baked, RgMode::Track, 6.0, false), 1.0);
    approx(compute_linear_gain(baked, RgMode::Album, -6.0, true), 1.0);
}

#[test]
fn album_mode_prefers_album_gain() {
    let baked = TrackReplayGain {
        track_gain: Some(-3.0),
        album_gain: Some(-6.020_6),
        ..Default::default()
    };
    // Album mode uses album gain (-6.02 dB ≈ ×0.5), not track gain.
    approx(compute_linear_gain(baked, RgMode::Album, 0.0, false), 0.5);
}

#[test]
fn track_mode_prefers_track_gain() {
    let baked = TrackReplayGain {
        track_gain: Some(-6.020_6),
        album_gain: Some(-3.0),
        ..Default::default()
    };
    approx(compute_linear_gain(baked, RgMode::Track, 0.0, false), 0.5);
}

#[test]
fn album_mode_falls_back_to_track_gain() {
    // Album gain absent → use track gain (and its peak) instead of unity.
    let baked = TrackReplayGain {
        track_gain: Some(-6.020_6),
        track_peak: Some(0.5),
        album_gain: None,
        album_peak: None,
    };
    approx(compute_linear_gain(baked, RgMode::Album, 0.0, false), 0.5);
}

#[test]
fn track_mode_falls_back_to_album_gain() {
    let baked = TrackReplayGain {
        track_gain: None,
        album_gain: Some(-6.020_6),
        ..Default::default()
    };
    approx(compute_linear_gain(baked, RgMode::Track, 0.0, false), 0.5);
}

#[test]
fn preamp_adds_to_gain() {
    // -6.02 dB gain + 6.02 dB preamp = 0 dB total = unity.
    let baked = TrackReplayGain {
        album_gain: Some(-6.020_6),
        ..Default::default()
    };
    approx(compute_linear_gain(baked, RgMode::Album, 6.020_6, false), 1.0);
}

#[test]
fn prevent_clipping_clamps_boost_by_peak() {
    // +6.02 dB gain would be ×2, but peak 0.8 caps the boost at 1/0.8 = 1.25.
    let baked = TrackReplayGain {
        album_gain: Some(6.020_6),
        album_peak: Some(0.8),
        ..Default::default()
    };
    approx(compute_linear_gain(baked, RgMode::Album, 0.0, true), 1.25);
    // With prevent-clipping off, the full ×2 boost applies.
    approx(compute_linear_gain(baked, RgMode::Album, 0.0, false), 2.0);
}

#[test]
fn prevent_clipping_no_clamp_when_peak_absent() {
    // No peak → no static clamp; rely on the downstream limiter. Full ×2 boost.
    let baked = TrackReplayGain {
        album_gain: Some(6.020_6),
        album_peak: None,
        ..Default::default()
    };
    approx(compute_linear_gain(baked, RgMode::Album, 0.0, true), 2.0);
}

#[test]
fn prevent_clipping_does_not_clamp_attenuation() {
    // A cut (-6 dB) already sits below full scale; the peak clamp only limits
    // boosts, so the attenuated gain passes through unchanged.
    let baked = TrackReplayGain {
        album_gain: Some(-6.020_6),
        album_peak: Some(0.99),
        ..Default::default()
    };
    approx(compute_linear_gain(baked, RgMode::Album, 0.0, true), 0.5);
}

#[test]
fn clamp_rg_preamp_bounds_and_nan() {
    approx(clamp_rg_preamp(0.0), 0.0);
    approx(clamp_rg_preamp(100.0), RG_MAX_PREAMP_DB);
    approx(clamp_rg_preamp(-100.0), RG_MIN_PREAMP_DB);
    approx(clamp_rg_preamp(f32::NAN), 0.0);
}

#[test]
fn rg_mode_u8_round_trip() {
    assert_eq!(RgMode::from_u8(RgMode::Track.to_u8()), RgMode::Track);
    assert_eq!(RgMode::from_u8(RgMode::Album.to_u8()), RgMode::Album);
    // Unknown byte falls back to Album.
    assert_eq!(RgMode::from_u8(99), RgMode::Album);
}

#[test]
fn rg_mode_settings_str_round_trip() {
    assert_eq!(RgMode::from_settings_str(RgMode::Track.as_settings_str()), RgMode::Track);
    assert_eq!(RgMode::from_settings_str(RgMode::Album.as_settings_str()), RgMode::Album);
    assert_eq!(RgMode::from_settings_str("nonsense"), RgMode::Album);
    assert_eq!(RgMode::default(), RgMode::Album);
}

#[test]
fn shared_setters_bump_generation_and_round_trip() {
    let shared = ReplayGainShared::new();
    let g0 = shared.generation();
    assert!(!shared.enabled());
    assert_eq!(shared.mode(), RgMode::Album);
    assert!(shared.prevent_clipping());

    shared.set_enabled(true);
    assert!(shared.enabled());
    assert!(shared.generation() > g0);

    shared.set_mode(RgMode::Track);
    assert_eq!(shared.mode(), RgMode::Track);

    shared.set_preamp(6.0);
    approx(shared.preamp(), 6.0);
    // Out-of-range preamp is clamped on store.
    shared.set_preamp(999.0);
    approx(shared.preamp(), RG_MAX_PREAMP_DB);

    shared.set_prevent_clipping(false);
    assert!(!shared.prevent_clipping());
}

#[test]
fn is_unity_gain_threshold() {
    assert!(is_unity_gain(1.0));
    assert!(is_unity_gain(1.000_01));
    assert!(!is_unity_gain(1.25));
    assert!(!is_unity_gain(0.5));
}
