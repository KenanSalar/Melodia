//! Graphic-equalizer DSP.
//!
//! Rodio 0.22 ships only `low_pass` / `high_pass` BLT filters — no
//! peaking/parametric filters — so a real graphic EQ can't be built from its
//! primitives. This module provides:
//!
//! - [`EqShared`]: lock-free state (per-band gains, a preamp, an enabled flag)
//!   read on the audio thread. The library/UI layer mutates it; the audio thread
//!   polls a generation counter and only recomputes coefficients on change.
//! - [`EqSource`]: a custom Rodio [`Source`] wrapping a decoder. Each sample
//!   runs through a preamp gain then a cascade of ten `Type::PeakingEQ` biquads
//!   — one [`DirectForm1`] per band, **per channel** (rodio's own `BltFilter`
//!   keeps a single filter state across interleaved channels, which is part of
//!   why it's "probably buggy"; we keep independent per-channel state). A
//!   coupled soft-knee peak [`Limiter`] then catches any residual peaks so heavy
//!   boosts compress instead of hard-clipping.
//!
//! `DirectForm1` is used (not `DirectForm2Transposed`) because its delay line
//! holds past inputs/outputs that stay valid when coefficients change at
//! runtime, so live slider drags swap coefficients without injecting transients.
//!
//! Clip protection is two-stage and standard for graphic EQs: a **preamp**
//! (transparent linear headroom the user controls) plus a **limiter** (an
//! automatic safety net). Both run only in the active path — when the EQ is
//! disabled, or every band is flat *and* the preamp is 0 dB, [`EqSource`] is a
//! transparent per-sample passthrough (**bit-identical, zero added DSP cost**).
//!
//! [`EqSource`] **also applies `ReplayGain`** (see [`super::replaygain`]): a
//! per-track linear pre-gain, baked in at construction, applied *before* the EQ
//! bands so the same limiter guards a `ReplayGain` boost for free. `ReplayGain` is
//! independent of the EQ toggle — it applies whether or not the EQ is enabled,
//! and its own master state ([`ReplayGainShared`]) is polled via a second
//! generation counter alongside the EQ's. When `ReplayGain` is off (or the track
//! has no tags and the EQ is also inert) the passthrough fast-path still holds.
//!
//! Finally, [`EqSource`] carries the **crossfade ramp** (see [`super::crossfade`]):
//! a deck-scoped [`FadeShared`] cell polled through a third generation counter,
//! applied *after* the limiter's clamp so two overlapping decks can never sum
//! past unity in rodio's (unclamped) mixer.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use biquad::{Biquad, Coefficients, DirectForm1, Hertz, Type};
use rodio::source::SeekError;
use rodio::{ChannelCount, Sample, SampleRate, Source};

use super::crossfade::{self, FadeShared};
use super::dsp::{Generation, db_to_linear, linear_to_db};
use super::replaygain::{self, ReplayGainShared, TrackReplayGain};

/// Number of equalizer bands.
pub const NUM_BANDS: usize = 10;

/// ISO octave centre frequencies (Hz) for the ten bands, low → high.
pub const BAND_FREQS: [f32; NUM_BANDS] = [
    31.0, 62.0, 125.0, 250.0, 500.0, 1_000.0, 2_000.0, 4_000.0, 8_000.0, 16_000.0,
];

/// Per-band gain range, in decibels.
pub const MIN_GAIN_DB: f32 = -12.0;
pub const MAX_GAIN_DB: f32 = 12.0;

/// Preamp (master input gain) range, in decibels. Asymmetric: generous cut for
/// headroom after boosting, a little boost for makeup gain. 0 dB is unity.
pub const MIN_PREAMP_DB: f32 = -12.0;
pub const MAX_PREAMP_DB: f32 = 6.0;

/// Quality factor for the peaking filters. ~1.4 gives musically sensible
/// overlap for octave-spaced bands without ringing.
const BAND_Q: f32 = 1.41;

/// Bands within this many dB of unity are treated as off (skipped in the
/// filter chain) so a near-flat EQ stays close to free.
const GAIN_EPSILON_DB: f32 = 0.05;

