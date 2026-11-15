//! Load, save and atomic read-mutate-write for `settings.json`, and the locked shape `eq.json`
//! shares.

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

/// A JSON state file written whole, its lock held across the read, mutate and write so a burst of
/// UI events can't interleave snapshots and lose updates. A bare read stays unlocked, so writing
/// back what one returned is the lost update this exists to stop.
pub(crate) struct LockedFile<T> {
    lock: parking_lot::Mutex<()>,
    read: fn(&Paths) -> AppResult<T>,
    write: fn(&Paths, &T) -> AppResult<()>,
}

impl<T> LockedFile<T> {
    pub(crate) const fn new(
        read: fn(&Paths) -> AppResult<T>,
        write: fn(&Paths, &T) -> AppResult<()>,
    ) -> Self {
        Self { lock: parking_lot::Mutex::new(()), read, write }
    }

    /// Always writes; a caller that often changes nothing takes [`Self::mutate_if`].
    pub(crate) fn mutate_with<R>(
        &self,
        paths: &Paths,
        mutate: impl FnOnce(&mut T) -> R,
    ) -> AppResult<R> {
        let _guard = self.lock.lock();
        let mut value = (self.read)(paths)?;
        let out = mutate(&mut value);
        (self.write)(paths, &value)?;
        Ok(out)
    }

    /// Writes only when the closure returns `true`, still under the lock.
    pub(crate) fn mutate_if(
        &self,
        paths: &Paths,
        mutate: impl FnOnce(&mut T) -> bool,
    ) -> AppResult<()> {
        let _guard = self.lock.lock();
        let mut value = (self.read)(paths)?;
        if mutate(&mut value) {
            (self.write)(paths, &value)?;
        }
        Ok(())
    }
}

static SETTINGS_FILE: LockedFile<SettingsData> = LockedFile::new(read_settings, write_settings);

/// Reads, mutates and writes `settings.json` under its lock.
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
    SETTINGS_FILE.mutate_with(paths, mutate)
}

/// [`mutate_settings`] for a caller that often changes nothing: the file is written only when the
/// closure returns `true`, still under the lock.
pub fn mutate_settings_if<F>(paths: &Paths, mutate: F) -> AppResult<()>
where
    F: FnOnce(&mut SettingsData) -> bool,
{
    SETTINGS_FILE.mutate_if(paths, mutate)
}
