//! The graphic equalizer. [`EqShared`] is the lock-free control state and [`EqSource`] reads it,
//! one [`DirectForm1`] per band per channel, Form 1 because its delay line stays valid across a
//! live coefficient swap, so a slider drag injects no transient.
//!
//! [`EqSource`] also carries three stages the EQ toggle doesn't gate, each polled on its own and
//! each arguing its place in its own module: the track's `ReplayGain` and the headphone correction
//! before the bands, and the deck's crossfade ramp after the clamp. With all four inert it is a
//! bit-identical passthrough.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use biquad::{Biquad, Coefficients, DirectForm1, Hertz, Type};

use super::correction::bank::CorrectionStage;
use super::correction::response::FilterResponse;
use super::correction::shared::{CorrectionShared, CorrectionWriter};
use super::crossfade::{self, FadeShared};
use super::dsp::{AtomicF32, Generation, PASSTHROUGH, db_to_linear, linear_to_db};
use super::replaygain::{self, ReplayGainShared, TrackReplayGain};
use melodia_audio::player::source::audio::{
    AudioSource, ChannelCount, Sample, SampleRate, SeekError, SourceFormat,
};

pub const NUM_BANDS: usize = 10;

/// ISO octave centre frequencies (Hz) for the ten bands, low → high.
pub const BAND_FREQS: [f32; NUM_BANDS] =
    [31.0, 62.0, 125.0, 250.0, 500.0, 1_000.0, 2_000.0, 4_000.0, 8_000.0, 16_000.0];

/// Per-band gain range, in decibels.
pub const MIN_GAIN_DB: f32 = -12.0;
pub const MAX_GAIN_DB: f32 = 12.0;

/// Preamp (master input gain) range, in decibels. Asymmetric: generous cut for
/// headroom after boosting, a little boost for makeup gain. 0 dB is unity.
pub const MIN_PREAMP_DB: f32 = -12.0;
pub const MAX_PREAMP_DB: f32 = 6.0;

/// Quality factor for the peaking filters. ~1.4 gives musically sensible
/// overlap for octave-spaced bands without ringing.
const BAND_Q: f64 = 1.41;

/// Bands within this many dB of unity are treated as off (skipped in the
/// filter chain) so a near-flat EQ stays close to free.
const GAIN_EPSILON_DB: f32 = 0.05;

/// A preamp within this many dB of unity is treated as 0 dB (no gain stage), so
/// a flat EQ with no preamp can still take the bit-identical bypass path.
const PREAMP_EPSILON_DB: f32 = 0.01;

// The safety limiter: transparent until near full scale, then it pins peaks at the threshold. In
// the active path only, so audio with nothing engaged is never touched.
const LIMITER_THRESHOLD_DB: f32 = -1.0;
const LIMITER_KNEE_DB: f32 = 4.0;
const LIMITER_ATTACK_S: f32 = 0.005;
const LIMITER_RELEASE_S: f32 = 0.100;

/// How long the output reads exactly zero before the filters' state is cleared.
const SILENCE_SETTLE_MS: u64 = 10;

/// `PRESETS` order is load-bearing: the preset dropdown lists the names as inline `@tr` literals
/// in this order and maps its index straight in.
pub struct EqPreset {
    pub name: &'static str,
    pub gains: [f32; NUM_BANDS],
}