/// A preamp within this many dB of unity is treated as 0 dB (no gain stage), so
/// a flat EQ with no preamp can still take the bit-identical bypass path.
const PREAMP_EPSILON_DB: f32 = 0.01;

// Safety limiter (feed-forward, soft-knee), applied to the EQ output. These are
// rodio's general-purpose `LimitSettings::default()` values (-1 dBFS threshold,
// 4 dB knee, 5 ms attack, 100 ms release): transparent until near full scale,
// then it pins peaks at the threshold. It only runs in the active path, so
// EQ-off audio is never touched.
const LIMITER_THRESHOLD_DB: f32 = -1.0;
const LIMITER_KNEE_DB: f32 = 4.0;
const LIMITER_ATTACK_S: f32 = 0.005;
const LIMITER_RELEASE_S: f32 = 0.100;

/// A named gain curve. `PRESETS` order is load-bearing: the Slint preset
/// dropdown lists the same names as inline `@tr` literals in this order, and
/// the UI maps the dropdown index straight into this slice.
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

/// Number of built-in presets. The UI's synthetic "Custom" dropdown entry
/// sits at this index (one past the last built-in).
pub const PRESET_COUNT: usize = PRESETS.len();

/// Default preset name persisted on first launch.
pub const DEFAULT_PRESET: &str = "Flat";

/// Sentinel preset name persisted for a hand-tuned (non-built-in) curve. It is
/// deliberately absent from [`PRESETS`], so [`preset_index`] returns `None` for
/// it and the UI maps it to the synthetic "Custom" dropdown slot at
/// [`PRESET_COUNT`].
pub const CUSTOM_PRESET: &str = "Custom";

/// Clamp a single band gain into the supported range.
#[must_use]
pub fn clamp_gain(db: f32) -> f32 {
    if db.is_nan() {
        0.0
    } else {
        db.clamp(MIN_GAIN_DB, MAX_GAIN_DB)
    }
}

/// Clamp the preamp into the supported range.
#[must_use]
pub fn clamp_preamp(db: f32) -> f32 {
    if db.is_nan() {
        0.0
    } else {
        db.clamp(MIN_PREAMP_DB, MAX_PREAMP_DB)
    }
}

/// Coerce an arbitrary (possibly hand-edited / wrong-length) gain list into a
/// validated `[f32; NUM_BANDS]`: pad missing bands with 0, drop extras, clamp.
#[must_use]
pub fn normalize_gains(gains: &[f32]) -> [f32; NUM_BANDS] {
    std::array::from_fn(|i| clamp_gain(gains.get(i).copied().unwrap_or(0.0)))
}

/// Index of a named preset, if it matches one of [`PRESETS`].
#[must_use]
pub fn preset_index(name: &str) -> Option<usize> {
    PRESETS.iter().position(|p| p.name == name)
}

/// Lock-free equalizer state shared between the control layer (writer) and the
/// audio thread (reader). Gains / preamp are stored as `f32` bit patterns in
/// atomics; every mutation bumps the [`Generation`] so [`EqSource`] knows to
/// recompute.
pub struct EqShared {
    enabled: AtomicBool,
    gains_bits: [AtomicU32; NUM_BANDS],
    preamp_bits: AtomicU32,
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
            gains_bits: std::array::from_fn(|i| AtomicU32::new(norm[i].to_bits())),
            preamp_bits: AtomicU32::new(0.0_f32.to_bits()),
            generation: Generation::new(),
        })
    }

    /// Publish a state change — see [`Generation::bump`].
    fn bump(&self) {
        self.generation.bump();
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
        self.bump();
    }

    pub fn set_gain(&self, index: usize, db: f32) {
        if let Some(cell) = self.gains_bits.get(index) {
            cell.store(clamp_gain(db).to_bits(), Ordering::Relaxed);
            self.bump();
        }
    }

    pub fn set_all_gains(&self, gains: &[f32]) {
        let norm = normalize_gains(gains);
        for (cell, g) in self.gains_bits.iter().zip(norm) {
            cell.store(g.to_bits(), Ordering::Relaxed);
        }
        self.bump();
    }

    pub fn set_preamp(&self, db: f32) {
        self.preamp_bits.store(clamp_preamp(db).to_bits(), Ordering::Relaxed);
        self.bump();
    }

    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn gain(&self, index: usize) -> f32 {
        self.gains_bits
            .get(index)
            .map_or(0.0, |c| f32::from_bits(c.load(Ordering::Relaxed)))
    }

    #[must_use]
    pub fn gains(&self) -> [f32; NUM_BANDS] {
        std::array::from_fn(|i| f32::from_bits(self.gains_bits[i].load(Ordering::Relaxed)))
    }

    #[must_use]
    pub fn preamp(&self) -> f32 {
        f32::from_bits(self.preamp_bits.load(Ordering::Relaxed))
    }

    #[must_use]
    fn generation(&self) -> u64 {
        self.generation.get()
    }
}

