//! `ReplayGain` state and gain math, mirroring the equalizer. [`TrackReplayGain`] is baked into
//! each [`EqSource`](super::equalizer::EqSource), so a staged track carries its own gain, and
//! [`ReplayGainShared`] holds the master controls the audio thread polls by generation. Applied
//! before the EQ bands, so the limiter guards a boost, whether or not the EQ is on.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use melodia_core::entities::track::TrackSummary;

use super::dsp::{AtomicF32, Generation, db_to_linear};

/// Symmetric, unlike the EQ's, being a listener's loudness preference.
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

#[must_use]
pub fn clamp_rg_preamp(db: f32) -> f32 {
    if db.is_nan() { 0.0 } else { db.clamp(RG_MIN_PREAMP_DB, RG_MAX_PREAMP_DB) }
}

/// No `Off`: [`ReplayGainShared`]'s `enabled` is the one gate, as the EQ's is.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum RgMode {
    /// Every track normalized alone, best for shuffle.
    Track,
    /// Album-relative levels kept, best for listening to whole albums.
    #[default]
    Album,
}

impl RgMode {
    /// Also the dropdown's order.
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

/// Gains in dB as tagged and peaks linear, 1.0 being full scale; all `None` is unity.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct TrackReplayGain {
    pub track_gain: Option<f32>,
    pub track_peak: Option<f32>,
    pub album_gain: Option<f32>,
    pub album_peak: Option<f32>,
}

/// Here rather than on `TrackSummary`, a row type having no business naming this one.
impl From<&TrackSummary> for TrackReplayGain {
    fn from(track: &TrackSummary) -> Self {
        Self {
            track_gain: track.replaygain_track_gain,
            track_peak: track.replaygain_track_peak,
            album_gain: track.replaygain_album_gain,
            album_peak: track.replaygain_album_peak,
        }
    }
}

/// Unity with no gain data, so an untagged track plays unchanged.
#[must_use]
pub fn compute_linear_gain(
    baked: TrackReplayGain,
    mode: RgMode,
    preamp_db: f32,
    prevent_clipping: bool,
) -> f32 {
    // The other pair where the mode's gain is absent, so the peak always matches the gain.
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
        return 1.0;
    };

    let mut lin = db_to_linear(gain_db + preamp_db);

    // Only with a known peak; otherwise the downstream limiter is the safety net.
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

/// The master state, on [`EqShared`](super::equalizer::EqShared)'s generation-poll pattern.
pub struct ReplayGainShared {
    enabled: AtomicBool,
    mode: AtomicU8,
    preamp: AtomicF32,
    prevent_clipping: AtomicBool,
    generation: Generation,
}

impl ReplayGainShared {
    /// Inert until `AppState::init` applies the persisted values.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            enabled: AtomicBool::new(false),
            mode: AtomicU8::new(RgMode::Album.to_u8()),
            preamp: AtomicF32::new(RG_DEFAULT_PREAMP_DB),
            prevent_clipping: AtomicBool::new(true),
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

    pub fn set_mode(&self, mode: RgMode) {
        self.mode.store(mode.to_u8(), Ordering::Relaxed);
        self.bump();
    }

    pub fn set_preamp(&self, db: f32) {
        self.preamp.store(clamp_rg_preamp(db));
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
        self.preamp.load()
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
