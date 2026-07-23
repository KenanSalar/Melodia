//! `ReplayGain` (loudness normalization) state and gain math.
//!
//! `ReplayGain` tags record how loud a track (or its album) is relative to a
//! reference level. Applying the stored gain at playback normalizes perceived
//! loudness across a library so tracks don't jump in volume. Melodia already
//! parses the tags at scan time ([`crate::media::metadata`]) into the `tracks`
//! table; this module carries them into the audio path.
//!
//! Two pieces, mirroring the equalizer:
//!
//! - [`TrackReplayGain`]: the four per-track values (track/album gain + peak),
//!   baked into each [`EqSource`](super::equalizer::EqSource) at construction so
//!   the gapless-preloaded *next* track carries its own gain, not the playing
//!   one's. `Default` (all `None`) means "no `ReplayGain` data" → unity gain, so
//!   untagged tracks play unchanged.
//! - [`ReplayGainShared`]: lock-free master controls (enabled / mode / preamp /
//!   prevent-clipping) read on the audio thread. The library/UI layer mutates
//!   it; the audio thread polls a generation counter and recomputes only on
//!   change — the same pattern as [`EqShared`](super::equalizer::EqShared).
//!
//! The gain itself is applied inside `EqSource` (before the EQ bands), so the
//! equalizer's existing soft-knee limiter guards any boost for free. `ReplayGain`
//! is otherwise independent of the EQ toggle — it applies whether or not the EQ
//! is enabled.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use super::dsp::{Generation, db_to_linear};

/// Preamp (extra gain applied on top of the tag value) range, in decibels.
/// Symmetric — unlike the EQ preamp — because the `ReplayGain` preamp is a
/// listener preference for overall loudness, cutting or boosting equally.
/// 0 dB is unity.
pub const RG_MIN_PREAMP_DB: f32 = -15.0;
pub const RG_MAX_PREAMP_DB: f32 = 15.0;

/// Default `ReplayGain` preamp on first launch (unity).
pub const RG_DEFAULT_PREAMP_DB: f32 = 0.0;

/// Default mode name persisted on first launch. Album mode preserves within-album
/// level relationships, which suits gapless full-album listening.
pub const DEFAULT_MODE: &str = "album";

/// A computed linear gain within this much of unity is treated as no gain, so a
/// track with ~0 dB effective `ReplayGain` can still take the bit-identical bypass.
const RG_UNITY_EPSILON: f32 = 1e-4;

/// Clamp a `ReplayGain` preamp value into the supported range.
#[must_use]
pub fn clamp_rg_preamp(db: f32) -> f32 {
    if db.is_nan() { 0.0 } else { db.clamp(RG_MIN_PREAMP_DB, RG_MAX_PREAMP_DB) }
}

/// Which gain pair `ReplayGain` applies. `enabled` on [`ReplayGainShared`] gates
/// whether *any* gain applies, so there is deliberately no `Off` variant here
/// (mirroring the EQ, whose `enabled` flag is the only on/off gate).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum RgMode {
    /// Per-track gain — every track normalized to the reference level
    /// individually (best for shuffle).
    Track,
    /// Per-album gain — album-relative levels preserved (best for gapless
    /// full-album listening). The default.
    #[default]
    Album,
}

impl RgMode {
    /// Stable index used both for the atomic encoding and the UI dropdown order
    /// (0 = Track, 1 = Album).
    #[must_use]
    pub fn to_u8(self) -> u8 {
        match self {
            RgMode::Track => 0,
            RgMode::Album => 1,
        }
    }

    /// Decode the atomic / dropdown index; unknown values fall back to `Album`.
    #[must_use]
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => RgMode::Track,
            _ => RgMode::Album,
        }
    }

    /// Parse the persisted settings string; unknown values fall back to `Album`.
    #[must_use]
    pub fn from_settings_str(s: &str) -> Self {
        match s {
            "track" => RgMode::Track,
            _ => RgMode::Album,
        }
    }

    /// The lowercase token persisted in `settings.json`.
    #[must_use]
    pub fn as_settings_str(self) -> &'static str {
        match self {
            RgMode::Track => "track",
            RgMode::Album => "album",
        }
    }
}

