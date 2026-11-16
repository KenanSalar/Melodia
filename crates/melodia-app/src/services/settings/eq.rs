//! `eq.json`: the equalizer's curve and every device's headphone correction, the curves a user
//! builds up, apart from the switches `settings.json` keeps with the other preferences.

use serde::{Deserialize, Serialize};

use super::io::{LockedFile, mutate_settings};
use super::{LegacyEq, SettingsData};
use melodia_core::config::Paths;
use melodia_core::entities::correction::DeviceCorrections;
use melodia_core::error::AppResult;
use melodia_core::utils::atomic_file;
use melodia_playback::player::playback::equalizer::{DEFAULT_PRESET, NUM_BANDS};

/// Ships flat and empty. Gains are dB, low to high, through `equalizer::normalize_gains` on read so
/// a hand-edited or wrong-length array can't pin a bad value.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EqData {
    pub band_gains: Vec<f32>,
    pub preset: String,
    /// Master gain in dB, clamped to `MIN_PREAMP_DB..=MAX_PREAMP_DB`.
    pub preamp: f32,
    pub corrections: DeviceCorrections,
}

impl Default for EqData {
    fn default() -> Self {
        Self {
            band_gains: vec![0.0; NUM_BANDS],
            preset: DEFAULT_PRESET.to_owned(),
            preamp: 0.0,
            corrections: DeviceCorrections::default(),
        }
    }
}

static EQ_FILE: LockedFile<EqData> = LockedFile::new(read_eq, write_eq);

/// The defaults where the file is missing, and a key that won't read reset alone.
pub fn read_eq(paths: &Paths) -> AppResult<EqData> {
    atomic_file::load_json_or_default_sync(&paths.eq_path)
}

fn write_eq(paths: &Paths, eq: &EqData) -> AppResult<()> {
    atomic_file::write_json_sync(&paths.eq_path, eq)
}

/// Reads, mutates and writes `eq.json` under its own lock.
pub fn mutate_eq<F>(paths: &Paths, mutate: F) -> AppResult<()>
where
    F: FnOnce(&mut EqData),
{
    EQ_FILE.mutate_with(paths, mutate)
}

/// Moves the curve a `settings.json` older than `eq.json` kept into a new `eq.json`, then drops it
/// from `settings.json`: in that order, so a failed write leaves it where it was for the next boot
/// to retry. An `eq.json` already there wins. Boot runs it ahead of the engine's seed, which a
/// `one_shot` sweep, taking the `AppState` built after that seed, would come too late for.
pub fn move_legacy_eq(paths: &Paths, settings: &SettingsData) -> AppResult<()> {
    if settings.legacy_eq.is_empty() {
        return Ok(());
    }
    if !paths.eq_path.try_exists()? {
        let legacy = settings.legacy_eq.clone();
        mutate_eq(paths, move |eq| legacy.move_into(eq))?;
    }
    mutate_settings(paths, |settings| settings.legacy_eq = LegacyEq::default())
}

#[cfg(test)]
#[path = "../tests/eq_tests.rs"]
mod tests;
