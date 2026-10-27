use std::path::PathBuf;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone)]
pub struct Paths {
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub settings_path: PathBuf,
    pub view_state_path: PathBuf,
    pub queue_path: PathBuf,
    pub search_history_path: PathBuf,
    pub scrobble_credentials_path: PathBuf,
    pub scrobble_queue_path: PathBuf,
    pub artwork_dir: PathBuf,
    pub artists_dir: PathBuf,
    /// Apart from [`Self::artwork_dir`], whose contents a re-scan rebuilds: a logo came off a
    /// third-party host and can't be.
    pub radio_logos_dir: PathBuf,
    /// Fetched sheets and no-answer markers. A directory rather than a table, so the filesystem is
    /// the whole index and the sweep needs no reference set.
    pub lyrics_dir: PathBuf,
    /// Its own directory, so the retention sweep runs away from the live database's sidecars.
    pub backups_dir: PathBuf,
    /// Logs and crash reports together, so "Open log folder" shows a reporter everything; each
    /// sweep is gated on its own naming scheme.
    pub logs_dir: PathBuf,
}

const DATA_DIR_NAME: &str = "Melodia";

/// A dev build's own root: a migration is irreversible, so a branch-local one would leave the
/// installed Melodia unable to open its database.
const DEV_DATA_DIR_NAME: &str = "Melodia-dev";

/// Overrides the choice. Made absolute and component-collected because `single_instance` hashes
/// the root as spelled, and two spellings of one directory are two writers on one database; `..`
/// and symlinks survive, `canonicalize` not being worth the disk touch.
pub const DATA_DIR_ENV: &str = "MELODIA_DATA_DIR";

impl Paths {
    /// Resolves every path Melodia owns, creating the directories it names.
    pub fn resolve() -> AppResult<Self> {
        let paths = Self::rooted_at(Self::data_root()?);
        paths.create_dirs()?;
        Ok(paths)
    }

    fn data_root() -> AppResult<PathBuf> {
        Self::data_root_for(crate::utils::exe::is_dev_build())
    }

    /// The half a test drives, [`is_dev_build`](crate::utils::exe::is_dev_build) being pinned to
    /// `true` in a test binary. The environment read stays inside, so a test still proves
    /// [`DATA_DIR_ENV`] is the variable consulted.
    fn data_root_for(is_dev: bool) -> AppResult<PathBuf> {
        if let Some(root) = std::env::var_os(DATA_DIR_ENV).filter(|root| !root.is_empty()) {
            return Ok(std::path::absolute(root)?.components().collect());
        }
        let name = if is_dev { DEV_DATA_DIR_NAME } else { DATA_DIR_NAME };
        Ok(dirs::data_dir()
            .ok_or_else(|| AppError::settings_msg("could not resolve user data directory"))?
            .join(name))
    }

    /// Touches no disk, so a test can root the tree in a `TempDir` rather than steer
    /// `XDG_DATA_HOME`, a process-global mutation.
    #[must_use]
    pub fn rooted_at(data_dir: PathBuf) -> Self {
        Self {
            db_path: data_dir.join("melodia.db"),
            settings_path: data_dir.join("settings.json"),
            view_state_path: data_dir.join("views.json"),
            queue_path: data_dir.join("queue.json"),
            search_history_path: data_dir.join("search_history.json"),
            scrobble_credentials_path: data_dir.join("scrobble_credentials.json"),
            scrobble_queue_path: data_dir.join("scrobble_queue.json"),
            artwork_dir: data_dir.join("artwork"),
            artists_dir: data_dir.join("artists"),
            radio_logos_dir: data_dir.join("radio-logos"),
            lyrics_dir: data_dir.join("lyrics"),
            backups_dir: data_dir.join("backups"),
            logs_dir: data_dir.join("logs"),
            data_dir,
        }
    }

    /// Creates the data directory and every subdirectory under it.
    pub fn create_dirs(&self) -> AppResult<()> {
        for dir in [
            &self.data_dir,
            &self.artwork_dir,
            &self.artists_dir,
            &self.radio_logos_dir,
            &self.lyrics_dir,
            &self.backups_dir,
            &self.logs_dir,
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/config_tests.rs"]
mod tests;