/// The four `ReplayGain` values for one track, in playback-ready `f32`. `gain`
/// fields are decibels (as stored in the tag); `peak` fields are linear sample
/// peaks (1.0 = full scale). Baked into each [`EqSource`] at build time. Cheap
/// to copy (four `Option<f32>`), so it threads through the play actions by value.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct TrackReplayGain {
    pub track_gain: Option<f32>,
    pub track_peak: Option<f32>,
    pub album_gain: Option<f32>,
    pub album_peak: Option<f32>,
}

/// Compute the linear gain factor to apply for one track under the given master
/// settings. Returns `1.0` (unity) when no gain data is present, so untagged
/// tracks are unchanged. The pure core of the `ReplayGain` DSP — unit-tested
/// directly; the caller (`EqSource::rebuild`) only calls it when RG is enabled.
#[must_use]
pub fn compute_linear_gain(
    baked: TrackReplayGain,
    mode: RgMode,
    preamp_db: f32,
    prevent_clipping: bool,
) -> f32 {
    // Pick the (gain, peak) pair for the active mode, falling back to the other
    // pair when the preferred gain is absent so the peak always matches the gain.
    let pair = match mode {
        RgMode::Album => baked
            .album_gain
            .map(|g| (g, baked.album_peak))
            .or_else(|| baked.track_gain.map(|g| (g, baked.track_peak))),
        RgMode::Track => baked
            .track_gain
            .map(|g| (g, baked.track_peak))
            .or_else(|| baked.album_gain.map(|g| (g, baked.album_peak))),
    };

    let Some((gain_db, peak)) = pair else {
        return 1.0; // no gain data → unity
    };

    let mut lin = db_to_linear(gain_db + preamp_db);

    // Static peak-based clip guard: don't let the boosted signal exceed full
    // scale. Only applied when a positive peak is known; otherwise the
    // downstream limiter is the safety net.
    if prevent_clipping
        && let Some(p) = peak
        && p > 0.0
    {
        lin = lin.min(1.0 / p);
    }

    lin
}

/// Whether a computed linear gain is close enough to unity to skip processing.
#[must_use]
pub fn is_unity_gain(lin: f32) -> bool {
    (lin - 1.0).abs() < RG_UNITY_EPSILON
}

/// Lock-free `ReplayGain` master state shared between the control layer (writer)
/// and the audio thread (reader). Every mutation bumps the [`Generation`] so the
/// [`EqSource`](super::equalizer::EqSource) reading it knows to recompute its
/// baked gain — the same poll pattern as
/// [`EqShared`](super::equalizer::EqShared).
pub struct ReplayGainShared {
    enabled: AtomicBool,
    mode: AtomicU8,
    preamp_bits: AtomicU32,
    prevent_clipping: AtomicBool,
    generation: Generation,
}

impl ReplayGainShared {
    /// Build shared state seeded inert (disabled, Album, 0 dB, prevent-clipping
    /// on). `AppState::init` applies the persisted values before playback via the
    /// setters below.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            enabled: AtomicBool::new(false),
            mode: AtomicU8::new(RgMode::Album.to_u8()),
            preamp_bits: AtomicU32::new(RG_DEFAULT_PREAMP_DB.to_bits()),
            prevent_clipping: AtomicBool::new(true),
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

    pub fn set_mode(&self, mode: RgMode) {
        self.mode.store(mode.to_u8(), Ordering::Relaxed);
        self.bump();
    }

    pub fn set_preamp(&self, db: f32) {
        self.preamp_bits.store(clamp_rg_preamp(db).to_bits(), Ordering::Relaxed);
        self.bump();
    }

    pub fn set_prevent_clipping(&self, on: bool) {
        self.prevent_clipping.store(on, Ordering::Relaxed);
        self.bump();
    }

    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn mode(&self) -> RgMode {
        RgMode::from_u8(self.mode.load(Ordering::Relaxed))
    }

    #[must_use]
    pub fn preamp(&self) -> f32 {
        f32::from_bits(self.preamp_bits.load(Ordering::Relaxed))
    }

    #[must_use]
    pub fn prevent_clipping(&self) -> bool {
        self.prevent_clipping.load(Ordering::Relaxed)
    }

    #[must_use]
    pub(crate) fn generation(&self) -> u64 {
        self.generation.get()
    }
}

#[cfg(test)]
#[path = "tests/replaygain_tests.rs"]
mod tests;