/// Transparent passthrough coefficients (`y[n] = x[n]`). Only used to
/// construct the filter banks; [`EqSource::rebuild`] overwrites a band's
/// coefficients before that band is ever run.
fn identity_coeffs() -> Coefficients<f32> {
    Coefficients { a1: 0.0, a2: 0.0, b0: 1.0, b1: 0.0, b2: 0.0 }
}

/// One-pole smoothing coefficient for a time constant at a given update rate:
/// the smoothed value moves `(1 - coeff)` of the way to its target each update.
fn smoothing_coeff(time_s: f32, rate: f32) -> f32 {
    if rate > 0.0 && time_s > 0.0 {
        (-1.0 / (time_s * rate)).exp()
    } else {
        0.0
    }
}

/// Soft-knee feed-forward peak limiter. Computes one gain per frame from the
/// frame's peak magnitude and smooths it with separate attack/release times, so
/// boosts are turned down cleanly (no added harmonics) rather than hard-clipped.
/// A single coupled gain is applied across all channels to preserve stereo
/// imaging — matching rodio's limiter design (Giannoulis et al. 2012).
struct Limiter {
    /// Current smoothed linear gain (≤ 1.0); starts at unity.
    gain: f32,
    attack_coeff: f32,
    release_coeff: f32,
    /// Linear magnitude at the knee's lower edge (`THRESHOLD − KNEE/2` dB). A
    /// frame peak at or below this needs no reduction, so [`Self::target_gain`]
    /// can return unity without evaluating `log10`. Precomputed once.
    knee_low_linear: f32,
}

impl Limiter {
    fn new(frame_rate: f32) -> Self {
        Self {
            gain: 1.0,
            attack_coeff: smoothing_coeff(LIMITER_ATTACK_S, frame_rate),
            release_coeff: smoothing_coeff(LIMITER_RELEASE_S, frame_rate),
            knee_low_linear: db_to_linear(LIMITER_THRESHOLD_DB - LIMITER_KNEE_DB / 2.0),
        }
    }

    fn reset(&mut self) {
        self.gain = 1.0;
    }

    /// Target gain (linear, ≤ 1.0) for a peak magnitude — the soft-knee
    /// limiter curve with an infinite ratio above the knee.
    fn target_gain(&self, peak: f32) -> f32 {
        // Below the knee's lower edge the curve is flat at unity, so quiet
        // frames (the overwhelmingly common case) skip the `log10` entirely.
        // `log10` is monotonic, so `peak <= knee_low_linear` is the exact
        // linear-domain equivalent of the old `over <= -half_knee` dB test; it
        // also subsumes the previous `peak <= 0.0` guard, since `peak` is a
        // magnitude (≥ 0) and `knee_low_linear > 0`.
        if peak <= self.knee_low_linear {
            return 1.0;
        }
        let peak_db = linear_to_db(peak);
        let over = peak_db - LIMITER_THRESHOLD_DB;
        let half_knee = LIMITER_KNEE_DB / 2.0;
        let reduction_db = if over >= half_knee {
            -over
        } else {
            // Within the knee (the guard above already excluded `over <=
            // -half_knee`, so this branch is `-half_knee < over < half_knee`).
            let k = over + half_knee;
            -(k * k) / (2.0 * LIMITER_KNEE_DB)
        };
        db_to_linear(reduction_db)
    }

