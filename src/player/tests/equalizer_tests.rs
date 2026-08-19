//! Tests for the graphic-equalizer DSP core.

use std::sync::Arc;
use std::time::Duration;

use rodio::Source;

use super::{
    BAND_FREQS, EqShared, EqSource, MAX_GAIN_DB, MAX_PREAMP_DB, MIN_GAIN_DB, MIN_PREAMP_DB,
    NUM_BANDS, PRESETS, clamp_gain, clamp_preamp, normalize_gains, preset_index,
};
use crate::player::crossfade::FadeShared;
use crate::player::replaygain::{ReplayGainShared, RgMode, TrackReplayGain};
use crate::player::tests::helpers::{TestSource, approx_eq as approx, bits};

// --- helpers ---------------------------------------------------------------

/// Deterministic non-trivial signal in [-1, 1] with broadband content, built
/// without int→float casts.
fn ramp(len: usize) -> Vec<f32> {
    const PATTERN: [f32; 8] = [-1.0, -0.6, -0.2, 0.2, 0.6, 1.0, 0.4, -0.4];
    (0..len).map(|i| PATTERN[i % PATTERN.len()]).collect()
}

fn run_eq(gains: &[f32], enabled: bool, input: Vec<f32>, channels: u16) -> Vec<f32> {
    let shared = EqShared::new(enabled, gains);
    // Inert ReplayGain (disabled, no baked tags) so these tests exercise the EQ
    // path alone.
    let rg = ReplayGainShared::new();
    EqSource::new(
        TestSource::new(input, channels, 44_100),
        shared,
        rg,
        TrackReplayGain::default(),
        FadeShared::idle(),
    )
    .collect()
}

/// Collect an `EqSource` over a mono 44.1 kHz signal with the given EQ-enabled
/// flag (bands flat) and `ReplayGain` state (shared master + baked per-track tags).
fn run_rg(
    eq_enabled: bool,
    rg: Arc<ReplayGainShared>,
    baked: TrackReplayGain,
    input: Vec<f32>,
) -> Vec<f32> {
    let shared = EqShared::new(eq_enabled, &[0.0; NUM_BANDS]);
    EqSource::new(TestSource::new(input, 1, 44_100), shared, rg, baked, FadeShared::idle())
        .collect()
}

// --- tests -----------------------------------------------------------------

#[test]
fn clamp_gain_bounds_and_nan() {
    assert!(approx(clamp_gain(0.0), 0.0));
    assert!(approx(clamp_gain(100.0), MAX_GAIN_DB));
    assert!(approx(clamp_gain(-100.0), MIN_GAIN_DB));
    assert!(approx(clamp_gain(f32::NAN), 0.0));
}

#[test]
fn normalize_gains_pads_truncates_clamps() {
    // Short input pads with zeros.
    let short = normalize_gains(&[3.0, -3.0]);
    assert!(approx(short[0], 3.0));
    assert!(approx(short[1], -3.0));
    assert!(short[2..].iter().all(|g| approx(*g, 0.0)));

    // Over-long input is truncated to NUM_BANDS and clamped.
    let long = normalize_gains(&[99.0; NUM_BANDS + 5]);
    assert_eq!(long.len(), NUM_BANDS);
    assert!(long.iter().all(|g| approx(*g, MAX_GAIN_DB)));
}

#[test]
fn presets_well_formed() {
    assert_eq!(preset_index("Flat"), Some(0));
    assert!(preset_index("Nope").is_none());
    // Flat is neutral.
    assert!(PRESETS[0].gains.iter().all(|g| approx(*g, 0.0)));
    // Every preset stays within range and has the right arity.
    for p in &PRESETS {
        assert_eq!(p.gains.len(), NUM_BANDS);
        assert!(p.gains.iter().all(|g| *g >= MIN_GAIN_DB && *g <= MAX_GAIN_DB));
    }
    assert_eq!(BAND_FREQS.len(), NUM_BANDS);
}

#[test]
fn disabled_is_bit_identical_passthrough() {
    let input = ramp(512);
    let out = run_eq(&[12.0; NUM_BANDS], false, input.clone(), 2);
    assert_eq!(bits(&out), bits(&input));
}