/// Built-in presets. `Flat` (index 0) is the neutral default; the UI appends a
/// synthetic "Custom" entry after these for hand-tuned curves.
pub const PRESETS: [EqPreset; 9] = [
    EqPreset { name: "Flat", gains: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0] },
    EqPreset { name: "Rock", gains: [4.0, 3.0, 2.0, 0.0, -1.0, -1.0, 1.0, 3.0, 4.0, 4.0] },
    EqPreset { name: "Pop", gains: [-1.0, 0.0, 1.0, 2.0, 3.0, 3.0, 2.0, 1.0, 0.0, -1.0] },
    EqPreset { name: "Jazz", gains: [3.0, 2.0, 1.0, 2.0, -1.0, -1.0, 0.0, 1.0, 2.0, 3.0] },
    EqPreset { name: "Classical", gains: [4.0, 3.0, 2.0, 1.0, -1.0, -1.0, 0.0, 2.0, 3.0, 4.0] },
    EqPreset { name: "Bass Boost", gains: [6.0, 5.0, 4.0, 2.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0] },
    EqPreset { name: "Treble Boost", gains: [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 4.0, 5.0, 6.0] },
    EqPreset { name: "Vocal", gains: [-2.0, -1.0, 0.0, 2.0, 4.0, 4.0, 3.0, 1.0, 0.0, -1.0] },
    EqPreset { name: "Electronic", gains: [5.0, 4.0, 1.0, 0.0, -2.0, 1.0, 0.0, 1.0, 3.0, 5.0] },
];

/// Number of built-in presets — also the index of the UI's synthetic "Custom"
/// dropdown entry.
pub const PRESET_COUNT: usize = PRESETS.len();

/// Default preset name persisted on first launch.
pub const DEFAULT_PRESET: &str = "Flat";

/// Sentinel persisted for a hand-tuned curve. Deliberately absent from
/// [`PRESETS`], so [`preset_index`] returns `None` and the UI falls through to
/// the synthetic slot at [`PRESET_COUNT`].
pub const CUSTOM_PRESET: &str = "Custom";

#[must_use]
pub fn clamp_gain(db: f32) -> f32 {
    if db.is_nan() { 0.0 } else { db.clamp(MIN_GAIN_DB, MAX_GAIN_DB) }
}

#[must_use]
pub fn clamp_preamp(db: f32) -> f32 {
    if db.is_nan() { 0.0 } else { db.clamp(MIN_PREAMP_DB, MAX_PREAMP_DB) }
}

/// Coerces a hand-edited or wrong-length list: pads with 0, drops extras, clamps.
#[must_use]
pub fn normalize_gains(gains: &[f32]) -> [f32; NUM_BANDS] {
    std::array::from_fn(|i| clamp_gain(gains.get(i).copied().unwrap_or(0.0)))
}

#[must_use]
pub fn preset_index(name: &str) -> Option<usize> {
    PRESETS.iter().position(|p| p.name == name)
}

/// Lock-free equalizer state: the control layer writes, the audio thread reads.
/// Every mutation bumps the [`Generation`] so [`EqSource`] knows to recompute.
pub struct EqShared {
    enabled: AtomicBool,
    gains: [AtomicF32; NUM_BANDS],
    preamp: AtomicF32,
    generation: Generation,
}

impl EqShared {
    /// Build shared state seeded from persisted settings. The preamp starts at
    /// unity (0 dB); seed it separately via [`set_preamp`](Self::set_preamp).
    #[must_use]
    pub fn new(enabled: bool, gains: &[f32]) -> Arc<Self> {
        let norm = normalize_gains(gains);
        Arc::new(Self {
            enabled: AtomicBool::new(enabled),
            gains: std::array::from_fn(|i| AtomicF32::new(norm[i])),
            preamp: AtomicF32::new(0.0),
            generation: Generation::new(),
        })
    }

    fn bump(&self) {
        self.generation.bump();
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
        self.bump();
    }

    pub fn set_gain(&self, index: usize, db: f32) {
        if let Some(cell) = self.gains.get(index) {
            cell.store(clamp_gain(db));
            self.bump();
        }
    }

    pub fn set_all_gains(&self, gains: &[f32]) {
        let norm = normalize_gains(gains);
        for (cell, g) in self.gains.iter().zip(norm) {
            cell.store(g);
        }
        self.bump();
    }

    pub fn set_preamp(&self, db: f32) {
        self.preamp.store(clamp_preamp(db));
        self.bump();
    }

    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn gain(&self, index: usize) -> f32 {
        self.gains.get(index).map_or(0.0, AtomicF32::load)
    }