    /// Advance the smoothed gain toward this frame's target and return it.
    /// Rising signal level → fast attack (gain falls quickly); falling level →
    /// slow release (gain recovers gently).
    fn process(&mut self, peak: f32) -> f32 {
        let target = self.target_gain(peak);
        let coeff = if target < self.gain { self.attack_coeff } else { self.release_coeff };
        self.gain = coeff.mul_add(self.gain, (1.0 - coeff) * target);
        self.gain
    }
}

/// A Rodio source that applies the shared graphic EQ **and `ReplayGain`** to its
/// inner decoder.
///
/// One [`EqSource`] wraps each decoded track; both the playing track and the
/// gapless-preloaded one share the same [`EqShared`] / [`ReplayGainShared`], so
/// a live master change applies to both. The **per-track** `ReplayGain` values
/// ([`baked_rg`](Self::baked_rg)) are baked in at construction, however — the
/// gapless-preloaded next track has different tags than the playing one, so its
/// gain must travel with its own source, not on a shared cell. Per-source state
/// (the filter banks + a one-frame buffer) is a few hundred bytes — no caches,
/// negligible memory.
#[allow(
    clippy::struct_excessive_bools,
    reason = "audio-thread hot state: each flag gates a distinct branch in `next()` and is read per sample; packing them into bitflags would add masking to the hot path"
)]
pub struct EqSource<S> {
    input: S,
    shared: Arc<EqShared>,
    /// Shared `ReplayGain` master state (enabled / mode / preamp / prevent-clip).
    rg_shared: Arc<ReplayGainShared>,
    /// This track's baked `ReplayGain` tag values. Combined with `rg_shared`'s
    /// live mode/preamp at `rebuild` time to produce `rg_gain`.
    baked_rg: TrackReplayGain,
    /// Last generation this source applied; mismatch triggers `rebuild`.
    last_generation: u64,
    /// Last `ReplayGain` generation this source applied — polled alongside
    /// `last_generation` so a live RG-only change also triggers `rebuild`.
    last_rg_generation: u64,
    /// Deck-scoped crossfade ramp cell. Shared with whatever other source this
    /// deck plays (a gapless-appended successor), but **not** with the other
    /// deck — a fade armed here moves only this voice.
    fade: Arc<FadeShared>,
    /// Last fade generation this source applied; mismatch re-arms the ramp.
    last_fade_generation: u64,
    /// Whether the fade stage applies at all. `false` ⇒ gain is exactly 1.0 and
    /// the bit-identical bypass path stays available.
    fade_engaged: bool,
    /// Whether `fade_pos` is still advancing (vs. holding at `fade_target`).
    fade_ramping: bool,
    fade_start: f32,
    fade_target: f32,
    /// Current ramp gain. Also the implicit start point when a ramp is armed
    /// with [`FadeCmd::start`](super::crossfade::FadeCmd::start) = `None`.
    fade_gain: f32,
    /// Ramp progress and length, in **interleaved** samples of this source.
    fade_pos: u64,
    fade_total: u64,
    /// Position within the current interleaved frame, tracked by the bypass
    /// path (the active path buffers a whole frame instead, so it starts and
    /// ends on a boundary by construction and leaves this at `0`). Advanced on
    /// every bypass sample, fade or no fade. Two things read it: the ramp, which
    /// must step once per *frame* so both channels share a gain, and the
    /// generation poll in `next`, which only fires at phase `0` so a rebuild can
    /// never hand the active path a mid-frame start.
    frame_phase: usize,
    /// The gain held for every sample of the frame the bypass path is emitting.
    frame_fade_gain: f32,
    /// Fade-out: end the source (return `None`) once the ramp lands.
    fade_end_on_complete: bool,
    /// This source's sample rate (Hz), captured once — constant per decoded
    /// file, so coefficients computed from it stay correct for the source's life.
    sample_rate: f32,
    /// Integer sample rate + channel count, used to convert a ramp's media
    /// milliseconds into this source's interleaved sample count.
    sample_rate_hz: u64,
    channels: u64,
    /// `[channel][band]` filter state. Fixed size; allocated once. Its length
    /// is the channel count, so the active path iterates it directly.
    banks: Vec<[DirectForm1<f32>; NUM_BANDS]>,
    /// Per-band on/off — a band is inactive at unity gain or outside Nyquist.
    band_active: [bool; NUM_BANDS],
    /// Linear preamp gain applied before the bands in the active path.
    preamp_gain: f32,
    /// Linear `ReplayGain` factor applied before the bands (after the preamp) in
    /// the active path. 1.0 when `ReplayGain` is disabled or the track is untagged.
    rg_gain: f32,
    /// Coupled safety limiter applied to the EQ output.
    limiter: Limiter,
    /// One processed interleaved frame (`channels` samples) awaiting emit, used
    /// only by the active path so the limiter can act per-frame (coupled).
    frame: Vec<f32>,
    frame_len: usize,
    frame_pos: usize,
    /// Fast path: when true, `next` returns the inner sample untouched.
    bypass: bool,
}