#[test]
fn enabled_but_flat_is_passthrough() {
    let input = ramp(512);
    let out = run_eq(&[0.0; NUM_BANDS], true, input.clone(), 2);
    assert_eq!(bits(&out), bits(&input));
}

#[test]
fn active_eq_alters_signal_and_stays_finite() {
    let input = ramp(1024);
    let mut gains = [0.0; NUM_BANDS];
    gains[0] = 12.0; // strong low-band boost
    let out = run_eq(&gains, true, input.clone(), 1);

    assert_eq!(out.len(), input.len());
    assert!(out.iter().all(|s| s.is_finite()));
    assert_ne!(bits(&out), bits(&input), "an active band must change the signal");
}

#[test]
fn band_outside_nyquist_is_skipped() {
    // 16 kHz band at an 8 kHz sample rate (Nyquist 4 kHz) must not panic and
    // must leave the signal untouched (no valid coefficients → bypass).
    let mut gains = [0.0; NUM_BANDS];
    let top = NUM_BANDS - 1;
    assert!(BAND_FREQS[top] > 4_000.0);
    gains[top] = 12.0;

    let input = ramp(256);
    let shared = EqShared::new(true, &gains);
    let out: Vec<f32> = EqSource::new(
        TestSource::new(input.clone(), 1, 8_000),
        shared,
        ReplayGainShared::new(),
        TrackReplayGain::default(),
        FadeShared::idle(),
    )
    .collect();
    assert_eq!(bits(&out), bits(&input));
}

#[test]
fn seek_resets_filter_state() {
    let mut gains = [0.0; NUM_BANDS];
    gains[2] = 10.0;
    let input = ramp(800);

    // Reference: a fresh source over the whole input.
    let fresh = run_eq(&gains, true, input.clone(), 1);

    // Warm a source's delay lines on the first half, then seek back to 0 and
    // collect the full run. With state reset, it must match the fresh run.
    let shared = EqShared::new(true, &gains);
    let mut src = EqSource::new(
        TestSource::new(input.clone(), 1, 44_100),
        shared,
        ReplayGainShared::new(),
        TrackReplayGain::default(),
        FadeShared::idle(),
    );
    for _ in 0..400 {
        let _ = src.next();
    }
    assert!(src.try_seek(Duration::ZERO).is_ok());
    let after_seek: Vec<f32> = src.collect();

    assert_eq!(after_seek.len(), input.len());
    assert_eq!(bits(&after_seek), bits(&fresh), "seek must reset filter delay lines");
}

#[test]
fn live_gain_change_is_observed_via_generation() {
    let shared = EqShared::new(true, &[0.0; NUM_BANDS]);
    let mut src = EqSource::new(
        TestSource::new(ramp(2048), 1, 44_100),
        shared.clone(),
        ReplayGainShared::new(),
        TrackReplayGain::default(),
        FadeShared::idle(),
    );

    // First samples with a flat curve are passthrough.
    let head: Vec<f32> = (0..256).filter_map(|_| src.next()).collect();
    let flat_input = ramp(2048);
    assert_eq!(bits(&head), bits(&flat_input[..256]));

    // Apply a boost mid-stream; subsequent samples must diverge from the input.
    shared.set_gain(1, 12.0);
    let tail: Vec<f32> = src.collect();
    assert!(tail.iter().all(|s| s.is_finite()));
    assert_ne!(bits(&tail), bits(&flat_input[256..]), "a live gain change must take effect");
}

#[test]
fn preamp_clamps_and_roundtrips() {
    assert!(approx(clamp_preamp(0.0), 0.0));
    assert!(approx(clamp_preamp(99.0), MAX_PREAMP_DB));
    assert!(approx(clamp_preamp(-99.0), MIN_PREAMP_DB));
    assert!(approx(clamp_preamp(f32::NAN), 0.0));

    let shared = EqShared::new(true, &[0.0; NUM_BANDS]);
    shared.set_preamp(99.0);
    assert!(approx(shared.preamp(), MAX_PREAMP_DB));
    shared.set_preamp(-3.0);
    assert!(approx(shared.preamp(), -3.0));
}

