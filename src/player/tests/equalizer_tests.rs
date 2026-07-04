//! Tests for the graphic-equalizer DSP core.

use std::num::NonZero;
use std::sync::Arc;
use std::time::Duration;

use rodio::source::SeekError;
use rodio::{ChannelCount, Sample, SampleRate, Source};

use super::{
    BAND_FREQS, EqShared, EqSource, MAX_GAIN_DB, MAX_PREAMP_DB, MIN_GAIN_DB, MIN_PREAMP_DB,
    NUM_BANDS, PRESETS, clamp_gain, clamp_preamp, normalize_gains, preset_index,
};
use crate::player::replaygain::{ReplayGainShared, RgMode, TrackReplayGain};

// --- helpers ---------------------------------------------------------------

fn nz_u16(v: u16) -> ChannelCount {
    match NonZero::new(v) {
        Some(n) => n,
        None => NonZero::<u16>::MIN,
    }
}

fn nz_u32(v: u32) -> SampleRate {
    match NonZero::new(v) {
        Some(n) => n,
        None => NonZero::<u32>::MIN,
    }
}

/// Scalar near-equality — avoids `clippy::float_cmp` on intentional exact
/// checks (clamp bounds, preset values).
fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

/// Bit pattern of each sample — lets us assert *bit-identical* passthrough
/// (and divergence) without float `==`.
fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|s| s.to_bits()).collect()
}

/// Deterministic non-trivial signal in [-1, 1] with broadband content, built
/// without int→float casts.
fn ramp(len: usize) -> Vec<f32> {
    const PATTERN: [f32; 8] = [-1.0, -0.6, -0.2, 0.2, 0.6, 1.0, 0.4, -0.4];
    (0..len).map(|i| PATTERN[i % PATTERN.len()]).collect()
}

/// In-memory source for tests. `try_seek` rewinds to the start (like a decoder
/// seeking to 0) so a post-seek run can be compared against a fresh run.
struct TestSource {
    data: Vec<f32>,
    pos: usize,
    channels: u16,
    sample_rate: u32,
}

impl TestSource {
    fn new(data: Vec<f32>, channels: u16, sample_rate: u32) -> Self {
        Self { data, pos: 0, channels, sample_rate }
    }
}

impl Iterator for TestSource {
    type Item = Sample;
    fn next(&mut self) -> Option<Sample> {
        let s = self.data.get(self.pos).copied();
        if s.is_some() {
            self.pos += 1;
        }
        s
    }
}

impl Source for TestSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> ChannelCount {
        nz_u16(self.channels)
    }
    fn sample_rate(&self) -> SampleRate {
        nz_u32(self.sample_rate)
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
    fn try_seek(&mut self, _pos: Duration) -> Result<(), SeekError> {
        self.pos = 0;
        Ok(())
    }
}

fn run_eq(gains: &[f32], enabled: bool, input: Vec<f32>, channels: u16) -> Vec<f32> {
    let shared = EqShared::new(enabled, gains);
    // Inert ReplayGain (disabled, no baked tags) so these tests exercise the EQ
    // path alone.
    let rg = ReplayGainShared::new();
    EqSource::new(TestSource::new(input, channels, 44_100), shared, rg, TrackReplayGain::default())
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
    EqSource::new(TestSource::new(input, 1, 44_100), shared, rg, baked).collect()
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
    )
    .collect();

    assert!(out.iter().all(|s| s.is_finite()));
    assert!(out.iter().all(|s| s.abs() <= 1.0 + 1e-6), "output must not exceed full scale");
    assert!(out.iter().any(|s| s.abs() > 0.01), "output must not be silent");
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
    let baked = TrackReplayGain { album_gain: Some(-6.020_6), ..Default::default() };

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
    let baked = TrackReplayGain { album_gain: Some(-6.020_6), ..Default::default() };

    let shared = EqShared::new(true, &[0.0; NUM_BANDS]);
    let mut src = EqSource::new(
        TestSource::new(input.clone(), 1, 44_100),
        shared,
        rg.clone(),
        baked,
    );

    let head: Vec<f32> = (0..256).filter_map(|_| src.next()).collect();
    assert_eq!(bits(&head), bits(&input[..256]), "flat EQ + RG off must be passthrough");

    rg.set_enabled(true);
    let tail: Vec<f32> = src.collect();
    assert!(tail.iter().all(|s| s.is_finite()));
    assert_ne!(
        bits(&tail),
        bits(&input[256..]),
        "a live ReplayGain-only change must take effect"
    );
}