impl<S: Source> EqSource<S> {
    pub fn new(
        input: S,
        shared: Arc<EqShared>,
        rg_shared: Arc<ReplayGainShared>,
        baked_rg: TrackReplayGain,
        fade: Arc<FadeShared>,
    ) -> Self {
        let channels = usize::from(input.channels().get());
        let sample_rate_hz = u64::from(input.sample_rate().get());
        #[allow(
            clippy::cast_precision_loss,
            reason = "audio sample rates are well below 2^24 Hz and convert to f32 exactly"
        )]
        let sample_rate = input.sample_rate().get() as f32;
        let banks = (0..channels)
            .map(|_| std::array::from_fn(|_| DirectForm1::<f32>::new(identity_coeffs())))
            .collect();
        // The limiter updates its gain once per interleaved frame. A frame is
        // one sample per channel, so frames elapse at the per-channel sample
        // rate — which is exactly what rodio's `sample_rate()` reports (samples
        // per second per channel). The limiter's attack/release time constants
        // are therefore relative to `sample_rate` directly, NOT divided by the
        // channel count (the biquad path uses the same value as its per-channel
        // `fs`, so dividing here would desync the two by the channel count).
        let frame_rate = sample_rate;
        // Seed the cached generations to something other than the live values so
        // the first `next()` rebuilds from the current shared state. The fade
        // cell is seeded the same way, so a source appended to a deck with a
        // ramp already armed (a gapless successor on a fading deck) picks it up.
        let last_generation = shared.generation().wrapping_sub(1);
        let last_rg_generation = rg_shared.generation().wrapping_sub(1);
        let last_fade_generation = fade.generation().wrapping_sub(1);
        Self {
            input,
            shared,
            rg_shared,
            baked_rg,
            last_generation,
            last_rg_generation,
            fade,
            last_fade_generation,
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
            preamp_gain: 1.0,
            rg_gain: 1.0,
            limiter: Limiter::new(frame_rate),
            frame: vec![0.0; channels],
            frame_len: 0,
            frame_pos: 0,
            bypass: true,
        }
    }

    /// Adopt the ramp currently armed on this source's deck. Called from `next`
    /// only when the fade generation advances.
    fn apply_fade_cmd(&mut self) {
        let Some(cmd) = self.fade.snapshot() else {
            // Idle: drop back to transparency and re-enable the bypass path.
            self.fade_engaged = false;
            self.fade_ramping = false;
            self.fade_gain = 1.0;
            return;
        };
        // `None` means "ramp from wherever this source currently sits" — that's
        // how a playing track fades out from unity, and how an aborted
        // crossfade recovers a partially faded-in track without a step.
        self.fade_start = cmd.start.unwrap_or(self.fade_gain);
        self.fade_target = cmd.target;
        self.fade_gain = self.fade_start;
        self.fade_pos = 0;
        // Media milliseconds → this source's interleaved sample count. The
        // controller can't precompute this: the two decks may hold tracks at
        // different sample rates, and it doesn't know the outgoing source's.
        self.fade_total = cmd
            .ramp_ms
            .saturating_mul(self.sample_rate_hz)
            .saturating_mul(self.channels)
            / 1000;
        self.fade_end_on_complete = cmd.end_on_complete;
        self.fade_engaged = true;
        self.fade_ramping = true;
    }

    /// The armed fade-out has landed and this source should end.
    fn fade_ended(&self) -> bool {
        self.fade_engaged && !self.fade_ramping && self.fade_end_on_complete
    }

    /// Advance the interleave phase by one sample, wrapping at the frame width.
    ///
    /// A compare, not a modulo: this runs per sample on the bypass path, which
    /// is what a flat EQ with unity `ReplayGain` takes — the default. `banks` is
    /// sized from the decoder's `NonZero` channel count, so its length is always
    /// at least 1 and the wrap can't spin (which also means a mono source sits at
    /// phase `0` permanently, exactly as if there were no framing at all).
    #[inline]
    fn advance_frame_phase(&mut self) {
        self.frame_phase += 1;
        if self.frame_phase >= self.banks.len() {
            self.frame_phase = 0;
        }
    }

    /// Gain for the next `samples` interleaved samples, advancing the ramp.
    /// Only called when `fade_engaged`.
    fn step_fade(&mut self, samples: u64) -> f32 {
        if !self.fade_ramping {
            return self.fade_gain;
        }
        let g = crossfade::ramp_gain(self.fade_start, self.fade_target, self.fade_pos, self.fade_total);
        self.fade_gain = g;
        self.fade_pos = self.fade_pos.saturating_add(samples);
        if self.fade_pos >= self.fade_total {
            self.fade_gain = self.fade_target;
            self.fade_ramping = false;
            // A ramp that lands back on unity (fade-in, crossfade abort) is
            // done influencing the signal — disengage so `bypass` can resume.
            // A ramp that lands on silence (pause fade) must stay engaged and
            // keep holding its target, unless it also ends the source.
            if !self.fade_end_on_complete && crossfade::is_unity_target(self.fade_target) {
                self.fade_engaged = false;
            }
        }
        g
    }

    /// Take the EQ out of the signal path entirely — no preamp, no bands. Used
    /// by both of [`Self::rebuild`]'s early returns (the EQ is off; the sample
    /// rate is pathological). `ReplayGain` is a plain scalar and applies either
    /// way, so it alone decides whether this is a true passthrough.
    fn disable_bands(&mut self, rg_is_unity: bool) {
        self.preamp_gain = 1.0;
        self.band_active = [false; NUM_BANDS];
        self.bypass = rg_is_unity;
    }

    /// Recompute per-band coefficients and the preamp gain from the shared
    /// state. Called only when the generation counter advances (toggle, slider
    /// drag, preset, reset, preamp change).
    fn rebuild(&mut self) {
        // ReplayGain is independent of the EQ toggle, so compute its gain FIRST —
        // before any EQ-disabled / pathological early return could skip it.
        let rg_gain = if self.rg_shared.enabled() {
            replaygain::compute_linear_gain(
                self.baked_rg,
                self.rg_shared.mode(),
                self.rg_shared.preamp(),
                self.rg_shared.prevent_clipping(),
            )
        } else {
            1.0
        };
        self.rg_gain = rg_gain;
        let rg_is_unity = replaygain::is_unity_gain(rg_gain);

        if !self.shared.enabled() {
            // EQ off. ReplayGain may still apply, so this is only a passthrough
            // when its gain is unity too.
            self.disable_bands(rg_is_unity);
            return;
        }

        let preamp_db = self.shared.preamp();
        self.preamp_gain = db_to_linear(preamp_db);
        let preamp_is_unity = preamp_db.abs() < PREAMP_EPSILON_DB;

        let Ok(fs) = Hertz::<f32>::from_hz(self.sample_rate) else {
            // Pathological sample rate — can't build the peaking filters. The
            // ReplayGain factor is just a scalar, so let it still apply.
            self.disable_bands(rg_is_unity);
            return;
        };

        let mut any_active = false;
        for band in 0..NUM_BANDS {
            let gain = self.shared.gain(band);

            // Skip unity-gain bands and any band whose centre exceeds this
            // source's Nyquist limit (low-sample-rate files).
            let coeffs = if gain.abs() < GAIN_EPSILON_DB {
                None
            } else {
                Hertz::<f32>::from_hz(BAND_FREQS[band]).ok().and_then(|f0| {
                    Coefficients::<f32>::from_params(Type::PeakingEQ(gain), fs, f0, BAND_Q).ok()
                })
            };

            match coeffs {
                Some(c) => {
                    let was_active = self.band_active[band];
                    for bank in &mut self.banks {
                        // inactive→active: start from a clean delay line so
                        // stale state can't pop. Already-active: keep state
                        // across the coefficient swap (DirectForm1 is safe for
                        // live coefficient changes).
                        if !was_active {
                            bank[band].reset_state();
                        }
                        bank[band].update_coefficients(c);
                    }
                    self.band_active[band] = true;
                    any_active = true;
                }
                None => self.band_active[band] = false,
            }
        }

        // Active when any band filters, OR the preamp is non-unity, OR
        // ReplayGain applies a non-unity factor — each needs the gain + limiter
        // stage. Only a flat EQ with 0 dB preamp and unity ReplayGain bypasses.
        self.bypass = !any_active && preamp_is_unity && rg_is_unity;
    }

    /// Pick up state changes — the EQ, `ReplayGain` and crossfade generations.
    ///
    /// **Only ever called on a true frame boundary**, and that gate is
    /// load-bearing, not an optimization. A `rebuild` can flip `bypass` off (the
    /// EQ is switched on mid-track), and [`Self::next_active`] then pulls a whole
    /// `channels`-wide frame starting from wherever the source sits. Let that
    /// happen part-way through a frame and every frame it forms is offset from a
    /// real one — harmless for the audio it emits, but `fade_ended` would then end
    /// the source off-boundary, on a *half* frame, flipping that deck's channel
    /// parity in the mixer for everything appended to it afterwards. Polling only
    /// at phase 0 means the active path can never be *entered* mid-frame; it then
    /// consumes whole frames and never advances the phase, so it still polls every
    /// frame.
    ///
    /// The bypass path buffers no frame, so the same gate is what takes its
    /// `Acquire` loads down from per-sample to per-frame. The cost is that a
    /// rebuild or an armed ramp lands up to `channels - 1` samples late.
    #[inline]
    fn poll_generations(&mut self) {
        let generation = self.shared.generation();
        let rg_generation = self.rg_shared.generation();
        if generation != self.last_generation || rg_generation != self.last_rg_generation {
            self.rebuild();
            self.last_generation = generation;
            self.last_rg_generation = rg_generation;
        }
        let fade_generation = self.fade.generation();
        if fade_generation != self.last_fade_generation {
            self.apply_fade_cmd();
            self.last_fade_generation = fade_generation;
        }
    }

    /// Pure per-sample passthrough (bit-identical, no framing). The interleave
    /// phase still advances, so the generation poll and the ramp both know where
    /// a frame begins.
    #[inline]
    fn next_bypass(&mut self) -> Option<Sample> {
        let s = self.input.next()?;
        self.advance_frame_phase();
        Some(s)
    }

    /// Bypass + fade: the EQ / `ReplayGain` stages are inert, so skip the frame
    /// machinery, but still clamp before applying the ramp. Raw decoder output can
    /// exceed full scale, and rodio's mixer sums its voices without clamping — two
    /// unclamped decks overlapping would clip.
    ///
    /// The ramp advances once per *frame* (one sample per channel is one time
    /// step), so both channels of a frame share a gain — advancing per sample
    /// would shear the stereo image across the fade.
    #[inline]
    fn next_bypass_faded(&mut self) -> Option<Sample> {
        if self.frame_phase == 0 {
            // An armed fade-out has run to silence: end the source. That drains
            // its deck, which is the signal `is_crossfading()` watches for. Only
            // ever at a frame boundary, so a partial frame is never emitted.
            if self.fade_ended() {
                return None;
            }
            self.frame_fade_gain = self.step_fade(self.channels);
        }
        let s = self.input.next()?;
        self.advance_frame_phase();
        Some(s.clamp(-1.0, 1.0) * self.frame_fade_gain)
    }

    /// Pull one interleaved frame, preamp + EQ each channel, then apply the
    /// coupled limiter and the ramp across the whole frame. Buffers the frame and
    /// returns its first sample; the rest drain through [`Iterator::next`]'s
    /// fast path.
    ///
    /// Only ever entered at a frame boundary (see [`Self::poll_generations`]), so
    /// ending the source here cannot cut a frame in half.
    #[inline]
    fn next_active(&mut self) -> Option<Sample> {
        if self.fade_ended() {
            return None;
        }

        // Zipping `banks`/`frame` (both `channels` long) instead of indexing them
        // keeps the per-sample inner work free of bounds checks.
        let mut frame_len = 0;
        for (bank, slot) in self.banks.iter_mut().zip(self.frame.iter_mut()) {
            let Some(x) = self.input.next() else { break };
            let mut out = x * self.preamp_gain * self.rg_gain;
            for (filter, &active) in bank.iter_mut().zip(self.band_active.iter()) {
                if active {
                    out = filter.run(out);
                }
            }
            *slot = out;
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
        let gain = self.limiter.process(peak);
        // One ramp step per frame — a frame is one sample per channel, i.e. one
        // time step — so the whole frame shares a gain and the stereo image
        // can't shear mid-fade.
        let fade_g = if self.fade_engaged {
            self.step_fade(u64::try_from(frame_len).unwrap_or(1))
        } else {
            1.0
        };
        for s in &mut self.frame[..self.frame_len] {
            // Final hard clamp catches the brief feed-forward overshoot before
            // the gain settles; the limiter keeps it within a fraction of a dB.
            // The crossfade ramp multiplies *after* the clamp, which is what
            // bounds the sum of two overlapping decks at unity.
            *s = (*s * gain).clamp(-1.0, 1.0) * fade_g;
        }

        self.frame_pos = 1;
        Some(self.frame[0])
    }
}