#[test]
fn preamp_scales_a_flat_curve() {
    // Flat bands but a -6 dB preamp: not bypassed, output is the input scaled by
    // the linear gain (low enough that the limiter never engages).
    let input = ramp(512);
    let factor = 10.0_f32.powf(-6.0 / 20.0);

    let shared = EqShared::new(true, &[0.0; NUM_BANDS]);
    shared.set_preamp(-6.0);
    let out: Vec<f32> = EqSource::new(
        TestSource::new(input.clone(), 2, 44_100),
        shared,
        ReplayGainShared::new(),
        TrackReplayGain::default(),
        FadeShared::idle(),
    )
    .collect();

    assert_ne!(bits(&out), bits(&input), "a non-zero preamp must change a flat curve");
    assert!(out.iter().zip(&input).all(|(o, i)| approx(*o, *i * factor)));
}

#[test]
fn limiter_keeps_heavy_boost_within_full_scale() {
    // Max boost on every band + max preamp on a near-full-scale signal would
    // blow well past ±1.0 without protection; the preamp/limiter/clamp chain
    // must keep every output sample inside full scale.
    let input = ramp(4096);
    let shared = EqShared::new(true, &[MAX_GAIN_DB; NUM_BANDS]);
    shared.set_preamp(MAX_PREAMP_DB);
    let out: Vec<f32> = EqSource::new(
        TestSource::new(input, 2, 44_100),
        shared,
        ReplayGainShared::new(),
        TrackReplayGain::default(),
        FadeShared::idle(),
    )
    .collect();

    assert!(out.iter().all(|s| s.is_finite()));
    assert!(out.iter().all(|s| s.abs() <= 1.0 + 1e-6), "output must not exceed full scale");
    assert!(out.iter().any(|s| s.abs() > 0.01), "output must not be silent");
}

#[test]
fn limiter_timing_is_channel_count_independent() {
    // The limiter updates once per interleaved frame, so its attack/release time
    // constants must depend only on the per-channel sample rate — NOT the channel
    // count. With flat bands + a boost preamp, mono and stereo sources at the same
    // sample rate must produce a bit-identical per-frame gain envelope: each
    // stereo frame's two samples equal the matching mono sample. A
    // `frame_rate = sample_rate / channels` bug makes the stereo envelope decay
    // ~2× faster, so the frames diverge and this fails.
    const FRAMES: usize = 2048;
    let build = |channels: u16| {
        let shared = EqShared::new(true, &[0.0; NUM_BANDS]);
        // Boost preamp → peak well above full scale, so the limiter engages and
        // its gain envelope (the thing driven by the time constants) actually moves.
        shared.set_preamp(MAX_PREAMP_DB);
        let samples = vec![1.0_f32; FRAMES * usize::from(channels)];
        EqSource::new(
            TestSource::new(samples, channels, 44_100),
            shared,
            ReplayGainShared::new(),
            TrackReplayGain::default(),
            FadeShared::idle(),
        )
        .collect::<Vec<f32>>()
    };

    let mono = build(1);
    let stereo = build(2);
    assert_eq!(mono.len(), FRAMES);
    assert_eq!(stereo.len(), FRAMES * 2);

    // The envelope must actually move (limiter engaged), else the test is vacuous.
    assert_ne!(
        mono.first().map(|s| s.to_bits()),
        mono.last().map(|s| s.to_bits()),
        "limiter must attenuate over time for this test to be meaningful"
    );

    for (frame, (m, s)) in mono.iter().zip(stereo.chunks_exact(2)).enumerate() {
        assert_eq!(
            m.to_bits(),
            s[0].to_bits(),
            "frame {frame}: stereo channel matches mono → limiter timing is channel-independent"
        );
        assert_eq!(
            s[0].to_bits(),
            s[1].to_bits(),
            "frame {frame}: both stereo channels share one per-frame limiter gain"
        );
    }
}

#[test]
fn shared_gain_setters_clamp_and_roundtrip() {
    let shared = EqShared::new(false, &[0.0; NUM_BANDS]);
    shared.set_gain(0, 99.0);
    shared.set_gain(1, -99.0);
    let g = shared.gains();
    assert!(approx(g[0], MAX_GAIN_DB));
    assert!(approx(g[1], MIN_GAIN_DB));
    assert!(approx(shared.gain(0), MAX_GAIN_DB));

    shared.set_all_gains(&[4.0; NUM_BANDS]);
    assert!(shared.gains().iter().all(|v| approx(*v, 4.0)));

    assert!(!shared.enabled());
    shared.set_enabled(true);
    assert!(shared.enabled());

    // Out-of-range index is a no-op, not a panic.
    shared.set_gain(999, 5.0);
}

