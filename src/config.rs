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

impl Paths {
    /// Resolves every path Melodia owns under the user's data directory,
    /// creating the directories it names.
    pub fn resolve() -> AppResult<Self> {
        let data_dir = dirs::data_dir()
            .ok_or_else(|| AppError::Settings("could not resolve user data directory".into()))?
            .join("Melodia");
        let paths = Self::rooted_at(data_dir);
        paths.create_dirs()?;
        Ok(paths)
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