impl<S: Source> Iterator for EqSource<S> {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        // Emit any remaining samples from the current processed frame first.
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

impl<S: Source> Source for EqSource<S> {
    #[inline]
    fn current_span_len(&self) -> Option<usize> {
        self.input.current_span_len()
    }

    #[inline]
    fn channels(&self) -> ChannelCount {
        self.input.channels()
    }

    #[inline]
    fn sample_rate(&self) -> SampleRate {
        self.input.sample_rate()
    }

    #[inline]
    fn total_duration(&self) -> Option<Duration> {
        self.input.total_duration()
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        self.input.try_seek(pos)?;
        // Clear every delay line + the limiter envelope so the seek destination
        // doesn't pop from pre-seek state, and drop any buffered frame.
        //
        // Deliberately does NOT touch the fade fields. `set_speed` re-anchors
        // the active deck with a `try_seek` to its own current position, and a
        // crossfade abort arms a ramp and *then* seeks — resetting `fade_pos`
        // here would restart a fade-in from silence in both cases.
        for bank in &mut self.banks {
            for filter in bank.iter_mut() {
                filter.reset_state();
            }
        }
        self.limiter.reset();
        self.frame_len = 0;
        self.frame_pos = 0;
        // The decoder lands on a frame boundary, so the bypass path's interleave
        // phase restarts with it. The ramp's own progress is untouched.
        self.frame_phase = 0;
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/equalizer_tests.rs"]
mod tests;