// --- ReplayGain integration (⚠ M1 / M2 guards) -----------------------------

#[test]
fn replaygain_applies_with_eq_disabled() {
    // ⚠ M1: ReplayGain enabled, EQ DISABLED. The source must NOT bypass — output
    // is the input scaled by the baked gain (-6.02 dB ≈ ×0.5, low enough that
    // the limiter never engages), proving the EQ-off early return doesn't kill RG.
    let input = ramp(512);
    let rg = ReplayGainShared::new();
    rg.set_enabled(true);
    rg.set_mode(RgMode::Album);
    let baked = TrackReplayGain {
        album_gain: Some(-6.020_6),
        ..Default::default()
    };

    let out = run_rg(false, rg, baked, input.clone());

    assert_ne!(bits(&out), bits(&input), "ReplayGain must apply even with the EQ off");
    assert!(out.iter().all(|s| s.is_finite()));
    assert!(out.iter().zip(&input).all(|(o, i)| approx(*o, *i * 0.5)));
}

#[test]
fn replaygain_enabled_untagged_track_is_passthrough() {
    // RG enabled but the track has no tags and the EQ is off → unity gain → the
    // bit-identical bypass still holds (no wasted DSP on untagged tracks).
    let input = ramp(512);
    let rg = ReplayGainShared::new();
    rg.set_enabled(true);
    let out = run_rg(false, rg, TrackReplayGain::default(), input.clone());
    assert_eq!(bits(&out), bits(&input));
}

#[test]
fn live_replaygain_change_is_observed_via_generation() {
    // ⚠ M2: EQ enabled but flat (0 dB, no preamp), so only ReplayGain can alter
    // the signal. Start with RG disabled → passthrough; enable RG mid-stream with
    // NO EQ change → subsequent samples must diverge, proving `next()` polls the
    // ReplayGain generation too.
    let input = ramp(2048);
    let rg = ReplayGainShared::new(); // starts disabled
    let baked = TrackReplayGain {
        album_gain: Some(-6.020_6),
        ..Default::default()
    };

    let shared = EqShared::new(true, &[0.0; NUM_BANDS]);
    let mut src = EqSource::new(
        TestSource::new(input.clone(), 1, 44_100),
        shared,
        rg.clone(),
        baked,
        FadeShared::idle(),
    );

    let head: Vec<f32> = (0..256).filter_map(|_| src.next()).collect();
    assert_eq!(bits(&head), bits(&input[..256]), "flat EQ + RG off must be passthrough");

    rg.set_enabled(true);
    let tail: Vec<f32> = src.collect();
    assert!(tail.iter().all(|s| s.is_finite()));
    assert_ne!(bits(&tail), bits(&input[256..]), "a live ReplayGain-only change must take effect");
}

// --- crossfade ramp --------------------------------------------------------

/// Ramp length in interleaved samples for a mono 1 kHz source. Keeps the media
/// milliseconds → samples conversion easy to reason about in the assertions.
const RATE: u32 = 1_000;

/// A flat, inert `EqSource` over `input`, sharing `fade` with its deck.
fn faded_source(input: Vec<f32>, channels: u16, fade: Arc<FadeShared>) -> EqSource<TestSource> {
    EqSource::new(
        TestSource::new(input, channels, RATE),
        EqShared::new(false, &[0.0; NUM_BANDS]),
        ReplayGainShared::new(),
        TrackReplayGain::default(),
        fade,
    )
}

#[test]
fn an_idle_fade_cell_keeps_the_bit_identical_bypass() {
    let input = ramp(512);
    let out: Vec<f32> = faded_source(input.clone(), 1, FadeShared::idle()).collect();
    assert_eq!(bits(&out), bits(&input), "an idle ramp must not touch the signal");
}

