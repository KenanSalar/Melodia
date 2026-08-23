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
    /// Ids the MBID auto-tag backfill has already looked up (matched or not), so
    /// unmatched tracks aren't re-queried on every launch. Cleared by a manual
    /// "Look up missing IDs" kick.
    pub scrobble_mbid_state_path: PathBuf,
    pub artwork_dir: PathBuf,
    pub artists_dir: PathBuf,
    /// Pre-migration copies of [`Self::db_path`], written by
    /// `database::backup`. Its own directory so the retention sweep runs
    /// somewhere the live database and its `-wal`/`-shm` sidecars aren't.
    pub backups_dir: PathBuf,
    /// The rolling log files `services::logging` writes and the crash reports
    /// `services::crash_report` drops beside them. One directory rather than
    /// two, so "Open log folder" shows a reporter everything at once — the two
    /// naming schemes don't overlap and each sweep is gated on its own.
    pub logs_dir: PathBuf,
}

/// The folder an installed Melodia owns under the platform's data directory.
const DATA_DIR_NAME: &str = "Melodia";

/// Its sibling for a build run out of the source tree.
///
/// A migration is applied on the first boot that carries it and is not reversible, so a
/// branch-local one leaves the *installed* Melodia unable to open its own database until that
/// branch ships. Separate roots is the only fix that doesn't ask the developer to remember
/// anything: a dev build has its own database, settings, queue and artwork store, and the
/// single-instance claim keys on the root, so the two can run side by side.
const DEV_DATA_DIR_NAME: &str = "Melodia-dev";

/// Overrides the whole choice above, for the cases the build shape can't answer: pointing a dev
/// build at real data to reproduce something, or an installed one at a copy.
///
/// Normalized before anything reads it, because `single_instance` hashes the root *as spelled* and
/// two spellings of one directory are two writers on one database: absolute, since a relative value
/// moves with the working directory a `.desktop` and a terminal launch disagree about, and
/// component-collected, since `absolute` keeps a trailing separator on purpose. `..` and symlinks
/// survive both, `canonicalize` being the only answer and not worth the disk touch.
pub const DATA_DIR_ENV: &str = "MELODIA_DATA_DIR";

impl Paths {
    /// Resolves every path Melodia owns under the user's data directory,
    /// creating the directories it names.
    pub fn resolve() -> AppResult<Self> {
        let paths = Self::rooted_at(Self::data_root()?);
        paths.create_dirs()?;
        Ok(paths)
    }

    /// The root [`resolve`](Self::resolve) hands to [`rooted_at`](Self::rooted_at).
    fn data_root() -> AppResult<PathBuf> {
        Self::data_root_for(crate::services::is_dev_build())
    }

    /// The half a test can drive, with the one answer it cannot steer passed in: a test binary
    /// carries `debug_assertions`, so [`is_dev_build`](crate::services::is_dev_build) is pinned to
    /// `true` there and the installed branch below would otherwise never run.
    ///
    /// The environment read stays here rather than being lifted with it, so a test still proves
    /// [`DATA_DIR_ENV`] is the variable actually consulted rather than only the mapping.
    fn data_root_for(is_dev: bool) -> AppResult<PathBuf> {
        if let Some(root) = std::env::var_os(DATA_DIR_ENV).filter(|root| !root.is_empty()) {
            return Ok(std::path::absolute(root)?.components().collect());
        }
        let name = if is_dev {
            DEV_DATA_DIR_NAME
        } else {
            DATA_DIR_NAME
        };
        Ok(dirs::data_dir()
            .ok_or_else(|| AppError::Settings("could not resolve user data directory".into()))?
            .join(name))
    }

    /// Derives every path Melodia owns from `data_dir`, touching no disk —
    /// [`create_dirs`](Self::create_dirs) is the half that does.
    ///
    /// Split out so a test can root the tree in a `TempDir` directly. Steering
    /// `dirs::data_dir()` through `XDG_DATA_HOME` instead is a process-global
    /// mutation, which an integration test can only reach through `unsafe`.
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
            scrobble_mbid_state_path: data_dir.join("scrobble_mbid_attempted.json"),
            artwork_dir: data_dir.join("artwork"),
            artists_dir: data_dir.join("artists"),
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
