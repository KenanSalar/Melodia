//! Load, save and atomic read-mutate-write for `settings.json`.

use melodia_core::config::Paths;
use melodia_core::error::AppResult;

use super::{MAX_CORNER_RADIUS, SettingsData};

pub fn read_settings(paths: &Paths) -> AppResult<SettingsData> {
    let mut settings: SettingsData =
        melodia_core::utils::atomic_file::load_json_or_default_sync(&paths.settings_path)?;
    settings.volume = settings.volume.min(melodia_engine::player::engine::state::MAX_VOLUME);
    settings.corner_radius = settings.corner_radius.min(MAX_CORNER_RADIUS);
    Ok(settings)
}

/// The whole file, unlocked. Crate-private: every writer goes through the `mutate_settings`
/// family, and only a test fixture seeding a fresh root may write a file wholesale.
pub(crate) fn write_settings(paths: &Paths, settings: &SettingsData) -> AppResult<()> {
    melodia_core::utils::atomic_file::write_json_sync(&paths.settings_path, settings)
}

/// Held across the read, mutate and write, so a burst of UI events can't interleave snapshots and
/// lose updates. A bare `read_settings` stays unlocked, so writing back what one returned is the
/// lost update this exists to stop.
static MUTATE_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// Reads, mutates and writes `settings.json` under `MUTATE_LOCK`.
pub fn mutate_settings<F>(paths: &Paths, mutate: F) -> AppResult<()>
where
    F: FnOnce(&mut SettingsData),
{
    mutate_settings_with(paths, mutate)
}

/// [`mutate_settings`], handing back the closure's value. It always writes; a caller that often
/// changes nothing takes [`mutate_settings_if`].
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

/// [`mutate_settings`] for a caller that often changes nothing: the file is written only when the
/// closure returns `true`, still under `MUTATE_LOCK`.
pub fn mutate_settings_if<F>(paths: &Paths, mutate: F) -> AppResult<()>
where
    F: FnOnce(&mut SettingsData) -> bool,
{
    let _guard = MUTATE_LOCK.lock();
    let mut settings = read_settings(paths)?;
    if mutate(&mut settings) {
        write_settings(paths, &settings)?;
    }
    Ok(())
}