#[test]
fn a_fade_out_ends_the_source_when_the_ramp_lands() {
    // 100 ms at 1 kHz mono = 100 interleaved samples of ramp, then the source
    // must end — that is what drains the deck and completes the crossfade.
    // Gains run over positions 0..99, so the last emitted sample sits at 1/100
    // of full scale and the source ends instead of emitting an exact zero.
    let fade = FadeShared::idle();
    fade.arm(None, 0.0, 100, true);
    let out: Vec<f32> = faded_source(vec![1.0; 512], 1, fade).collect();

    assert_eq!(out.len(), 100, "source must end as soon as the ramp lands");
    assert!(approx(out[0], 1.0), "fade-out starts at unity");
    assert!(approx(out[50], 0.5), "and is linear through the midpoint");
    assert!(approx(out[99], 0.01), "and ends a hair above silence");
}

#[test]
fn a_fade_in_climbs_to_unity_and_restores_the_bypass() {
    let fade = FadeShared::idle();
    fade.arm(Some(0.0), 1.0, 100, false);
    let out: Vec<f32> = faded_source(vec![1.0; 300], 1, fade).collect();

    assert_eq!(out.len(), 300, "a fade-in must never end the source");
    assert!(approx(out[0], 0.0));
    assert!(approx(out[50], 0.5));
    // Past the ramp the source disengages the fade stage entirely, so the tail
    // is bit-identical passthrough rather than a multiply by 1.0.
    assert_eq!(bits(&out[150..]), bits(&vec![1.0_f32; 150]));
}

#[test]
fn complementary_ramps_on_two_decks_never_sum_past_unity() {
    // The property that keeps rodio's unclamped mixer safe. Both "decks" run a
    // full-scale signal; their summed output must stay inside full scale.
    let out_fade = FadeShared::idle();
    out_fade.arm(None, 0.0, 200, true);
    let in_fade = FadeShared::idle();
    in_fade.arm(Some(0.0), 1.0, 200, false);

    let outgoing: Vec<f32> = faded_source(vec![1.0; 400], 1, out_fade).collect();
    let incoming: Vec<f32> = faded_source(vec![1.0; 400], 1, in_fade).collect();

    for (i, o) in outgoing.iter().enumerate() {
        let sum = o + incoming[i];
        assert!(sum <= 1.0 + 1e-4, "decks summed to {sum} at sample {i}");
    }
}

#[test]
fn a_fade_advances_once_per_frame_not_once_per_sample() {
    // Stereo: a frame is one sample per channel, i.e. one time step. A 100 ms
    // ramp at 1 kHz stereo is 200 interleaved samples, and both samples of a
    // frame must carry the same gain or the stereo image shears mid-fade.
    let fade = FadeShared::idle();
    fade.arm(None, 0.0, 100, true);
    let out: Vec<f32> = faded_source(vec![1.0; 1_024], 2, fade).collect();

    assert_eq!(out.len(), 200, "100 frames of ramp, two samples each");
    for frame in out.chunks_exact(2) {
        assert!(approx(frame[0], frame[1]), "both channels of a frame share one gain");
    }
    assert!(approx(out[0], 1.0));
    assert!(approx(out[100], 0.5), "frame 50 of 100 is the midpoint");
}

#[test]
fn the_fade_engages_the_clamp_on_a_bypassed_hot_source() {
    // Raw decoder output can exceed full scale. In the bypass path the fade
    // must still clamp before scaling, or two overlapping decks feeding rodio's
    // unclamped mixer could sum past unity.
    let fade = FadeShared::idle();
    fade.arm(Some(1.0), 1.0, 1_000, false);
    let out: Vec<f32> = faded_source(vec![1.8; 64], 1, fade).collect();
    assert!(out.iter().all(|s| *s <= 1.0 + 1e-6), "hot samples must be clamped");
}

#[test]
fn a_live_fade_arm_is_observed_via_generation() {
    // The cell is deck-scoped, so a ramp armed mid-track applies to whatever
    // source that deck is playing.
    let fade = FadeShared::idle();
    let mut src = faded_source(vec![1.0; 512], 1, fade.clone());

    let head: Vec<f32> = (0..64).filter_map(|_| src.next()).collect();
    assert_eq!(bits(&head), bits(&vec![1.0_f32; 64]), "idle cell is passthrough");

    fade.arm(None, 0.0, 100, true);
    let tail: Vec<f32> = src.collect();
    assert_eq!(tail.len(), 100, "the armed fade-out must take effect and end the source");
    assert!(approx(tail[0], 1.0));
}