    #[must_use]
    pub fn gains(&self) -> [f32; NUM_BANDS] {
        std::array::from_fn(|i| self.gains[i].load())
    }

    #[must_use]
    pub fn preamp(&self) -> f32 {
        self.preamp.load()
    }

    #[must_use]
    fn generation(&self) -> u64 {
        self.generation.get()
    }
}

/// The engine-wide state every [`EqSource`] polls.
#[derive(Clone)]
pub struct DspControls {
    pub eq: Arc<EqShared>,
    pub rg: Arc<ReplayGainShared>,
    pub correction: Arc<CorrectionShared>,
}

/// Its writer dropped, the correction stays empty for these sources' lives.
impl Default for DspControls {
    fn default() -> Self {
        Self {
            eq: EqShared::new(false, &[0.0; NUM_BANDS]),
            rg: ReplayGainShared::new(),
            correction: CorrectionWriter::default().shared(),
        }
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "the cascade's output is an audio sample, far inside f32's range, and rounding it back to the sample width is the point"
)]
#[inline]
fn narrow_sample(sample: f64) -> f32 {
    sample as f32
}

/// One-pole smoothing coefficient for a time constant at a given update rate:
/// the smoothed value moves `(1 - coeff)` of the way to its target each update.
fn smoothing_coeff(time_s: f32, rate: f32) -> f32 {
    if rate > 0.0 && time_s > 0.0 { (-1.0 / (time_s * rate)).exp() } else { 0.0 }
}

/// Soft-knee feed-forward peak limiter (Giannoulis et al. 2012): one gain per frame from its peak,
/// smoothed with separate attack and release, and coupled across channels to keep the image.
struct Limiter {
    /// Smoothed linear gain (≤ 1.0); starts at unity.
    gain: f32,
    attack_coeff: f32,
    release_coeff: f32,
    /// Linear, so [`Self::target_gain`] answers a quiet frame without a `log10`.
    knee_low_linear: f32,
    /// Linear, the curve having a closed linear form past it.
    knee_high_linear: f32,
    /// Above the knee, the gain's numerator.
    threshold_linear: f32,
}

impl Limiter {
    fn new(frame_rate: f32) -> Self {
        Self {
            gain: 1.0,
            attack_coeff: smoothing_coeff(LIMITER_ATTACK_S, frame_rate),
            release_coeff: smoothing_coeff(LIMITER_RELEASE_S, frame_rate),
            knee_low_linear: db_to_linear(LIMITER_THRESHOLD_DB - LIMITER_KNEE_DB / 2.0),
            knee_high_linear: db_to_linear(LIMITER_THRESHOLD_DB + LIMITER_KNEE_DB / 2.0),
            threshold_linear: db_to_linear(LIMITER_THRESHOLD_DB),
        }
    }

    fn reset(&mut self) {
        self.gain = 1.0;
    }

    /// The soft-knee curve with an unbounded ratio above the knee.
    fn target_gain(&self, peak: f32) -> f32 {
        // Unity below the knee, so the common quiet frame skips the `log10`.
        if peak <= self.knee_low_linear {
            return 1.0;
        }
        // The output pins at the threshold; `10^(-over/20)` in dB expands to exactly this.
        if peak >= self.knee_high_linear {
            return self.threshold_linear / peak;
        }
        // The quadratic knee, meeting both guards' answers at its edges.
        let over = linear_to_db(peak) - LIMITER_THRESHOLD_DB;
        let k = over + LIMITER_KNEE_DB / 2.0;
        db_to_linear(-(k * k) / (2.0 * LIMITER_KNEE_DB))
    }

    fn process(&mut self, peak: f32) -> f32 {
        let target = self.target_gain(peak);
        let coeff = if target < self.gain { self.attack_coeff } else { self.release_coeff };
        self.gain = coeff.mul_add(self.gain, (1.0 - coeff) * target);
        self.gain
    }
}

