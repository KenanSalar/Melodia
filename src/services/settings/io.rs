//! Load / save / atomic-mutate for `settings.json`. The data model these
//! functions read and write lives in the sibling [`super::data`] module.

use crate::config::Paths;
use crate::error::{AppError, AppResult};

use super::{MAX_CORNER_RADIUS, SettingsData};

pub fn read_settings(paths: &Paths) -> AppResult<SettingsData> {
    let mut settings: SettingsData =
        crate::services::load_json_or_default_sync(&paths.settings_path)?;
    settings.volume = settings.volume.min(crate::player::state::MAX_VOLUME);
    settings.corner_radius = settings.corner_radius.min(MAX_CORNER_RADIUS);
    Ok(settings)
}

pub fn write_settings(paths: &Paths, settings: &SettingsData) -> AppResult<()> {
    crate::services::write_json_atomic_sync(&paths.settings_path, settings)
        .map_err(|e| AppError::Settings(format!("Failed to write settings: {e}")))
}

/// Process-wide lock around `settings.json` mutation. Held by `mutate_settings`
/// for the entire read→mutate→write window so a burst of UI events (e.g. a
/// rapid theme + variant + accent click sequence) can't interleave their read
/// snapshots and lose updates. Plain `read_settings` and `write_settings` stay
/// lock-free — single-shot reads and full-replacement writes are inherently
/// race-free against each other.
static MUTATE_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// Atomically read → mutate → write `settings.json`. Serializes against every
/// other `mutate_settings` caller so concurrent partial-update callsites
/// (`set_appearance`, `update_view_columns`, …) merge cleanly without tearing
/// each other's reads.
pub fn mutate_settings<F>(paths: &Paths, mutate: F) -> AppResult<()>
where
    F: FnOnce(&mut SettingsData),
{
    mutate_settings_with(paths, mutate)
}

/// Read → inspect/mutate → optionally write `settings.json` under the
/// same `MUTATE_LOCK` as `mutate_settings`. The closure both reads and
/// optionally mutates the settings, returning a value the caller wants
/// to retain (typically the pre-mutation snapshot of one or two fields).
///
/// The closure receives `&mut SettingsData` so it can mutate in place;
/// the write step always runs, mirroring `mutate_settings`. If the
/// closure makes no changes the write is a redundant full-replacement,
/// but it's still cheaper than two separate `spawn_blocking` round-trips.
pub fn mutate_settings_with<F, R>(paths: &Paths, mutate: F) -> AppResult<R>
where
    F: FnOnce(&mut SettingsData) -> R,
{
    let _guard = MUTATE_LOCK.lock();
    let mut settings = read_settings(paths)?;
    let out = mutate(&mut settings);
    write_settings(paths, &settings)?;
    Ok(out)
}