#[test]
fn a_fade_armed_mid_frame_still_steps_on_frame_boundaries() {
    // Stereo, bypassed (EQ off + unity ReplayGain — the default), which is the
    // path a crossfade or pause-fade normally arms onto. Pull an ODD number of
    // samples first, so the source sits *mid*-frame when the ramp arrives. The
    // bypass path tracks its interleave phase precisely so that the ramp still
    // steps on real frame boundaries: both channels of a frame share one gain,
    // and the source ends on a boundary rather than emitting a half frame.
    let fade = FadeShared::idle();
    let mut src = faded_source(vec![1.0; 1_024], 2, fade.clone());

    let head: Vec<f32> = (0..65).filter_map(|_| src.next()).collect();
    assert_eq!(bits(&head), bits(&vec![1.0_f32; 65]), "the bypass prefix is passthrough");

    fade.arm(None, 0.0, 100, true);
    let full: Vec<f32> = head.into_iter().chain(src).collect();

    assert_eq!(full.len() % 2, 0, "the source must not end on a half frame");
    for (i, frame) in full.chunks_exact(2).enumerate() {
        assert!(
            approx(frame[0], frame[1]),
            "both channels of frame {i} must share one gain, got {} and {}",
            frame[0],
            frame[1]
        );
    }
}

#[test]
fn enabling_the_eq_mid_frame_still_ends_the_source_on_a_frame_boundary() {
    // Stereo. Pull an ODD number of samples in bypass, then switch the EQ on, so
    // the rebuild lands *mid*-frame. The active path buffers a whole `channels`-
    // wide frame at a time, so if it were allowed to start from that odd offset
    // every frame it formed would be off by one — and the self-ending fade-out
    // below would then end the source *between* the two channels of a frame,
    // flipping that deck's channel parity in rodio's mixer for every track
    // appended to it afterwards. The generation poll only fires at phase 0, so
    // the switch waits for the next real boundary.
    let fade = FadeShared::idle();
    let eq = EqShared::new(false, &[0.0; NUM_BANDS]);
    let mut src = EqSource::new(
        TestSource::new(ramp(2_048), 2, RATE),
        eq.clone(),
        ReplayGainShared::new(),
        TrackReplayGain::default(),
        fade.clone(),
    );

    let mut out: Vec<f32> = (0..65).filter_map(|_| src.next()).collect();
    assert_eq!(out.len(), 65, "the bypass prefix must yield every sample asked for");

    // Mid-frame: 65 samples of a stereo stream is half of frame 32.
    eq.set_enabled(true);
    eq.set_gain(0, 6.0);

    // A fade-out that ends its source once the ramp lands — the crossfade's
    // outgoing arm. 100 ms at 1 kHz is 200 interleaved samples, well inside the
    // 2 048 the input holds, so it really is the ramp that ends the source.
    fade.arm(None, 0.0, 100, true);
    out.extend(src);

    assert!(out.len() < 2_048, "the ramp, not the input, must end the source");
    assert_eq!(out.len() % 2, 0, "the source must not end on a half frame");
}

#[test]
fn seek_does_not_disturb_an_armed_fade() {
    // `set_speed` re-anchors the active deck with a `try_seek`, and a crossfade
    // abort arms a ramp then seeks. Either resetting `fade_pos` would restart
    // the ramp from its start gain.
    let fade = FadeShared::idle();
    fade.arm(None, 0.0, 100, true);
    let mut src = faded_source(vec![1.0; 512], 1, fade);

    let before: Vec<f32> = (0..50).filter_map(|_| src.next()).collect();
    assert!(approx(before[49], 0.51), "ramp is halfway down");

    assert!(src.try_seek(Duration::ZERO).is_ok());
    let after = src.next();
    // `try_seek` rewound the TestSource, but the ramp keeps its position: the
    // next gain continues the descent rather than jumping back to unity.
    assert!(after.is_some());
    if let Some(g) = after {
        assert!(approx(g, 0.5), "fade position must survive a seek, got {g}");
    }
}
