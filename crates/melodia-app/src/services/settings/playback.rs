//! The flag structs behind Settings ▸ Playback, ▸ Effects and ▸ Output, and behind the
//! visualizer on ▸ Appearance.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use melodia_engine::player::engine::backend::OutputChoice;
use melodia_engine::player::engine::types::RepeatMode;
use melodia_playback::player::playback::crossfade::DEFAULT_CROSSFADE_MS;
use melodia_playback::player::playback::equalizer::{DEFAULT_PRESET, NUM_BANDS};
use melodia_playback::player::playback::output::convert::Resampler;
use melodia_playback::player::playback::output::{
    DEFAULT_RESYNC_HOLD, Drive, ExclusiveTuning, OutputMode, RateFallback,
};
use melodia_playback::player::playback::replaygain::{DEFAULT_MODE, RG_DEFAULT_PREAMP_DB};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaybackFlags {
    pub gapless_playback: bool,
    pub resume_on_startup: bool,
    pub is_muted: bool,
    /// Clamped to the player's `MIN_SPEED..=MAX_SPEED` when applied or persisted.
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

/// Ships off and flat. Gains are dB, through `equalizer::normalize_gains` on read so a hand-edited
/// or wrong-length array can't pin a bad value.
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

/// Ships off, with `rg_prevent_clipping` on for once it isn't. `rg_mode` is a lowercase token, an
/// unknown one reading as Album.
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

/// Ships off, with `crossfade_skip_same_album` on so a continuous mix stays gapless once it isn't.
/// `crossfade_duration_ms` is clamped to `MIN_CROSSFADE_MS..=MAX_CROSSFADE_MS`.
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

/// How the output device is opened. Each number is held to its range on the way to the engine, so
/// a hand-edited value can't pin a bad one.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputFlags {
    /// Reopens at each track's own rate, costing a short silence and the crossfade at every rate
    /// boundary, so it ships off.
    pub output_follow_rate: bool,
    /// Ships shared, an exclusive claim taking the card from every other application.
    #[serde(with = "OutputModeKey")]
    pub output_mode: OutputMode,
    /// The card's stable id, `None` for the first one listed. Where it names a shared target too
    /// (Windows), both modes play through it.
    pub output_device: Option<String>,
    /// With `output_polling`, paces an exclusive claim's writer.
    pub output_period_ms: u32,
    pub output_polling: bool,
    /// The silence written after a reopen onto a new rate.
    pub output_resync_ms: u32,
    /// Carries a claim's volume on the device's own control, which is also its system volume, so
    /// it ships off.
    pub output_hardware_volume: bool,
    /// Converts to a rate the device has rather than giving the claim up, which it ships doing.
    #[serde(with = "RateFallbackKey")]
    pub output_rate_fallback: RateFallback,
    /// Gives a claimed device back after a long pause. Ships keeping it, play then having to
    /// reopen; kept out of the choice so a toggle never reopens a claim.
    pub output_paused_device: PausedDevice,
    /// How every conversion interpolates; [`Resampler::default`] argues the platform split.
    #[serde(with = "ResamplerKey")]
    pub output_resampler: Resampler,
}

impl Default for OutputFlags {
    fn default() -> Self {
        Self {
            output_follow_rate: false,
            output_mode: OutputMode::default(),
            output_device: None,
            output_period_ms: duration_ms(ExclusiveTuning::DEFAULT_PERIOD),
            output_polling: false,
            output_resync_ms: duration_ms(DEFAULT_RESYNC_HOLD),
            output_hardware_volume: false,
            output_rate_fallback: RateFallback::default(),
            output_paused_device: PausedDevice::default(),
            output_resampler: Resampler::default(),
        }
    }
}

impl OutputFlags {
    pub fn resync_hold(&self) -> Duration {
        Duration::from_millis(u64::from(self.output_resync_ms))
    }

    pub fn output_choice(&self) -> OutputChoice {
        let period = Duration::from_millis(u64::from(self.output_period_ms));
        OutputChoice {
            mode: self.output_mode,
            device: self.output_device.clone(),
            tuning: ExclusiveTuning::new(period, Drive::from_polling(self.output_polling)),
            hardware_volume: self.output_hardware_volume,
            rate_fallback: self.output_rate_fallback,
        }
    }

    /// [`Self::output_choice`]'s way back, for persisting what the engine was just handed.
    pub fn set_output_choice(&mut self, choice: &OutputChoice) {
        self.output_mode = choice.mode;
        self.output_device.clone_from(&choice.device);
        self.output_period_ms = duration_ms(choice.tuning.period);
        self.output_polling = choice.tuning.drive == Drive::Polling;
        self.output_hardware_volume = choice.hardware_volume;
        self.output_rate_fallback = choice.rate_fallback;
    }
}

fn duration_ms(duration: Duration) -> u32 {
    u32::try_from(duration.as_millis()).unwrap_or(u32::MAX)
}

/// [`OutputMode`] as persisted: a key, so reordering the picker repoints nothing. A mirror rather
/// than a derive, `melodia-playback` naming no serde.
#[derive(Serialize, Deserialize)]
#[serde(remote = "OutputMode", rename_all = "lowercase")]
enum OutputModeKey {
    Shared,
    Exclusive,
}

/// [`RateFallback`] as persisted, mirrored for the same reasons as [`OutputModeKey`].
#[derive(Serialize, Deserialize)]
#[serde(remote = "RateFallback", rename_all = "lowercase")]
enum RateFallbackKey {
    Shared,
    Resample,
}

/// [`Resampler`] as persisted, mirrored for the same reasons as [`OutputModeKey`].
#[derive(Serialize, Deserialize)]
#[serde(remote = "Resampler", rename_all = "snake_case")]
enum ResamplerKey {
    Linear,
    BandLimited,
}

/// What a long pause does with an exclusive device. A token rather than a `bool`, [`OutputFlags`]
/// already sitting at clippy's `struct_excessive_bools` cap.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PausedDevice {
    #[default]
    Keep,
    Release,
}

impl PausedDevice {
    /// The token for the Output card's toggle, which is on for [`Self::Release`].
    pub fn from_toggle(release: bool) -> Self {
        if release { Self::Release } else { Self::Keep }
    }

    /// [`Self::from_toggle`]'s way back, which is also what the engine is handed.
    pub fn releases(self) -> bool {
        self == Self::Release
    }
}

/// Ships on, a flourish confined to Now Playing rather than anything you hear. `viz_enabled` only
/// decides whether the strip mounts, the audio tap arming with the view on screen. `viz_style` is
/// a key rather than a picker index, so reordering the styles repoints no install.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VisualizerFlags {
    pub viz_enabled: bool,
    pub viz_style: String,
}

/// The style a fresh install starts on and an unknown key reads as. `melodia-views`'
/// `ui/visualizer/` style table must head with it, its own fallbacks landing on index 0.
pub const DEFAULT_VIZ_STYLE: &str = "bars";

impl Default for VisualizerFlags {
    fn default() -> Self {
        Self { viz_enabled: true, viz_style: DEFAULT_VIZ_STYLE.to_owned() }
    }
}

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