/// The EQ, `ReplayGain` and fade over one decoded track. The master state is shared, so a change
/// reaches the staged track too; the per-track gain is baked per source, a staged track having its
/// own tags.
#[expect(
    clippy::struct_excessive_bools,
    reason = "audio-thread hot state: each flag gates a distinct branch in `next()` and is read per sample; packing them into bitflags would add masking to the hot path"
)]
pub struct EqSource<S> {
    input: S,
    controls: DspControls,
    baked_rg: TrackReplayGain,
    /// RG polled apart, so an RG-only change still rebuilds.
    last_generation: u64,
    last_rg_generation: u64,
    /// Shared with a gapless successor on this deck but never with the other deck, so a fade armed
    /// here moves only this voice.
    fade: Arc<FadeShared>,
    last_fade_generation: u64,
    /// A ramp armed before this source was built was armed for another, so its start is ignored: a
    /// gapless successor would otherwise replay a crossfade's fade-in from silence.
    fade_generation_at_build: u64,
    /// `false` is unity, keeping the bit-identical bypass available.
    fade_engaged: bool,
    fade_ramping: bool,
    fade_start: f32,
    fade_target: f32,
    /// Also where a ramp armed with no start begins.
    fade_gain: f32,
    /// In interleaved samples of this source.
    fade_pos: u64,
    fade_total: u64,
    /// Tracked by the bypass path, the active one buffering whole frames. The ramp steps once a
    /// frame so both channels share a gain, and the generation poll fires only at `0`, so a
    /// rebuild never hands the active path a mid-frame start.
    frame_phase: usize,
    frame_fade_gain: f32,
    fade_end_on_complete: bool,
    /// Constant per decoded file, so coefficients built from it hold for the source's life.
    sample_rate: f32,
    sample_rate_hz: u64,
    channels: u64,
    /// `[channel][band]`, its length the channel count. In f64: a low band at a high rate puts its
    /// poles near z = 1, where f32 coefficients and state stop holding the curve.
    banks: Vec<[DirectForm1<f64>; NUM_BANDS]>,
    /// A band is inactive at unity gain or outside Nyquist.
    band_active: [bool; NUM_BANDS],
    /// For the correction's headroom, the banks' own coefficients being private to `biquad`.
    band_responses: [FilterResponse; NUM_BANDS],
    correction: CorrectionStage,
    preamp_gain: f32,
    rg_gain: f32,
    limiter: Limiter,
    /// The active path's buffered frame, so the limiter acts per frame.
    frame: Vec<f32>,
    frame_len: usize,
    frame_pos: usize,
    /// Frames in a row whose narrowed output was exactly zero, not merely quiet.
    silent_frames: u64,
    silence_settle_frames: u64,
    bypass: bool,
}

