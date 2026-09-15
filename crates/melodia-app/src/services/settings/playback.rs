//! The flag structs behind the Settings ▸ Playback tab.

use serde::{Deserialize, Serialize};

use melodia_engine::player::engine::types::RepeatMode;
use melodia_playback::player::playback::crossfade::DEFAULT_CROSSFADE_MS;
use melodia_playback::player::playback::equalizer::{DEFAULT_PRESET, NUM_BANDS};
use melodia_playback::player::playback::replaygain::{DEFAULT_MODE, RG_DEFAULT_PREAMP_DB};

/// Audio-playback preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaybackFlags {
    pub gapless_playback: bool,
    pub resume_on_startup: bool,
    pub is_muted: bool,
    /// Rate multiplier, clamped to the player's `MIN_SPEED..=MAX_SPEED` when
    /// applied or persisted.
    pub playback_speed: f64,
}

impl Default for PlaybackFlags {
    fn default() -> Self {
        Self {
            gapless_playback: true,
            resume_on_startup: false,
            is_muted: false,
            playback_speed: 1.0,
        }
    }
}

/// Graphic-equalizer preferences. Ships **off** with a flat curve, so a fresh
/// install sounds bit-identical to no EQ until the user opts in. Gains are in
/// dB and go through `equalizer::normalize_gains` on read, so a hand-edited or
/// wrong-length array can't pin a bad value.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EqualizerFlags {
    pub eq_enabled: bool,
    pub eq_band_gains: Vec<f32>,
    pub eq_selected_preset: String,
    /// Master gain in dB, clamped to `MIN_PREAMP_DB..=MAX_PREAMP_DB`.
    pub eq_preamp: f32,
}

impl Default for EqualizerFlags {
    fn default() -> Self {
        Self {
            eq_enabled: false,
            eq_band_gains: vec![0.0; NUM_BANDS],
            eq_selected_preset: DEFAULT_PRESET.to_owned(),
            eq_preamp: 0.0,
        }
    }
}

/// `ReplayGain` (loudness normalization) preferences. Ships **off**, so a fresh
/// install plays at the raw recorded level until the user opts in; the
/// `rg_prevent_clipping` guard then defaults **on** so a boosted track can't
/// clip. `rg_mode` is a lowercase token, falling back to Album on an unknown
/// value.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplayGainFlags {
    pub rg_enabled: bool,
    pub rg_mode: String,
    /// Extra preamp in dB, clamped to `RG_MIN_PREAMP_DB..=RG_MAX_PREAMP_DB`.
    pub rg_preamp: f32,
    pub rg_prevent_clipping: bool,
}

impl Default for ReplayGainFlags {
    fn default() -> Self {
        Self {
            rg_enabled: false,
            rg_mode: DEFAULT_MODE.to_owned(),
            rg_preamp: RG_DEFAULT_PREAMP_DB,
            rg_prevent_clipping: true,
        }
    }
}

/// Crossfade preferences. Ships **off**, so an install keeps the gapless
/// behaviour it already has. Once enabled, `crossfade_skip_same_album` defaults
/// **on** so continuous-mix albums stay gapless. `crossfade_duration_ms` is clamped to
/// `MIN_CROSSFADE_MS..=MAX_CROSSFADE_MS`.
#[allow(
    clippy::struct_excessive_bools,
    reason = "one serde field per independent user-facing toggle; each must round-trip through settings.json by name"
)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CrossfadeFlags {
    pub crossfade_enabled: bool,
    pub crossfade_duration_ms: u32,
    pub crossfade_manual: bool,
    pub crossfade_skip_same_album: bool,
    pub crossfade_fade_on_pause: bool,
}

impl Default for CrossfadeFlags {
    fn default() -> Self {
        Self {
            crossfade_enabled: false,
            crossfade_duration_ms: DEFAULT_CROSSFADE_MS,
            crossfade_manual: false,
            crossfade_skip_same_album: true,
            crossfade_fade_on_pause: false,
        }
    }
}

/// Audio-visualizer preferences — the one feature here that ships **on**, being
/// a presentation flourish confined to the Now-Playing view rather than
/// something that alters what you hear.
///
/// `viz_enabled` decides whether the strip *mounts* and nothing more: the
/// audio-thread tap is armed by the view being on screen, so leaving this on
/// costs nothing while the view is closed.
///
/// `viz_style` is a **key**, not an index into the picker — an index would
/// silently repoint every existing install the day the style list is reordered.
/// An unrecognized key resolves back to the default.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VisualizerFlags {
    pub viz_enabled: bool,
    pub viz_style: String,
}

/// The style key a fresh install starts on, and the one an unrecognized key
/// resolves back to. `melodia-views`' `ui/visualizer/`'s style table must *head with the
/// same key* — its own fallbacks land on index 0 — which its tests pin.
pub const DEFAULT_VIZ_STYLE: &str = "bars";

impl Default for VisualizerFlags {
    fn default() -> Self {
        Self { viz_enabled: true, viz_style: DEFAULT_VIZ_STYLE.to_owned() }
    }
}

/// Queue-behavior preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct QueueFlags {
    pub shuffle_enabled: bool,
    pub repeat_mode: RepeatMode,
}

impl Default for QueueFlags {
    fn default() -> Self {
        Self { shuffle_enabled: false, repeat_mode: RepeatMode::Off }
    }
}