impl<S: AudioSource> EqSource<S> {
    pub fn new(
        input: S,
        controls: DspControls,
        baked_rg: TrackReplayGain,
        fade: Arc<FadeShared>,
    ) -> Self {
        let channels = usize::from(input.channels().get());
        let sample_rate_hz = u64::from(input.sample_rate().get());
        #[expect(
            clippy::cast_precision_loss,
            reason = "audio sample rates are well below 2^24 Hz and convert to f32 exactly"
        )]
        let sample_rate = input.sample_rate().get() as f32;
        let banks = (0..channels)
            .map(|_| std::array::from_fn(|_| DirectForm1::<f64>::new(PASSTHROUGH)))
            .collect();
        let correction = CorrectionStage::new(channels, input.sample_rate());
        // Already per channel: dividing by the channel count runs the limiter that many times too
        // fast against the biquads.
        let frame_rate = sample_rate;
        // One behind the live values, so the first `next()` rebuilds and a gapless successor picks
        // up a ramp already armed on its deck.
        let last_generation = controls.eq.generation().wrapping_sub(1);
        let last_rg_generation = controls.rg.generation().wrapping_sub(1);
        let fade_generation_at_build = fade.generation();
        let last_fade_generation = fade_generation_at_build.wrapping_sub(1);
        Self {
            input,
            controls,
            baked_rg,
            last_generation,
            last_rg_generation,
            fade,
            last_fade_generation,
            fade_generation_at_build,
            fade_engaged: false,
            fade_ramping: false,
            fade_start: 1.0,
            fade_target: 1.0,
            fade_gain: 1.0,
            fade_pos: 0,
            fade_total: 0,
            frame_phase: 0,
            frame_fade_gain: 1.0,
            fade_end_on_complete: false,
            sample_rate,
            sample_rate_hz,
            channels: u64::try_from(channels).unwrap_or(1),
            banks,
            band_active: [false; NUM_BANDS],
            band_responses: [FilterResponse::FLAT; NUM_BANDS],
            correction,
            preamp_gain: 1.0,
            rg_gain: 1.0,
            limiter: Limiter::new(frame_rate),
            frame: vec![0.0; channels],
            frame_len: 0,
            frame_pos: 0,
            silent_frames: 0,
            silence_settle_frames: SILENCE_SETTLE_MS * sample_rate_hz / 1000,
            bypass: true,
        }
    }

    fn apply_fade_cmd(&mut self, generation: u64) {
        let Some(cmd) = self.fade.snapshot() else {
            self.fade_engaged = false;
            self.fade_ramping = false;
            self.fade_gain = 1.0;
            return;
        };
        let start = if generation == self.fade_generation_at_build { None } else { cmd.start };
        // No start ramps from wherever the source sits, so an aborted crossfade takes no step.
        self.fade_start = start.unwrap_or(self.fade_gain);
        self.fade_target = cmd.target;
        self.fade_gain = self.fade_start;
        self.fade_pos = 0;
        // Here rather than in the controller, which doesn't know each deck's rate.
        self.fade_total =
            cmd.ramp_ms.saturating_mul(self.sample_rate_hz).saturating_mul(self.channels) / 1000;
        self.fade_end_on_complete = cmd.end_on_complete;
        self.fade_engaged = true;
        self.fade_ramping = true;
    }

    fn fade_ended(&self) -> bool {
        self.fade_engaged && !self.fade_ramping && self.fade_end_on_complete
    }

    /// A compare rather than a modulo, per sample on the default bypass path.
    #[inline]
    fn advance_frame_phase(&mut self) {
        self.frame_phase += 1;
        if self.frame_phase >= self.banks.len() {
            self.frame_phase = 0;
        }
    }

    /// Only while `fade_engaged`.
    fn step_fade(&mut self, samples: u64) -> f32 {
        if !self.fade_ramping {
            return self.fade_gain;
        }
        let g =
            crossfade::ramp_gain(self.fade_start, self.fade_target, self.fade_pos, self.fade_total);
        self.fade_gain = g;
        self.fade_pos = self.fade_pos.saturating_add(samples);
        if self.fade_pos >= self.fade_total {
            self.fade_gain = self.fade_target;
            self.fade_ramping = false;
            // Landing on unity frees the bypass; landing on silence keeps holding.
            if !self.fade_end_on_complete && crossfade::is_unity_target(self.fade_target) {
                self.fade_engaged = false;
            }
        }
        g
    }

    /// `ReplayGain` and the correction apply whether or not the EQ is on, so they join the bands in
    /// deciding whether this is a passthrough.
    fn rebuild(&mut self) {
        let rg_gain = if self.controls.rg.enabled() {
            replaygain::compute_linear_gain(self.baked_rg, self.controls.rg.controls())
        } else {
            1.0
        };
        self.rg_gain = rg_gain;
        let bands_engaged = self.rebuild_bands();
        // After the bands, whose curve the correction's headroom counts.
        self.correction.update_headroom(&self.band_responses);
        self.bypass =
            !bands_engaged && replaygain::is_unity_gain(rg_gain) && !self.correction.engaged();
    }

    fn disable_bands(&mut self) {
        self.preamp_gain = 1.0;
        self.band_active = [false; NUM_BANDS];
        self.band_responses = [FilterResponse::FLAT; NUM_BANDS];
    }

    /// Returns whether a band or the preamp is engaged.
    fn rebuild_bands(&mut self) -> bool {
        if !self.controls.eq.enabled() {
            self.disable_bands();
            return false;
        }

        let preamp_db = self.controls.eq.preamp();
        self.preamp_gain = db_to_linear(preamp_db);
        let preamp_is_unity = preamp_db.abs() < PREAMP_EPSILON_DB;

        let Ok(fs) = Hertz::<f64>::from_hz(self.sample_rate) else {
            self.disable_bands();
            return false;
        };

        let mut any_active = false;
        for band in 0..NUM_BANDS {
            let gain = self.controls.eq.gain(band);

            // A band past this source's Nyquist yields no coefficients.
            let coeffs = if gain.abs() < GAIN_EPSILON_DB {
                None
            } else {
                Hertz::<f64>::from_hz(BAND_FREQS[band]).ok().and_then(|f0| {
                    let peaking = Type::PeakingEQ(f64::from(gain));
                    Coefficients::<f64>::from_params(peaking, fs, f0, BAND_Q).ok()
                })
            };

            let Some(c) = coeffs else {
                self.band_active[band] = false;
                self.band_responses[band] = FilterResponse::FLAT;
                continue;
            };
            let was_active = self.band_active[band];
            for bank in &mut self.banks {
                // A band coming on starts clean so stale state can't pop; an active one keeps its
                // state across the swap, which DF1 allows.
                if !was_active {
                    bank[band].reset_state();
                }
                bank[band].update_coefficients(c);
            }
            self.band_active[band] = true;
            self.band_responses[band] = self.correction.response(&c, f64::from(BAND_FREQS[band]));
            any_active = true;
        }

        any_active || !preamp_is_unity
    }

    /// Only ever on a frame boundary, and not as an optimization: a rebuild can flip `bypass` off,
    /// and [`Self::next_active`] framing from mid-frame would let `fade_ended` end the source on a
    /// half frame, flipping that deck's mixer channel parity for good.
    #[inline]
    fn poll_generations(&mut self) {
        let generation = self.controls.eq.generation();
        let rg_generation = self.controls.rg.generation();
        let reshaped = self.correction.poll(&self.controls.correction);
        if reshaped
            || generation != self.last_generation
            || rg_generation != self.last_rg_generation
        {
            self.rebuild();
            self.last_generation = generation;
            self.last_rg_generation = rg_generation;
        }
        let fade_generation = self.fade.generation();
        if fade_generation != self.last_fade_generation {
            self.apply_fade_cmd(fade_generation);
            self.last_fade_generation = fade_generation;
        }
    }

    /// Bit-identical, the phase still advancing so the poll and the ramp know where a frame begins.
    #[inline]
    fn next_bypass(&mut self) -> Option<Sample> {
        let s = self.input.next()?;
        self.advance_frame_phase();
        Some(s)
    }

    /// Still clamps before the ramp, raw decoder output exceeding full scale and the mixer summing
    /// unclamped. The ramp steps once a frame, or the stereo image shears.
    #[inline]
    fn next_bypass_faded(&mut self) -> Option<Sample> {
        if self.frame_phase == 0 {
            // Draining the deck is what `is_crossfading()` watches for.
            if self.fade_ended() {
                return None;
            }
            self.frame_fade_gain = self.step_fade(self.channels);
        }
        let s = self.input.next()?;
        self.advance_frame_phase();
        Some(s.clamp(-1.0, 1.0) * self.frame_fade_gain)
    }

    /// One whole frame through the filters, then the coupled limiter and the ramp across it; the
    /// rest drains through [`Iterator::next`]'s fast path.
    #[inline]
    fn next_active(&mut self) -> Option<Sample> {
        if self.fade_ended() {
            return None;
        }

        let mix = self.correction.frame_mix();
        // Zipping the channels' state with `frame` rather than indexing keeps the inner work free
        // of bounds checks.
        let channels = self.banks.iter_mut().zip(self.correction.channels_mut());
        let mut frame_len = 0;
        for ((bank, correction), slot) in channels.zip(self.frame.iter_mut()) {
            let Some(x) = self.input.next() else { break };
            // Widened after the gains, so a path with no filter engaged round-trips exactly.
            let mut out = correction.run(f64::from(x * self.preamp_gain * self.rg_gain), mix);
            for (filter, &active) in bank.iter_mut().zip(self.band_active.iter()) {
                if active {
                    out = filter.run(out);
                }
            }
            *slot = narrow_sample(out);
            frame_len += 1;
        }
        self.frame_len = frame_len;
        if frame_len == 0 {
            return None;
        }

        let mut peak = 0.0_f32;
        for &s in &self.frame[..self.frame_len] {
            peak = peak.max(s.abs());
        }
        self.settle_on_silence(peak);
        let gain = self.limiter.process(peak);
        let fade_g = if self.fade_engaged {
            self.step_fade(u64::try_from(frame_len).unwrap_or(1))
        } else {
            1.0
        };
        for s in &mut self.frame[..self.frame_len] {
            // The clamp catches the feed-forward overshoot; the ramp after it bounds two decks'
            // sum at unity.
            *s = (*s * gain).clamp(-1.0, 1.0) * fade_g;
        }

        self.frame_pos = 1;
        Some(self.frame[0])
    }

    /// The narrowed output reads zero long before the slowest filter's f64 state goes subnormal,
    /// so clearing every filter is inaudible and cuts the silence's subnormal arithmetic short.
    fn settle_on_silence(&mut self, peak: f32) {
        if peak > 0.0 {
            self.silent_frames = 0;
            return;
        }
        self.silent_frames = self.silent_frames.saturating_add(1);
        if self.silent_frames == self.silence_settle_frames {
            self.reset_bands();
            self.correction.reset_state();
        }
    }

    fn reset_bands(&mut self) {
        for bank in &mut self.banks {
            for filter in bank.iter_mut() {
                filter.reset_state();
            }
        }
    }
}

impl<S: AudioSource> Iterator for EqSource<S> {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        if self.frame_pos < self.frame_len {
            let s = self.frame[self.frame_pos];
            self.frame_pos += 1;
            return Some(s);
        }

        if self.frame_phase == 0 {
            self.poll_generations();
        }

        match (self.bypass, self.fade_engaged) {
            (true, false) => self.next_bypass(),
            (true, true) => self.next_bypass_faded(),
            (false, _) => self.next_active(),
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.input.size_hint()
    }
}

impl<S: AudioSource> AudioSource for EqSource<S> {
    #[inline]
    fn channels(&self) -> ChannelCount {
        self.input.channels()
    }

    #[inline]
    fn sample_rate(&self) -> SampleRate {
        self.input.sample_rate()
    }

    fn format(&self) -> SourceFormat {
        self.input.format()
    }

    /// The bypass flag rather than the settings, so an EQ with nothing to do reads as untouched;
    /// the fade is the transport's to report.
    fn dsp_engaged(&self) -> bool {
        !self.bypass || self.input.dsp_engaged()
    }

    #[inline]
    fn total_duration(&self) -> Option<Duration> {
        self.input.total_duration()
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        self.input.try_seek(pos)?;
        // Clears the filter and limiter state so the destination doesn't pop, and leaves the fade
        // fields alone, so a ramp in flight carries across rather than restarting from silence.
        self.reset_bands();
        self.correction.reset();
        self.limiter.reset();
        // The rest of the frame in hand drains and `frame_phase` stays: dropping owed samples
        // leaves every later frame a channel out of step.
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.input.is_ready()
    }

    fn release_claims(&mut self) {
        self.input.release_claims();
    }
}

#[cfg(test)]
#[path = "tests/equalizer_tests.rs"]
mod tests;
